use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc::Sender};
use std::time::{Duration, Instant};

use crate::input::JoypadState;

pub const SCREEN_WIDTH: usize = 160;
pub const SCREEN_HEIGHT: usize = 144;
pub const BYTES_PER_PIXEL: usize = 4;
pub const FRAMEBUFFER_SIZE: usize = SCREEN_WIDTH * SCREEN_HEIGHT * BYTES_PER_PIXEL;
pub const IO_REGISTERS_LEN: usize = 0x80;
pub const HRAM_LEN: usize = 0x7f;
pub const WRAM_BANK_SIZE: usize = 0x1000;
pub const WRAM_BANKS: usize = 8;
const SHARED_ROM_BANK_SIZE: usize = 0x4000;
pub const CPU_CLOCK_HZ: u64 = 4_194_304;

/// Shared interrupt flags register (IF at 0xFF0F).
/// Components set bits via atomic OR; CPU reads/clears directly.
#[derive(Clone, Debug)]
pub struct InterruptFlags {
    flags: Arc<AtomicU8>,
}

impl InterruptFlags {
    pub fn new() -> Self {
        Self {
            flags: Arc::new(AtomicU8::new(0)),
        }
    }

    pub fn load(&self) -> u8 {
        self.flags.load(Ordering::Acquire)
    }

    pub fn store(&self, value: u8) {
        self.flags.store(value & 0x1f, Ordering::Release);
    }

    pub fn set(&self, mask: u8) {
        self.flags.fetch_or(mask, Ordering::Release);
    }

    pub fn clear(&self, mask: u8) {
        self.flags.fetch_and(!mask, Ordering::Release);
    }
}

#[derive(Clone, Debug)]
pub struct MasterClock {
    target_cycle: Arc<AtomicU64>,
}

impl MasterClock {
    pub fn new() -> Self {
        Self {
            target_cycle: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn target(&self) -> u64 {
        self.target_cycle.load(Ordering::Acquire)
    }

    pub fn advance(&self, cycles: u64) {
        self.target_cycle.store(cycles, Ordering::Release);
    }

    pub fn store(&self, value: u64) {
        self.target_cycle.store(value, Ordering::Release);
    }
}

/// Shared PPU dot counter — lets the CPU know how far the PPU has advanced.
#[derive(Clone, Debug)]
pub struct PpuProgress {
    dots: Arc<AtomicU64>,
}

impl PpuProgress {
    pub fn new() -> Self {
        Self { dots: Arc::new(AtomicU64::new(0)) }
    }

    pub fn get(&self) -> u64 {
        self.dots.load(Ordering::Acquire)
    }

    pub fn set(&self, dots: u64) {
        self.dots.store(dots, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum HardwareMode {
    Cgb,
    DmgCompatibility,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThreadDelays {
    pub cpu: Duration,
    pub ppu: Duration,
    pub wram: Duration,
    pub cartridge: Duration,
    pub timer: Duration,
    pub apu: Duration,
}

impl Default for ThreadDelays {
    fn default() -> Self {
        Self {
            cpu: Duration::ZERO,
            ppu: Duration::from_nanos(16_536_000),
            wram: Duration::ZERO,
            cartridge: Duration::ZERO,
            timer: Duration::ZERO,
            apu: Duration::ZERO,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub cpu_steps: u64,
    pub cpu_af: u16,
    pub cpu_bc: u16,
    pub cpu_de: u16,
    pub cpu_hl: u16,
    pub cpu_sp: u16,
    pub cpu_pc: u16,
    pub ppu_frames: u64,
    pub wram_steps: u64,
    pub cartridge_steps: u64,
    pub timer_ticks: u64,
    pub selected_vram_bank: u8,
    pub selected_wram_bank: u8,
    pub selected_rom_bank: u8,
    pub selected_ram_bank: u8,
    pub framebuffer: Vec<u8>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            cpu_steps: 0,
            cpu_af: 0,
            cpu_bc: 0,
            cpu_de: 0,
            cpu_hl: 0,
            cpu_sp: 0,
            cpu_pc: 0,
            ppu_frames: 0,
            wram_steps: 0,
            cartridge_steps: 0,
            timer_ticks: 0,
            selected_vram_bank: 0,
            selected_wram_bank: 1,
            selected_rom_bank: 1,
            selected_ram_bank: 0,
            framebuffer: default_framebuffer(),
        }
    }
}

#[derive(Debug)]
pub struct PublishedFrame {
    pub frame: u64,
    pub framebuffer: Vec<u8>,
    pub published_at: Instant,
}

#[derive(Clone, Debug)]
pub struct PpuFeatures {
    pub oam: [u8; 160],
    pub lcdc: u8,
    pub stat: u8,
    pub scy: u8,
    pub scx: u8,
    pub ly: u8,
    pub wy: u8,
    pub wx: u8,
}

#[derive(Debug)]
pub enum Command {
    Memory(MemoryCommand),
    SetHardwareMode(HardwareMode),
    SaveState(std::sync::mpsc::Sender<Vec<u8>>),
    LoadState(Vec<u8>),
    RequestPpuFeatures(std::sync::mpsc::Sender<PpuFeatures>),
    Stop,
}

#[derive(Debug)]
pub enum MemoryCommand {
    Read {
        address: u16,
        respond_to: Sender<ReadResult>,
    },
    Write {
        address: u16,
        value: u8,
        respond_to: Option<Sender<WriteResult>>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadResult {
    Ready(u8),
    NoData,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteResult {
    Accepted,
    NoData,
}

#[derive(Clone, Debug)]
pub struct SharedCartridgeReadState {
    rom: Arc<[u8]>,
    boot_rom: Arc<[u8]>,
    boot_rom_mapped: Arc<AtomicBool>,
    lower_rom_bank: Arc<AtomicUsize>,
    selected_rom_bank: Arc<AtomicUsize>,
}

impl SharedCartridgeReadState {
    pub fn new(
        rom: Arc<[u8]>,
        boot_rom: Arc<[u8]>,
        lower_rom_bank: usize,
        selected_rom_bank: usize,
    ) -> Self {
        Self {
            rom,
            boot_rom,
            boot_rom_mapped: Arc::new(AtomicBool::new(true)),
            lower_rom_bank: Arc::new(AtomicUsize::new(lower_rom_bank)),
            selected_rom_bank: Arc::new(AtomicUsize::new(selected_rom_bank)),
        }
    }

    pub fn read(&self, address: u16) -> Option<u8> {
        if self.boot_rom_mapped.load(Ordering::Acquire) {
            match address {
                0x0000..=0x00ff | 0x0200..=0x08ff => {
                    return self.boot_rom.get(address as usize).copied();
                }
                _ => {}
            }
        }

        match address {
            0x0000..=0x3fff => {
                let bank = self.lower_rom_bank.load(Ordering::Acquire);
                let index = bank * SHARED_ROM_BANK_SIZE + address as usize;
                self.rom.get(index).copied()
            }
            0x4000..=0x7fff => {
                let bank = self.selected_rom_bank.load(Ordering::Acquire);
                let index = bank * SHARED_ROM_BANK_SIZE + (address as usize - SHARED_ROM_BANK_SIZE);
                self.rom.get(index).copied()
            }
            _ => None,
        }
    }

    pub fn set_boot_rom_mapped(&self, mapped: bool) {
        self.boot_rom_mapped.store(mapped, Ordering::Release);
    }

    pub fn set_lower_rom_bank(&self, bank: usize) {
        self.lower_rom_bank.store(bank, Ordering::Release);
    }

    pub fn set_selected_rom_bank(&self, bank: usize) {
        self.selected_rom_bank.store(bank, Ordering::Release);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComponentReport {
    Cpu(CpuReport),
    Ppu(PpuReport),
    Wram(WramReport),
    Cartridge(CartridgeReport),
    Timer(TimerReport),
    Apu(ApuReport),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuReport {
    pub steps: u64,
    pub registers: CpuRegisters,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PpuReport {
    pub frames: u64,
    pub selected_vram_bank: u8,
    pub framebuffer: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WramReport {
    pub steps: u64,
    pub selected_bank: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CartridgeReport {
    pub steps: u64,
    pub selected_rom_bank: u8,
    pub selected_ram_bank: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimerReport {
    pub ticks: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApuReport {
    pub samples: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CpuRegisters {
    pub af: u16,
    pub bc: u16,
    pub de: u16,
    pub hl: u16,
    pub sp: u16,
    pub pc: u16,
}

#[derive(Clone, Debug)]
pub struct CpuInitState {
    pub hardware_mode: HardwareMode,
    pub boot_target_mode: HardwareMode,
    pub registers: CpuRegisters,
    pub io_registers: [u8; IO_REGISTERS_LEN],
    pub hram: [u8; HRAM_LEN],
    pub interrupt_enable: u8,
    pub shared_cartridge: Option<SharedCartridgeReadState>,
    pub joypad: JoypadState,
    pub serial_output: Option<std::sync::mpsc::SyncSender<u8>>,
    pub interrupt_flags: InterruptFlags,
    /// Speed multiplier: 100 = 1.0x, 200 = 2.0x, 0 = uncapped.
    pub speed: std::sync::Arc<std::sync::atomic::AtomicU32>,
    pub ppu_progress: PpuProgress,
}

impl Default for CpuInitState {
    fn default() -> Self {
        Self {
            hardware_mode: HardwareMode::Cgb,
            boot_target_mode: HardwareMode::Cgb,
            registers: CpuRegisters::default(),
            io_registers: [0; IO_REGISTERS_LEN],
            hram: [0; HRAM_LEN],
            interrupt_enable: 0,
            shared_cartridge: None,
            joypad: JoypadState::new(),
            serial_output: None,
            interrupt_flags: InterruptFlags::new(),
            speed: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(100)),
            ppu_progress: PpuProgress::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WramInitState {
    pub banks: Vec<Vec<u8>>,
    pub selected_bank: u8,
    pub hardware_mode: HardwareMode,
}

impl Default for WramInitState {
    fn default() -> Self {
        Self {
            banks: vec![vec![0; WRAM_BANK_SIZE]; WRAM_BANKS],
            selected_bank: 1,
            hardware_mode: HardwareMode::Cgb,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PpuInitState {
    pub hardware_mode: HardwareMode,
    pub selected_vram_bank: u8,
    pub oam: [u8; 0x00a0],
    pub lcd_registers: [u8; 0x0c],
    pub hdma_registers: [u8; 0x05],
    pub bg_palette_index: u8,
    pub obj_palette_index: u8,
    pub bg_palette_ram: [u8; 0x40],
    pub obj_palette_ram: [u8; 0x40],
    pub framebuffer: Vec<u8>,
}

impl Default for PpuInitState {
    fn default() -> Self {
        Self {
            hardware_mode: HardwareMode::Cgb,
            selected_vram_bank: 0,
            oam: [0; 0x00a0],
            lcd_registers: [0; 0x0c],
            hdma_registers: [0; 0x05],
            bg_palette_index: 0,
            obj_palette_index: 0,
            bg_palette_ram: [0; 0x40],
            obj_palette_ram: [0; 0x40],
            framebuffer: default_framebuffer(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimerInitState {
    pub div: u8,
    pub tima: u8,
    pub tma: u8,
    pub tac: u8,
}

impl Default for TimerInitState {
    fn default() -> Self {
        Self {
            div: 0,
            tima: 0,
            tma: 0,
            tac: 0,
        }
    }
}

pub fn default_framebuffer() -> Vec<u8> {
    let mut framebuffer = vec![0; FRAMEBUFFER_SIZE];

    for y in 0..SCREEN_HEIGHT {
        for x in 0..SCREEN_WIDTH {
            let index = (y * SCREEN_WIDTH + x) * BYTES_PER_PIXEL;
            let band = (x / 16) as u8;
            let wave = ((y / 12) as u8).wrapping_mul(9);

            framebuffer[index] = band.wrapping_mul(5);
            framebuffer[index + 1] = wave;
            framebuffer[index + 2] = band ^ wave;
            framebuffer[index + 3] = 0xff;
        }
    }

    framebuffer
}
