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
#[repr(u8)]
pub enum HardwareMode {
    Cgb = 0,
    DmgCompatibility = 1,
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

/// Shared cartridge RAM state — allows the CPU to read/write cartridge RAM
/// (0xA000-0xBFFF) directly for standard MBC types (RomOnly, MBC1, MBC5, MMM01).
/// Exotic MBC types (MBC3 RTC, MBC2 4-bit, MBC7 EEPROM, etc.) fall through
/// to the channel.
#[derive(Clone, Debug)]
pub struct SharedCartridgeRamState {
    ram: Arc<SharedCartridgeRam>,
    ram_enabled: Arc<AtomicBool>,
    selected_ram_bank: Arc<AtomicUsize>,
    /// If false, all accesses must go through the channel (exotic MBC).
    standard_ram: Arc<AtomicBool>,
    ram_bank_count: usize,
    ram_size: usize,
}

struct SharedCartridgeRam {
    data: std::cell::UnsafeCell<Vec<u8>>,
}
unsafe impl Send for SharedCartridgeRam {}
unsafe impl Sync for SharedCartridgeRam {}
impl SharedCartridgeRam {
    fn new(data: Vec<u8>) -> Self { Self { data: std::cell::UnsafeCell::new(data) } }
    fn read(&self, index: usize) -> u8 {
        unsafe { *(*self.data.get()).as_ptr().add(index) }
    }
    fn write(&self, index: usize, value: u8) {
        unsafe { *(*self.data.get()).as_mut_ptr().add(index) = value }
    }
    fn as_slice(&self) -> &[u8] {
        unsafe {
            let ptr = self.data.get();
            let v = &*ptr;
            std::slice::from_raw_parts(v.as_ptr(), v.len())
        }
    }
    fn as_mut_slice(&self) -> &mut [u8] {
        unsafe {
            let ptr = self.data.get();
            let v = &mut *ptr;
            std::slice::from_raw_parts_mut(v.as_mut_ptr(), v.len())
        }
    }
}
impl std::fmt::Debug for SharedCartridgeRam {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedCartridgeRam").finish_non_exhaustive()
    }
}

impl SharedCartridgeRamState {
    pub fn new(ram: Vec<u8>, ram_bank_count: usize, ram_size: usize, standard: bool) -> Self {
        Self {
            ram: Arc::new(SharedCartridgeRam::new(ram)),
            ram_enabled: Arc::new(AtomicBool::new(false)),
            selected_ram_bank: Arc::new(AtomicUsize::new(0)),
            standard_ram: Arc::new(AtomicBool::new(standard)),
            ram_bank_count,
            ram_size,
        }
    }

    pub fn read(&self, address: u16) -> Option<u8> {
        if !self.standard_ram.load(Ordering::Acquire) {
            return None; // Exotic MBC — fall through to channel
        }
        if !self.ram_enabled.load(Ordering::Acquire) {
            return Some(0xff); // RAM disabled — returns open bus
        }
        self.ram_index(address).map(|i| self.ram.read(i))
    }

    pub fn write(&self, address: u16, value: u8) -> bool {
        if !self.standard_ram.load(Ordering::Acquire) {
            return false; // Exotic MBC — fall through
        }
        if !self.ram_enabled.load(Ordering::Acquire) {
            return true; // RAM disabled — accept but ignore
        }
        if let Some(i) = self.ram_index(address) {
            self.ram.write(i, value);
        }
        true
    }

    fn ram_index(&self, address: u16) -> Option<usize> {
        if self.ram_bank_count == 0 || self.ram_size == 0 {
            return None;
        }
        let bank_size = self.ram_size / self.ram_bank_count;
        let offset = (address - 0xa000) as usize;
        if offset >= bank_size {
            return None;
        }
        let bank = self.selected_ram_bank.load(Ordering::Acquire) % self.ram_bank_count;
        Some(bank * bank_size + offset)
    }

    pub fn set_ram_enabled(&self, enabled: bool) {
        self.ram_enabled.store(enabled, Ordering::Release);
    }

    pub fn set_selected_ram_bank(&self, bank: usize) {
        self.selected_ram_bank.store(bank, Ordering::Release);
    }

    pub fn set_standard_ram(&self, standard: bool) {
        self.standard_ram.store(standard, Ordering::Release);
    }

    pub fn ram_slice(&self) -> &[u8] { self.ram.as_slice() }
    pub fn ram_mut_slice(&self) -> &mut [u8] { self.ram.as_mut_slice() }
}

/// Shared WRAM backing store — allows the CPU thread to read/write WRAM
/// directly without channel round-trips.  Both the CPU and the WRAM thread
/// hold a clone (the inner data is `Arc`-wrapped).
///
/// Safety model: the Game Boy bus is single-initiator (only the CPU issues
/// memory accesses), so there are no concurrent reads+writes to the same byte.
/// The WRAM thread only touches this data during save/load state (while the
/// emulator is paused).  We use `UnsafeCell` for zero-cost access and rely on
/// the architectural guarantee that accesses are serialized.
#[derive(Clone, Debug)]
pub struct SharedWramState {
    banks: Arc<WramBanks>,
    selected_bank: Arc<AtomicU8>,
    hardware_mode: Arc<AtomicU8>,
}

impl SharedWramState {
    pub fn new(banks: Vec<Vec<u8>>, selected_bank: u8, hardware_mode: HardwareMode) -> Self {
        let mut flat = [0u8; WRAM_BANKS * WRAM_BANK_SIZE];
        for (i, bank) in banks.iter().enumerate() {
            let start = i * WRAM_BANK_SIZE;
            let end = start + bank.len().min(WRAM_BANK_SIZE);
            flat[start..end].copy_from_slice(&bank[..end - start]);
        }
        Self {
            banks: Arc::new(WramBanks::new(flat)),
            selected_bank: Arc::new(AtomicU8::new(selected_bank)),
            hardware_mode: Arc::new(AtomicU8::new(hardware_mode as u8)),
        }
    }

    pub fn read(&self, address: u16) -> Option<u8> {
        match address {
            0xff70 => {
                if self.is_dmg_compat() {
                    Some(0xff)
                } else {
                    Some(0xf8 | self.selected_bank.load(Ordering::Acquire))
                }
            }
            _ => self.translate(address).map(|offset| self.banks.read(offset)),
        }
    }

    pub fn write(&self, address: u16, value: u8) -> bool {
        match address {
            0xff70 => {
                if !self.is_dmg_compat() {
                    let bank = select_wram_bank(value);
                    self.selected_bank.store(bank, Ordering::Release);
                }
                true
            }
            _ => match self.translate(address) {
                Some(offset) => {
                    self.banks.write(offset, value);
                    true
                }
                None => false,
            },
        }
    }

    /// Bulk-read all banks into owned Vecs (for save state serialization).
    pub fn snapshot_banks(&self) -> Vec<Vec<u8>> {
        (0..WRAM_BANKS)
            .map(|i| {
                let start = i * WRAM_BANK_SIZE;
                let mut v = vec![0u8; WRAM_BANK_SIZE];
                for (j, byte) in v.iter_mut().enumerate() {
                    *byte = self.banks.read(start + j);
                }
                v
            })
            .collect()
    }

    /// Bulk-write all banks from save state data.
    pub fn restore_banks(&self, banks: &[Vec<u8>]) {
        for (i, bank) in banks.iter().enumerate() {
            let start = i * WRAM_BANK_SIZE;
            for (j, &byte) in bank.iter().enumerate() {
                if start + j < WRAM_BANKS * WRAM_BANK_SIZE {
                    self.banks.write(start + j, byte);
                }
            }
        }
    }

    pub fn selected_bank(&self) -> u8 {
        self.selected_bank.load(Ordering::Acquire)
    }

    pub fn set_selected_bank(&self, bank: u8) {
        self.selected_bank.store(bank, Ordering::Release);
    }

    pub fn set_hardware_mode(&self, mode: HardwareMode) {
        self.hardware_mode.store(mode as u8, Ordering::Release);
    }

    fn is_dmg_compat(&self) -> bool {
        self.hardware_mode.load(Ordering::Acquire) == HardwareMode::DmgCompatibility as u8
    }

    fn translate(&self, address: u16) -> Option<usize> {
        let bank = self.selected_bank.load(Ordering::Acquire) as usize;
        match address {
            0xc000..=0xcfff => Some((address - 0xc000) as usize),
            0xd000..=0xdfff => Some(bank * WRAM_BANK_SIZE + (address - 0xd000) as usize),
            0xe000..=0xefff => Some((address - 0xe000) as usize),
            0xf000..=0xfdff => Some(bank * WRAM_BANK_SIZE + (address - 0xf000) as usize),
            _ => None,
        }
    }
}

pub fn select_wram_bank(value: u8) -> u8 {
    match value & 0x07 {
        0 => 1,
        selected => selected,
    }
}

/// Flat byte array backing WRAM, shared between threads via `Arc`.
/// Uses `UnsafeCell` for zero-overhead access; synchronization is
/// guaranteed by the emulator's single-bus architecture (only the CPU
/// initiates memory accesses; WRAM thread touches this only while paused).
struct WramBanks {
    data: std::cell::UnsafeCell<[u8; WRAM_BANKS * WRAM_BANK_SIZE]>,
}

// SAFETY: Access is serialized by the emulator's bus protocol — the CPU is the
// only thread that reads/writes during normal execution, and save/load state
// only runs while the CPU is paused.
unsafe impl Send for WramBanks {}
unsafe impl Sync for WramBanks {}

impl WramBanks {
    fn new(data: [u8; WRAM_BANKS * WRAM_BANK_SIZE]) -> Self {
        Self {
            data: std::cell::UnsafeCell::new(data),
        }
    }

    fn read(&self, offset: usize) -> u8 {
        // SAFETY: see Send/Sync impl justification above.
        unsafe { (*self.data.get())[offset] }
    }

    fn write(&self, offset: usize, value: u8) {
        // SAFETY: see Send/Sync impl justification above.
        unsafe { (*self.data.get())[offset] = value }
    }
}

impl std::fmt::Debug for WramBanks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WramBanks").finish_non_exhaustive()
    }
}

/// Shared timer register state — allows the CPU to read timer registers
/// directly without channel round-trips.  The timer thread updates these
/// atomics after every step and on save/load state.
///
/// CPU reads spin briefly on `cycles` to ensure the timer has caught up
/// to the current clock position before reading register values.
/// Writes still go through the channel (side effects: falling edge, reload).
#[derive(Clone, Debug)]
pub struct SharedTimerState {
    pub cycles: Arc<AtomicU64>,
    div: Arc<AtomicU8>,
    tima: Arc<AtomicU8>,
    tma: Arc<AtomicU8>,
    tac: Arc<AtomicU8>,
}

impl SharedTimerState {
    pub fn new() -> Self {
        Self {
            cycles: Arc::new(AtomicU64::new(0)),
            div: Arc::new(AtomicU8::new(0)),
            tima: Arc::new(AtomicU8::new(0)),
            tma: Arc::new(AtomicU8::new(0)),
            tac: Arc::new(AtomicU8::new(0)),
        }
    }

    /// Read a timer register.  The caller must ensure the timer is caught up
    /// (by spinning on `cycles`) before calling this.
    pub fn read(&self, address: u16) -> Option<u8> {
        match address {
            0xff04 => Some(self.div.load(Ordering::Acquire)),
            0xff05 => Some(self.tima.load(Ordering::Acquire)),
            0xff06 => Some(self.tma.load(Ordering::Acquire)),
            0xff07 => Some(0xf8 | self.tac.load(Ordering::Acquire)),
            _ => None,
        }
    }

    /// Called by the timer thread after every step / state change.
    pub fn publish(&self, div: u8, tima: u8, tma: u8, tac: u8, cycles: u64) {
        self.div.store(div, Ordering::Release);
        self.tima.store(tima, Ordering::Release);
        self.tma.store(tma, Ordering::Release);
        self.tac.store(tac, Ordering::Release);
        // cycles must be stored last — it's the release fence the CPU spins on.
        self.cycles.store(cycles, Ordering::Release);
    }
}

/// Number of APU registers (0xFF10-0xFF26 inclusive).
const APU_REGISTER_COUNT: usize = 0x17;

/// Read masks for APU registers — bits that always read as 1.
const APU_READ_MASKS: [u8; APU_REGISTER_COUNT] = [
    0x80, // FF10 NR10
    0x3f, // FF11 NR11
    0x00, // FF12 NR12
    0xff, // FF13 NR13 (write-only)
    0xbf, // FF14 NR14
    0xff, // FF15 unused
    0x3f, // FF16 NR21
    0x00, // FF17 NR22
    0xff, // FF18 NR23 (write-only)
    0xbf, // FF19 NR24
    0x7f, // FF1A NR30
    0xff, // FF1B NR31 (write-only)
    0x9f, // FF1C NR32
    0xff, // FF1D NR33 (write-only)
    0xbf, // FF1E NR34
    0xff, // FF1F unused
    0xff, // FF20 NR41 (write-only)
    0x00, // FF21 NR42
    0x00, // FF22 NR43
    0xbf, // FF23 NR44
    0x00, // FF24 NR50
    0x00, // FF25 NR51
    0x70, // FF26 NR52 (bits 6-4 unused)
];

/// Shared APU register state — allows the CPU to read APU registers
/// (0xFF10-0xFF26) without channel round-trips.  Register reads just return
/// the stored write value OR'd with the read mask.  The APU thread updates
/// these on every register write and on power-off.
///
/// NR52 (0xFF26) is computed from a shared status byte.
/// Wave RAM (0xFF30-0xFF3F) when CH3 is active stays on the channel
/// (complex timing interaction).
const SHARED_WAVE_RAM_LEN: usize = 16;

#[derive(Clone, Debug)]
pub struct SharedApuState {
    registers: Arc<ApuRegisters>,
    /// NR52 status: bit 7 = master_enable, bits 0-3 = channel enabled flags.
    nr52_status: Arc<AtomicU8>,
    wave_ram: Arc<SharedWaveRam>,
    /// Channel 3 enabled — when true, wave RAM reads have timing interactions
    /// and must go through the channel.
    ch3_enabled: Arc<AtomicBool>,
}

struct SharedWaveRam {
    data: std::cell::UnsafeCell<[u8; SHARED_WAVE_RAM_LEN]>,
}
unsafe impl Send for SharedWaveRam {}
unsafe impl Sync for SharedWaveRam {}
impl SharedWaveRam {
    fn new() -> Self { Self { data: std::cell::UnsafeCell::new([0; SHARED_WAVE_RAM_LEN]) } }
    fn read(&self, index: usize) -> u8 { unsafe { (*self.data.get())[index] } }
    fn write(&self, index: usize, value: u8) { unsafe { (*self.data.get())[index] = value } }
    fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.data.get() as *const u8, SHARED_WAVE_RAM_LEN) }
    }
    fn as_mut_slice(&self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.data.get() as *mut u8, SHARED_WAVE_RAM_LEN) }
    }
}
impl std::fmt::Debug for SharedWaveRam {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedWaveRam").finish_non_exhaustive()
    }
}

/// Shared APU register backing store.
struct ApuRegisters {
    data: std::cell::UnsafeCell<[u8; APU_REGISTER_COUNT]>,
}

// SAFETY: Register writes are serialized through the APU channel; the CPU
// only reads.  Power-off zeroes all registers atomically from the APU thread.
unsafe impl Send for ApuRegisters {}
unsafe impl Sync for ApuRegisters {}

impl ApuRegisters {
    fn new() -> Self {
        Self {
            data: std::cell::UnsafeCell::new([0; APU_REGISTER_COUNT]),
        }
    }

    fn read(&self, index: usize) -> u8 {
        unsafe { (*self.data.get())[index] }
    }

    fn write(&self, index: usize, value: u8) {
        unsafe { (*self.data.get())[index] = value }
    }

    fn zero_all(&self) {
        unsafe {
            for byte in (*self.data.get()).iter_mut() {
                *byte = 0;
            }
        }
    }
}

impl std::fmt::Debug for ApuRegisters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApuRegisters").finish_non_exhaustive()
    }
}

impl SharedApuState {
    pub fn new() -> Self {
        Self {
            registers: Arc::new(ApuRegisters::new()),
            nr52_status: Arc::new(AtomicU8::new(0)),
            wave_ram: Arc::new(SharedWaveRam::new()),
            ch3_enabled: Arc::new(AtomicBool::new(false)),
        }
    }

    /// CPU-side read.  Returns Some for 0xFF10-0xFF3F, None for others.
    pub fn read(&self, address: u16) -> Option<u8> {
        match address {
            0xff10..=0xff25 => {
                let index = (address - 0xff10) as usize;
                Some(self.registers.read(index) | APU_READ_MASKS[index])
            }
            0xff26 => {
                let status = self.nr52_status.load(Ordering::Acquire);
                Some(0x70 | status)
            }
            0xff27..=0xff2f => Some(0xff),
            0xff30..=0xff3f => {
                // Wave RAM: direct access only when CH3 is disabled.
                // When enabled, timing interactions require the channel path.
                if self.ch3_enabled.load(Ordering::Acquire) {
                    None // Fall through to channel
                } else {
                    Some(self.wave_ram.read((address - 0xff30) as usize))
                }
            }
            _ => None,
        }
    }

    /// Called by the APU thread when a register is written.
    pub fn set_register(&self, index: usize, value: u8) {
        if index < APU_REGISTER_COUNT {
            self.registers.write(index, value);
        }
    }

    /// Called by the APU thread when registers are zeroed (power off).
    pub fn zero_registers(&self) {
        self.registers.zero_all();
    }

    /// Called by the APU thread when NR52 status changes.
    /// `status` should be: bit 7 = master_enable, bits 0-3 = ch1-ch4 enabled.
    pub fn set_nr52_status(&self, status: u8) {
        self.nr52_status.store(status, Ordering::Release);
    }

    pub fn set_ch3_enabled(&self, enabled: bool) {
        self.ch3_enabled.store(enabled, Ordering::Release);
    }

    pub fn wave_ram_slice(&self) -> &[u8] {
        self.wave_ram.as_slice()
    }

    pub fn wave_ram_mut_slice(&self) -> &mut [u8] {
        self.wave_ram.as_mut_slice()
    }

    pub fn set_wave_ram(&self, index: usize, value: u8) {
        self.wave_ram.write(index, value);
    }
}

// PPU shared memory sizes.
const SHARED_VRAM_BANK_SIZE: usize = 0x2000; // 8 KB per bank
const SHARED_VRAM_BANKS: usize = 2;
const SHARED_OAM_SIZE: usize = 0x00a0; // 160 bytes
const SHARED_LCD_REGISTERS_LEN: usize = 0x0c; // 12 registers

/// Shared PPU state — allows the CPU to read VRAM, OAM, and LCD registers
/// directly without channel round-trips.  The PPU thread updates gating
/// atomics (mode, lcd_enabled, oam_dma_active) on every state transition.
///
/// VRAM reads: blocked when mode == 3 (returns 0xFF).
/// OAM reads: blocked when mode >= 2 or OAM DMA active (returns 0xFF).
/// LCD register reads: always allowed.
/// LCD register writes: stay on channel (side effects).
const SHARED_PALETTE_RAM_LEN: usize = 64;
const SHARED_HDMA_REGISTERS_LEN: usize = 5;

#[derive(Clone, Debug)]
pub struct SharedPpuState {
    vram: Arc<SharedVram>,
    oam: Arc<SharedOam>,
    lcd_registers: Arc<SharedLcdRegisters>,
    selected_vram_bank: Arc<AtomicU8>,
    /// Current PPU mode (0-3).
    mode: Arc<AtomicU8>,
    lcd_enabled: Arc<AtomicBool>,
    oam_dma_active: Arc<AtomicBool>,
    hardware_mode: Arc<AtomicU8>,
    // Palette state (CGB).
    bg_palette_index: Arc<AtomicU8>,
    obj_palette_index: Arc<AtomicU8>,
    bg_palette_ram: Arc<SharedPaletteRam>,
    obj_palette_ram: Arc<SharedPaletteRam>,
    // HDMA registers (CGB).
    hdma_registers: Arc<SharedHdmaRegisters>,
    /// 0xFF if no HDMA active, otherwise remaining_blocks - 1.
    hdma_status: Arc<AtomicU8>,
}

struct SharedVram {
    data: std::cell::UnsafeCell<[u8; SHARED_VRAM_BANKS * SHARED_VRAM_BANK_SIZE]>,
}
unsafe impl Send for SharedVram {}
unsafe impl Sync for SharedVram {}

impl SharedVram {
    fn new() -> Self {
        Self { data: std::cell::UnsafeCell::new([0; SHARED_VRAM_BANKS * SHARED_VRAM_BANK_SIZE]) }
    }
    fn read(&self, offset: usize) -> u8 {
        unsafe { (*self.data.get())[offset] }
    }
    fn write(&self, offset: usize, value: u8) {
        unsafe { (*self.data.get())[offset] = value }
    }
    fn bank_slice(&self, bank: usize) -> &[u8] {
        let start = bank * SHARED_VRAM_BANK_SIZE;
        unsafe {
            let ptr = self.data.get() as *const u8;
            std::slice::from_raw_parts(ptr.add(start), SHARED_VRAM_BANK_SIZE)
        }
    }
    fn bank_slice_mut(&self, bank: usize) -> &mut [u8] {
        let start = bank * SHARED_VRAM_BANK_SIZE;
        unsafe {
            let ptr = self.data.get() as *mut u8;
            std::slice::from_raw_parts_mut(ptr.add(start), SHARED_VRAM_BANK_SIZE)
        }
    }
}
impl std::fmt::Debug for SharedVram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedVram").finish_non_exhaustive()
    }
}

struct SharedOam {
    data: std::cell::UnsafeCell<[u8; SHARED_OAM_SIZE]>,
}
unsafe impl Send for SharedOam {}
unsafe impl Sync for SharedOam {}

impl SharedOam {
    fn new() -> Self {
        Self { data: std::cell::UnsafeCell::new([0; SHARED_OAM_SIZE]) }
    }
    fn read(&self, offset: usize) -> u8 {
        unsafe { (*self.data.get())[offset] }
    }
    fn write(&self, offset: usize, value: u8) {
        unsafe { (*self.data.get())[offset] = value }
    }
    fn as_slice(&self) -> &[u8] {
        unsafe {
            let ptr = self.data.get() as *const u8;
            std::slice::from_raw_parts(ptr, SHARED_OAM_SIZE)
        }
    }
    fn as_mut_slice(&self) -> &mut [u8] {
        unsafe {
            let ptr = self.data.get() as *mut u8;
            std::slice::from_raw_parts_mut(ptr, SHARED_OAM_SIZE)
        }
    }
}
impl std::fmt::Debug for SharedOam {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedOam").finish_non_exhaustive()
    }
}

struct SharedLcdRegisters {
    data: std::cell::UnsafeCell<[u8; SHARED_LCD_REGISTERS_LEN]>,
}
unsafe impl Send for SharedLcdRegisters {}
unsafe impl Sync for SharedLcdRegisters {}

impl SharedLcdRegisters {
    fn new() -> Self {
        Self { data: std::cell::UnsafeCell::new([0; SHARED_LCD_REGISTERS_LEN]) }
    }
    fn read(&self, index: usize) -> u8 {
        unsafe { (*self.data.get())[index] }
    }
    fn write(&self, index: usize, value: u8) {
        unsafe { (*self.data.get())[index] = value }
    }
}
impl std::fmt::Debug for SharedLcdRegisters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedLcdRegisters").finish_non_exhaustive()
    }
}

struct SharedPaletteRam {
    data: std::cell::UnsafeCell<[u8; SHARED_PALETTE_RAM_LEN]>,
}
unsafe impl Send for SharedPaletteRam {}
unsafe impl Sync for SharedPaletteRam {}
impl SharedPaletteRam {
    fn new() -> Self { Self { data: std::cell::UnsafeCell::new([0; SHARED_PALETTE_RAM_LEN]) } }
    fn read(&self, index: usize) -> u8 { unsafe { (*self.data.get())[index] } }
    fn write(&self, index: usize, value: u8) { unsafe { (*self.data.get())[index] = value } }
    fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.data.get() as *const u8, SHARED_PALETTE_RAM_LEN) }
    }
    fn as_mut_slice(&self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.data.get() as *mut u8, SHARED_PALETTE_RAM_LEN) }
    }
}
impl std::fmt::Debug for SharedPaletteRam {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedPaletteRam").finish_non_exhaustive()
    }
}

struct SharedHdmaRegisters {
    data: std::cell::UnsafeCell<[u8; SHARED_HDMA_REGISTERS_LEN]>,
}
unsafe impl Send for SharedHdmaRegisters {}
unsafe impl Sync for SharedHdmaRegisters {}
impl SharedHdmaRegisters {
    fn new() -> Self { Self { data: std::cell::UnsafeCell::new([0; SHARED_HDMA_REGISTERS_LEN]) } }
    fn read(&self, index: usize) -> u8 { unsafe { (*self.data.get())[index] } }
    fn write(&self, index: usize, value: u8) { unsafe { (*self.data.get())[index] = value } }
}
impl std::fmt::Debug for SharedHdmaRegisters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedHdmaRegisters").finish_non_exhaustive()
    }
}

impl SharedPpuState {
    pub fn new() -> Self {
        Self {
            vram: Arc::new(SharedVram::new()),
            oam: Arc::new(SharedOam::new()),
            lcd_registers: Arc::new(SharedLcdRegisters::new()),
            selected_vram_bank: Arc::new(AtomicU8::new(0)),
            mode: Arc::new(AtomicU8::new(0)),
            lcd_enabled: Arc::new(AtomicBool::new(false)),
            oam_dma_active: Arc::new(AtomicBool::new(false)),
            hardware_mode: Arc::new(AtomicU8::new(0)),
            bg_palette_index: Arc::new(AtomicU8::new(0)),
            obj_palette_index: Arc::new(AtomicU8::new(0)),
            bg_palette_ram: Arc::new(SharedPaletteRam::new()),
            obj_palette_ram: Arc::new(SharedPaletteRam::new()),
            hdma_registers: Arc::new(SharedHdmaRegisters::new()),
            hdma_status: Arc::new(AtomicU8::new(0xff)),
        }
    }

    // --- CPU-side reads ---

    pub fn read(&self, address: u16) -> Option<ReadResult> {
        let lcd_on = self.lcd_enabled.load(Ordering::Acquire);
        let mode = if lcd_on { self.mode.load(Ordering::Acquire) } else { 0 };

        match address {
            0x8000..=0x9fff => {
                if mode == 3 {
                    Some(ReadResult::Ready(0xff))
                } else {
                    let bank = self.selected_vram_bank.load(Ordering::Acquire) as usize;
                    let offset = bank * SHARED_VRAM_BANK_SIZE + (address - 0x8000) as usize;
                    Some(ReadResult::Ready(self.vram.read(offset)))
                }
            }
            0xfe00..=0xfe9f => {
                if self.oam_dma_active.load(Ordering::Acquire) || mode >= 2 {
                    Some(ReadResult::Ready(0xff))
                } else {
                    Some(ReadResult::Ready(self.oam.read((address - 0xfe00) as usize)))
                }
            }
            0xff40..=0xff4b => {
                Some(ReadResult::Ready(self.lcd_registers.read((address - 0xff40) as usize)))
            }
            0xff4f => {
                if self.is_dmg_compat() {
                    Some(ReadResult::Ready(0xff))
                } else {
                    Some(ReadResult::Ready(0xfe | self.selected_vram_bank.load(Ordering::Acquire)))
                }
            }
            0xff51..=0xff55 if self.is_dmg_compat() => Some(ReadResult::Ready(0xff)),
            0xff51..=0xff54 => {
                Some(ReadResult::Ready(self.hdma_registers.read((address - 0xff51) as usize)))
            }
            0xff55 => Some(ReadResult::Ready(self.hdma_status.load(Ordering::Acquire))),
            0xff68..=0xff6b if self.is_dmg_compat() => Some(ReadResult::Ready(0xff)),
            0xff68 => Some(ReadResult::Ready(self.bg_palette_index.load(Ordering::Acquire))),
            0xff69 => {
                let idx = (self.bg_palette_index.load(Ordering::Acquire) & 0x3f) as usize;
                Some(ReadResult::Ready(self.bg_palette_ram.read(idx)))
            }
            0xff6a => Some(ReadResult::Ready(self.obj_palette_index.load(Ordering::Acquire))),
            0xff6b => {
                let idx = (self.obj_palette_index.load(Ordering::Acquire) & 0x3f) as usize;
                Some(ReadResult::Ready(self.obj_palette_ram.read(idx)))
            }
            _ => None,
        }
    }

    /// CPU-side write.  Returns true if the write was handled, false if it
    /// should fall through to the channel.
    pub fn write(&self, address: u16, value: u8) -> bool {
        let lcd_on = self.lcd_enabled.load(Ordering::Acquire);
        let mode = if lcd_on { self.mode.load(Ordering::Acquire) } else { 0 };

        match address {
            0x8000..=0x9fff => {
                if mode != 3 {
                    let bank = self.selected_vram_bank.load(Ordering::Acquire) as usize;
                    let offset = bank * SHARED_VRAM_BANK_SIZE + (address - 0x8000) as usize;
                    self.vram.write(offset, value);
                }
                true // Always "accepted" — matching PPU behavior
            }
            0xfe00..=0xfe9f => {
                if !self.oam_dma_active.load(Ordering::Acquire) && mode < 2 {
                    self.oam.write((address - 0xfe00) as usize, value);
                }
                true
            }
            // Side-effect-free LCD register writes: scroll, palettes, window pos.
            0xff42 | 0xff43 | 0xff47..=0xff4b => {
                self.lcd_registers.write((address - 0xff40) as usize, value);
                true
            }
            // VRAM bank select (CGB).
            0xff4f if !self.is_dmg_compat() => {
                self.selected_vram_bank.store(value & 0x01, Ordering::Release);
                true
            }
            0xff4f => true, // DMG compat: ignore write
            _ => false, // LCDC, STAT, LY, LYC, DMA, palette, HDMA writes → channel
        }
    }

    // --- PPU-thread-side accessors ---

    pub fn vram_bank_slice(&self, bank: usize) -> &[u8] {
        self.vram.bank_slice(bank)
    }

    pub fn vram_bank_slice_mut(&self, bank: usize) -> &mut [u8] {
        self.vram.bank_slice_mut(bank)
    }

    pub fn oam_slice(&self) -> &[u8] {
        self.oam.as_slice()
    }

    pub fn oam_mut_slice(&self) -> &mut [u8] {
        self.oam.as_mut_slice()
    }

    pub fn lcd_reg_read(&self, index: usize) -> u8 {
        self.lcd_registers.read(index)
    }

    pub fn lcd_reg_write(&self, index: usize, value: u8) {
        self.lcd_registers.write(index, value);
    }

    pub fn set_mode(&self, mode: u8) {
        self.mode.store(mode, Ordering::Release);
    }

    pub fn set_lcd_enabled(&self, enabled: bool) {
        self.lcd_enabled.store(enabled, Ordering::Release);
    }

    pub fn set_oam_dma_active(&self, active: bool) {
        self.oam_dma_active.store(active, Ordering::Release);
    }

    pub fn set_selected_vram_bank(&self, bank: u8) {
        self.selected_vram_bank.store(bank, Ordering::Release);
    }

    pub fn set_hardware_mode(&self, mode: HardwareMode) {
        self.hardware_mode.store(mode as u8, Ordering::Release);
    }

    pub fn set_bg_palette_index(&self, index: u8) {
        self.bg_palette_index.store(index, Ordering::Release);
    }

    pub fn set_obj_palette_index(&self, index: u8) {
        self.obj_palette_index.store(index, Ordering::Release);
    }

    pub fn bg_palette_slice(&self) -> &[u8] {
        self.bg_palette_ram.as_slice()
    }

    pub fn bg_palette_mut_slice(&self) -> &mut [u8] {
        self.bg_palette_ram.as_mut_slice()
    }

    pub fn obj_palette_slice(&self) -> &[u8] {
        self.obj_palette_ram.as_slice()
    }

    pub fn obj_palette_mut_slice(&self) -> &mut [u8] {
        self.obj_palette_ram.as_mut_slice()
    }

    pub fn set_hdma_register(&self, index: usize, value: u8) {
        self.hdma_registers.write(index, value);
    }

    pub fn set_hdma_status(&self, status: u8) {
        self.hdma_status.store(status, Ordering::Release);
    }

    fn is_dmg_compat(&self) -> bool {
        self.hardware_mode.load(Ordering::Acquire) == HardwareMode::DmgCompatibility as u8
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
    pub shared_cartridge_ram: Option<SharedCartridgeRamState>,
    pub shared_wram: Option<SharedWramState>,
    pub shared_timer: Option<SharedTimerState>,
    pub shared_apu: Option<SharedApuState>,
    pub shared_ppu: Option<SharedPpuState>,
    pub joypad: JoypadState,
    pub serial_output: Option<std::sync::mpsc::SyncSender<u8>>,
    pub interrupt_flags: InterruptFlags,
    /// Speed multiplier: 100 = 1.0x, 200 = 2.0x, 0 = uncapped.
    pub speed: std::sync::Arc<std::sync::atomic::AtomicU32>,
    pub paused: std::sync::Arc<AtomicBool>,
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
            shared_cartridge_ram: None,
            shared_wram: None,
            shared_timer: None,
            shared_apu: None,
            shared_ppu: None,
            joypad: JoypadState::new(),
            serial_output: None,
            interrupt_flags: InterruptFlags::new(),
            speed: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(100)),
            paused: std::sync::Arc::new(AtomicBool::new(false)),
            ppu_progress: PpuProgress::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct WramInitState {
    pub banks: Vec<Vec<u8>>,
    pub selected_bank: u8,
    pub hardware_mode: HardwareMode,
    pub shared: Option<SharedWramState>,
}

impl Default for WramInitState {
    fn default() -> Self {
        Self {
            banks: vec![vec![0; WRAM_BANK_SIZE]; WRAM_BANKS],
            selected_bank: 1,
            hardware_mode: HardwareMode::Cgb,
            shared: None,
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
