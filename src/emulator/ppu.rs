use std::sync::mpsc::{Receiver, Sender, SyncSender, TryRecvError, TrySendError};
use std::thread;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use super::bus::{Bus, PendingRead};
use super::component::{
    BYTES_PER_PIXEL, Command, ComponentReport, FRAMEBUFFER_SIZE, HardwareMode, InterruptFlags,
    MasterClock, MemoryCommand, PpuInitState, PpuReport, PublishedFrame, ReadResult, SCREEN_HEIGHT,
    SCREEN_WIDTH, WriteResult,
};

#[derive(Serialize, Deserialize)]
pub struct PpuSaveState {
    pub vram_banks: Vec<Vec<u8>>,
    pub selected_vram_bank: u8,
    pub oam: Vec<u8>,
    pub lcd_registers: Vec<u8>,
    pub hdma_registers: Vec<u8>,
    pub bg_palette_index: u8,
    pub obj_palette_index: u8,
    pub bg_palette_ram: Vec<u8>,
    pub obj_palette_ram: Vec<u8>,
    pub dots_into_line: u16,
    pub stat_interrupt_line: bool,
    pub stat_interrupt_armed: bool,
    pub window_line: u8,
    pub dots: u64,
    pub frames: u64,
    pub hardware_mode: HardwareMode,
}

const VRAM_BANK_SIZE: usize = 0x2000;
const VRAM_BANKS: usize = 2;
const OAM_SIZE: usize = 0x00a0;
const OAM_ENTRY_SIZE: usize = 4;
const MAX_SPRITES: usize = 40;
const OAM_DMA_BYTES_PER_SAMPLE: usize = 8;
const LCD_REGISTERS_LEN: usize = 0x0c;
const HDMA_REGISTERS_LEN: usize = 0x05;
const PALETTE_RAM_LEN: usize = 0x40;
const TILE_BYTES: usize = 16;
const BG_MAP_OFFSET_9800: usize = 0x1800;
const BG_MAP_OFFSET_9C00: usize = 0x1c00;
const SIGNED_TILE_DATA_BASE: i32 = 0x1000;
const UNSIGNED_TILE_DATA_BASE: usize = 0x0000;
const DOTS_PER_LINE: u16 = 456;
const VISIBLE_LINES: u8 = 144;
const TOTAL_LINES: u8 = 154;
const MODE2_DOTS: u16 = 80;
const MODE3_DOTS: u16 = 172;
const PPU_DOTS_PER_SAMPLE: usize = 32;
const STAT_MODE_FLAG_MASK: u8 = 0x03;
const STAT_LYC_FLAG_MASK: u8 = 0x04;
const STAT_MODE0_INTERRUPT_MASK: u8 = 0x08;
const STAT_MODE1_INTERRUPT_MASK: u8 = 0x10;
const STAT_MODE2_INTERRUPT_MASK: u8 = 0x20;
const STAT_LYC_INTERRUPT_MASK: u8 = 0x40;
const STAT_UNUSED_MASK: u8 = 0x80;
const LCDC_ENABLE_MASK: u8 = 0x80;
const VBLANK_INTERRUPT_MASK: u8 = 0x01;
const STAT_INTERRUPT_MASK: u8 = 0x02;
const LCDC_INDEX: usize = 0x00;
const STAT_INDEX: usize = 0x01;
const SCY_INDEX: usize = 0x02;
const SCX_INDEX: usize = 0x03;
const LY_INDEX: usize = 0x04;
const LYC_INDEX: usize = 0x05;
const BGP_INDEX: usize = 0x07;
const OBP0_INDEX: usize = 0x08;
const OBP1_INDEX: usize = 0x09;
const WY_INDEX: usize = 0x0a;
const WX_INDEX: usize = 0x0b;
const LCDC_BG_ENABLE_MASK: u8 = 0x01;
const LCDC_OBJ_ENABLE_MASK: u8 = 0x02;
const LCDC_OBJ_SIZE_MASK: u8 = 0x04;
const LCDC_BG_TILE_MAP_MASK: u8 = 0x08;
const LCDC_TILE_DATA_MASK: u8 = 0x10;
const LCDC_WINDOW_ENABLE_MASK: u8 = 0x20;
const LCDC_WINDOW_TILE_MAP_MASK: u8 = 0x40;
const SPRITE_ATTR_PRIORITY_MASK: u8 = 0x80;
const SPRITE_ATTR_Y_FLIP_MASK: u8 = 0x40;
const SPRITE_ATTR_X_FLIP_MASK: u8 = 0x20;
const SPRITE_ATTR_DMG_PALETTE_MASK: u8 = 0x10;
const SPRITE_ATTR_BANK_MASK: u8 = 0x08;
const CGB_BG_ATTR_Y_FLIP_MASK: u8 = 0x40;
const CGB_BG_ATTR_X_FLIP_MASK: u8 = 0x20;
const CGB_BG_ATTR_BANK_MASK: u8 = 0x08;
const CGB_BG_ATTR_PALETTE_MASK: u8 = 0x07;
const CGB_BG_ATTR_PRIORITY_MASK: u8 = 0x80;
const CGB_OBJ_ATTR_PALETTE_MASK: u8 = 0x07;

#[derive(Debug)]
pub struct PpuThread {
    bus: Bus,
    clock: MasterClock,
    interrupt_flags: InterruptFlags,
    dots: u64,
    hardware_mode: HardwareMode,
    vram_banks: Vec<Vec<u8>>,
    selected_vram_bank: u8,
    oam: [u8; OAM_SIZE],
    lcd_registers: [u8; LCD_REGISTERS_LEN],
    hdma_registers: [u8; HDMA_REGISTERS_LEN],
    bg_palette_index: u8,
    obj_palette_index: u8,
    bg_palette_ram: [u8; PALETTE_RAM_LEN],
    obj_palette_ram: [u8; PALETTE_RAM_LEN],
    framebuffer: Vec<u8>,
    framebuffer_spare: Option<Vec<u8>>,
    pending_published_frame: Option<PublishedFrame>,
    frame_ready: SyncSender<PublishedFrame>,
    frame_recycle: Receiver<Vec<u8>>,
    frames: u64,
    last_published_at: Option<Instant>,
    dots_into_line: u16,
    stat_interrupt_line: bool,
    stat_interrupt_armed: bool,
    report_dirty: bool,
    framebuffer_dirty: bool,
    frame_publication_pending: bool,
    report_framebuffer_pending: bool,
    oam_dma: Option<OamDmaTransfer>,
    hdma: Option<HdmaTransfer>,
    scanline_registers: [ScanlineRegisters; SCREEN_HEIGHT],
    window_line: u8,
    rendering_vram: Vec<Vec<u8>>,
    rendering_oam: [u8; OAM_SIZE],
    progress: super::component::PpuProgress,
}

#[derive(Debug)]
struct OamDmaTransfer {
    source_base: u16,
    next_index: usize,
    pending_read: Option<(usize, PendingRead)>,
}

#[derive(Clone, Copy, Debug)]
struct HdmaTransfer {
    source: u16,
    destination: u16,
    remaining_blocks: u8,
    hblank_mode: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct ScanlineRegisters {
    lcdc: u8,
    scy: u8,
    scx: u8,
    bgp: u8,
    obp0: u8,
    obp1: u8,
    wy: u8,
    wx: u8,
}

#[derive(Clone, Copy, Debug, Default)]
struct BgPixel {
    color_index: u8,
    palette: u8,
    priority: bool,
}

impl PpuThread {
    pub fn spawn(
        init_state: PpuInitState,
        bus: Bus,
        interrupt_flags: InterruptFlags,
        inbox: Receiver<Command>,
        reports: Sender<ComponentReport>,
        frame_ready: SyncSender<PublishedFrame>,
        frame_recycle: Receiver<Vec<u8>>,
        clock: MasterClock,
    ) -> (thread::JoinHandle<()>, super::component::PpuProgress) {
        let progress = super::component::PpuProgress::new();
        let progress_clone = progress.clone();
        let handle = thread::Builder::new()
            .name("ppu".to_owned())
            .spawn(move || {
                let mut ppu = Self::from_init_state(
                    init_state,
                    bus,
                    interrupt_flags,
                    clock.clone(),
                    frame_ready,
                    frame_recycle,
                );
                ppu.progress = progress_clone;
                ppu.run(inbox, reports, clock)
            })
            .expect("failed to spawn ppu thread");
        (handle, progress)
    }

    fn run(
        mut self,
        inbox: Receiver<Command>,
        reports: Sender<ComponentReport>,
        _clock: MasterClock,
    ) {
        loop {
            if !self.service_inbox(&inbox) {
                break;
            }

            let target = self.clock.target();
            if self.dots < target {
                if !self.advance_ppu(&inbox, target) {
                    break;
                }
                self.advance_oam_dma();
                self.refill_spare_framebuffer();
                self.publish_report_if_needed(&reports);
            } else {
                thread::yield_now();
            }
        }
    }

    fn service_inbox(&mut self, inbox: &Receiver<Command>) -> bool {
        loop {
            match inbox.try_recv() {
                Ok(Command::Memory(command)) => self.handle_memory(command),
                Ok(Command::SetHardwareMode(hardware_mode)) => {
                    self.hardware_mode = hardware_mode;
                    self.framebuffer_dirty = true;
                    self.report_framebuffer_pending = true;
                    self.report_dirty = true;
                }
                Ok(Command::SaveState(respond_to)) => {
                    let state = self.create_save_state();
                    let bytes = bincode::serialize(&state).unwrap_or_default();
                    let _ = respond_to.send(bytes);
                }
                Ok(Command::LoadState(bytes)) => {
                    if let Ok(state) = bincode::deserialize::<PpuSaveState>(&bytes) {
                        self.apply_save_state(state);
                    }
                }
                Ok(Command::RequestPpuFeatures(respond_to)) => {
                    let features = super::component::PpuFeatures {
                        oam: self.oam,
                        lcdc: self.lcd_registers[LCDC_INDEX],
                        stat: self.lcd_registers[STAT_INDEX],
                        scy: self.lcd_registers[SCY_INDEX],
                        scx: self.lcd_registers[SCX_INDEX],
                        ly: self.lcd_registers[LY_INDEX],
                        wy: self.lcd_registers[WY_INDEX],
                        wx: self.lcd_registers[WX_INDEX],
                    };
                    let _ = respond_to.send(features);
                }
                Ok(Command::Stop) => return false,
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
    }

    fn create_save_state(&self) -> PpuSaveState {
        PpuSaveState {
            vram_banks: self.vram_banks.clone(),
            selected_vram_bank: self.selected_vram_bank,
            oam: self.oam.to_vec(),
            lcd_registers: self.lcd_registers.to_vec(),
            hdma_registers: self.hdma_registers.to_vec(),
            bg_palette_index: self.bg_palette_index,
            obj_palette_index: self.obj_palette_index,
            bg_palette_ram: self.bg_palette_ram.to_vec(),
            obj_palette_ram: self.obj_palette_ram.to_vec(),
            dots_into_line: self.dots_into_line,
            stat_interrupt_line: self.stat_interrupt_line,
            stat_interrupt_armed: self.stat_interrupt_armed,
            window_line: self.window_line,
            dots: self.dots,
            frames: self.frames,
            hardware_mode: self.hardware_mode,
        }
    }

    fn apply_save_state(&mut self, state: PpuSaveState) {
        self.vram_banks = state.vram_banks;
        self.selected_vram_bank = state.selected_vram_bank;
        let len = state.oam.len().min(OAM_SIZE);
        self.oam[..len].copy_from_slice(&state.oam[..len]);
        let len = state.lcd_registers.len().min(LCD_REGISTERS_LEN);
        self.lcd_registers[..len].copy_from_slice(&state.lcd_registers[..len]);
        let len = state.hdma_registers.len().min(HDMA_REGISTERS_LEN);
        self.hdma_registers[..len].copy_from_slice(&state.hdma_registers[..len]);
        self.bg_palette_index = state.bg_palette_index;
        self.obj_palette_index = state.obj_palette_index;
        let len = state.bg_palette_ram.len().min(PALETTE_RAM_LEN);
        self.bg_palette_ram[..len].copy_from_slice(&state.bg_palette_ram[..len]);
        let len = state.obj_palette_ram.len().min(PALETTE_RAM_LEN);
        self.obj_palette_ram[..len].copy_from_slice(&state.obj_palette_ram[..len]);
        self.dots_into_line = state.dots_into_line;
        self.stat_interrupt_line = state.stat_interrupt_line;
        self.stat_interrupt_armed = state.stat_interrupt_armed;
        self.window_line = state.window_line;
        self.dots = state.dots;
        self.frames = state.frames;
        self.hardware_mode = state.hardware_mode;
        // Transient rendering data
        self.oam_dma = None;
        self.hdma = None;
        // Rebuild rendering snapshots from the restored VRAM/OAM
        self.snapshot_vram_for_rendering();
        self.report_dirty = true;
        self.framebuffer_dirty = true;
        self.report_framebuffer_pending = true;
    }

    fn handle_memory(&mut self, command: MemoryCommand) {
        match command {
            MemoryCommand::Read {
                address,
                respond_to,
            } => {
                self.chase_clock();
                let mode = if self.lcd_enabled() { self.current_mode() } else { 0 };
                let result = match address {
                    0x8000..=0x9fff => {
                        if mode == 3 {
                            ReadResult::Ready(0xff)
                        } else {
                            ReadResult::Ready(
                                self.vram_banks[self.selected_vram_bank as usize]
                                    [(address - 0x8000) as usize],
                            )
                        }
                    }
                    0xfe00..=0xfe9f => {
                        if self.oam_dma.is_some() || mode >= 2 {
                            ReadResult::Ready(0xff)
                        } else {
                            ReadResult::Ready(self.oam[(address - 0xfe00) as usize])
                        }
                    }
                    0xff40..=0xff4b => {
                        ReadResult::Ready(self.lcd_registers[(address - 0xff40) as usize])
                    }
                    0xff4f if matches!(self.hardware_mode, HardwareMode::DmgCompatibility) => {
                        ReadResult::Ready(0xff)
                    }
                    0xff4f => ReadResult::Ready(0xfe | self.selected_vram_bank),
                    0xff51..=0xff55
                        if matches!(self.hardware_mode, HardwareMode::DmgCompatibility) =>
                    {
                        ReadResult::Ready(0xff)
                    }
                    0xff51..=0xff54 => {
                        ReadResult::Ready(self.hdma_registers[(address - 0xff51) as usize])
                    }
                    0xff55 => {
                        let value = match &self.hdma {
                            Some(hdma) => hdma.remaining_blocks.saturating_sub(1),
                            None => 0xff,
                        };
                        ReadResult::Ready(value)
                    }
                    0xff68..=0xff6b
                        if matches!(self.hardware_mode, HardwareMode::DmgCompatibility) =>
                    {
                        ReadResult::Ready(0xff)
                    }
                    0xff68 => ReadResult::Ready(self.bg_palette_index),
                    0xff69 => ReadResult::Ready(self.bg_palette_ram[self.bg_palette_address()]),
                    0xff6a => ReadResult::Ready(self.obj_palette_index),
                    0xff6b => ReadResult::Ready(self.obj_palette_ram[self.obj_palette_address()]),
                    _ => ReadResult::NoData,
                };
                let _ = respond_to.send(result);
            }
            MemoryCommand::Write {
                address,
                value,
                respond_to,
            } => {
                // Apply writes BEFORE chasing so dots aren't rendered
                // with stale data.  The mode check uses the PPU's current
                // position (slightly behind the CPU), which is more correct
                // than checking after a potentially large chase.
                let mode = if self.lcd_enabled() { self.current_mode() } else { 0 };
                let result = match address {
                    0x8000..=0x9fff => {
                        if mode != 3 {
                            self.vram_banks[self.selected_vram_bank as usize]
                                [(address - 0x8000) as usize] = value;
                            trace_ppu_memory_write(
                                self.frames,
                                address,
                                value,
                                self.lcd_registers[LY_INDEX],
                                self.dots_into_line,
                                mode,
                                if self.selected_vram_bank == 0 {
                                    "vram0"
                                } else {
                                    "vram1"
                                },
                            );
                        }
                        WriteResult::Accepted
                    }
                    0xfe00..=0xfe9f => {
                        if self.oam_dma.is_none() && mode < 2 {
                            self.oam[(address - 0xfe00) as usize] = value;
                            trace_ppu_memory_write(
                                self.frames,
                                address,
                                value,
                                self.lcd_registers[LY_INDEX],
                                self.dots_into_line,
                                mode,
                                "oam",
                            );
                        }
                        WriteResult::Accepted
                    }
                    0xff40 => {
                        trace_split_write(
                            address,
                            value,
                            self.lcd_registers[LY_INDEX],
                            self.dots_into_line,
                            self.current_mode(),
                        );
                        let was_enabled = self.lcd_enabled();
                        self.lcd_registers[LCDC_INDEX] = value;
                        trace_ppu_memory_write(
                            self.frames,
                            address,
                            value,
                            self.lcd_registers[LY_INDEX],
                            self.dots_into_line,
                            self.current_mode(),
                            "reg",
                        );
                        self.update_lcd_state(was_enabled);
                        self.framebuffer_dirty = true;
                        self.report_dirty = true;
                        WriteResult::Accepted
                    }
                    0xff41 => {
                        let mode_flags = self.lcd_registers[STAT_INDEX]
                            & (STAT_MODE_FLAG_MASK | STAT_LYC_FLAG_MASK);
                        self.lcd_registers[STAT_INDEX] =
                            STAT_UNUSED_MASK | (value & 0x78) | mode_flags;
                        trace_ppu_memory_write(
                            self.frames,
                            address,
                            value,
                            self.lcd_registers[LY_INDEX],
                            self.dots_into_line,
                            self.current_mode(),
                            "reg",
                        );
                        self.refresh_stat_interrupt_line();
                        WriteResult::Accepted
                    }
                    0xff44 => {
                        self.lcd_registers[LY_INDEX] = 0;
                        self.dots_into_line = 0;
                        trace_ppu_memory_write(
                            self.frames,
                            address,
                            value,
                            self.lcd_registers[LY_INDEX],
                            self.dots_into_line,
                            self.current_mode(),
                            "reg",
                        );
                        self.update_stat_mode();
                        WriteResult::Accepted
                    }
                    0xff45 => {
                        self.lcd_registers[LYC_INDEX] = value;
                        trace_ppu_memory_write(
                            self.frames,
                            address,
                            value,
                            self.lcd_registers[LY_INDEX],
                            self.dots_into_line,
                            self.current_mode(),
                            "reg",
                        );
                        self.update_stat_mode();
                        WriteResult::Accepted
                    }
                    0xff46 => {
                        self.lcd_registers[(address - 0xff40) as usize] = value;
                        trace_ppu_memory_write(
                            self.frames,
                            address,
                            value,
                            self.lcd_registers[LY_INDEX],
                            self.dots_into_line,
                            self.current_mode(),
                            "reg",
                        );
                        self.start_oam_dma(value);
                        WriteResult::Accepted
                    }
                    0xff42..=0xff43 | 0xff46..=0xff4b => {
                        trace_split_write(
                            address,
                            value,
                            self.lcd_registers[LY_INDEX],
                            self.dots_into_line,
                            self.current_mode(),
                        );
                        self.lcd_registers[(address - 0xff40) as usize] = value;
                        trace_ppu_memory_write(
                            self.frames,
                            address,
                            value,
                            self.lcd_registers[LY_INDEX],
                            self.dots_into_line,
                            self.current_mode(),
                            "reg",
                        );
                        WriteResult::Accepted
                    }
                    0xff4f | 0xff51..=0xff55 | 0xff68..=0xff6b
                        if matches!(self.hardware_mode, HardwareMode::DmgCompatibility) =>
                    {
                        WriteResult::Accepted
                    }
                    0xff4f => {
                        self.selected_vram_bank = value & 0x01;
                        trace_ppu_memory_write(
                            self.frames,
                            address,
                            value,
                            self.lcd_registers[LY_INDEX],
                            self.dots_into_line,
                            self.current_mode(),
                            "reg",
                        );
                        WriteResult::Accepted
                    }
                    0xff51..=0xff54 => {
                        self.hdma_registers[(address - 0xff51) as usize] = value;
                        WriteResult::Accepted
                    }
                    0xff55 => {
                        self.hdma_registers[4] = value;
                        self.start_hdma(value);
                        WriteResult::Accepted
                    }
                    0xff68 => {
                        self.bg_palette_index = sanitize_palette_index(value);
                        trace_ppu_memory_write(
                            self.frames,
                            address,
                            value,
                            self.lcd_registers[LY_INDEX],
                            self.dots_into_line,
                            self.current_mode(),
                            "reg",
                        );
                        WriteResult::Accepted
                    }
                    0xff69 => {
                        let address = self.bg_palette_address();
                        self.bg_palette_ram[address] = value;
                        trace_ppu_memory_write(
                            self.frames,
                            0xff69,
                            value,
                            self.lcd_registers[LY_INDEX],
                            self.dots_into_line,
                            self.current_mode(),
                            "bgp",
                        );
                        self.advance_bg_palette_index();
                        WriteResult::Accepted
                    }
                    0xff6a => {
                        self.obj_palette_index = sanitize_palette_index(value);
                        trace_ppu_memory_write(
                            self.frames,
                            address,
                            value,
                            self.lcd_registers[LY_INDEX],
                            self.dots_into_line,
                            self.current_mode(),
                            "reg",
                        );
                        WriteResult::Accepted
                    }
                    0xff6b => {
                        let address = self.obj_palette_address();
                        self.obj_palette_ram[address] = value;
                        trace_ppu_memory_write(
                            self.frames,
                            0xff6b,
                            value,
                            self.lcd_registers[LY_INDEX],
                            self.dots_into_line,
                            self.current_mode(),
                            "obp",
                        );
                        self.advance_obj_palette_index();
                        WriteResult::Accepted
                    }
                    _ => WriteResult::NoData,
                };
                // Do NOT chase after writes. The run loop's advance_ppu
                // (which checks the inbox every dot) will catch up and
                // render with the updated data.  Chasing here would render
                // many dots while OTHER queued writes sit unprocessed.
                if let Some(respond_to) = respond_to {
                    let _ = respond_to.send(result);
                }
            }
        }
    }

    fn from_init_state(
        init_state: PpuInitState,
        bus: Bus,
        interrupt_flags: InterruptFlags,
        clock: MasterClock,
        frame_ready: SyncSender<PublishedFrame>,
        frame_recycle: Receiver<Vec<u8>>,
    ) -> Self {
        let mut ppu = Self {
            bus,
            clock,
            interrupt_flags,
            dots: 0,
            hardware_mode: init_state.hardware_mode,
            vram_banks: vec![vec![0; VRAM_BANK_SIZE]; VRAM_BANKS],
            selected_vram_bank: init_state.selected_vram_bank,
            oam: init_state.oam,
            lcd_registers: init_state.lcd_registers,
            hdma_registers: init_state.hdma_registers,
            bg_palette_index: sanitize_palette_index(init_state.bg_palette_index),
            obj_palette_index: sanitize_palette_index(init_state.obj_palette_index),
            bg_palette_ram: init_state.bg_palette_ram,
            obj_palette_ram: init_state.obj_palette_ram,
            framebuffer: init_state.framebuffer,
            framebuffer_spare: frame_recycle.try_recv().ok(),
            pending_published_frame: None,
            frame_ready,
            frame_recycle,
            frames: 0,
            last_published_at: None,
            dots_into_line: 0,
            stat_interrupt_line: false,
            stat_interrupt_armed: true,
            report_dirty: true,
            framebuffer_dirty: true,
            frame_publication_pending: true,
            report_framebuffer_pending: false,
            oam_dma: None,
            hdma: None,
            scanline_registers: [ScanlineRegisters::default(); SCREEN_HEIGHT],
            window_line: 0,
            rendering_vram: vec![vec![0; VRAM_BANK_SIZE]; VRAM_BANKS],
            rendering_oam: init_state.oam,
            progress: super::component::PpuProgress::new(),
        };
        ppu.lcd_registers[STAT_INDEX] |= STAT_UNUSED_MASK;
        ppu.refresh_scanline_defaults();
        ppu.update_stat_mode();
        ppu
    }

    fn chase_clock(&mut self) {
        let target = self.clock.target();
        while self.dots < target {
            self.step_dot();
            self.dots += 1;
            self.progress.set(self.dots);
        }
    }

    fn advance_ppu(&mut self, inbox: &Receiver<Command>, target: u64) -> bool {
        let end = std::cmp::min(self.dots + PPU_DOTS_PER_SAMPLE as u64, target);
        while self.dots < end {
            if !self.service_inbox(inbox) {
                return false;
            }
            self.step_dot();
            self.dots += 1;
            self.progress.set(self.dots);
        }
        true
    }

    fn step_dot(&mut self) {
        if !self.lcd_enabled() {
            self.lcd_registers[LY_INDEX] = 0;
            self.dots_into_line = 0;
            self.update_stat_mode();
            return;
        }

        self.dots_into_line += 1;

        if self.dots_into_line == MODE2_DOTS + MODE3_DOTS {
            let line = self.lcd_registers[LY_INDEX];
            if line < VISIBLE_LINES {
                self.scanline_registers[line as usize] = self.capture_scanline_registers();
                self.render_scanline(line as usize);
                self.advance_hblank_dma();
            }
        }

        if self.dots_into_line >= DOTS_PER_LINE {
            self.dots_into_line = 0;
            let next_line = (self.lcd_registers[LY_INDEX] + 1) % TOTAL_LINES;
            self.lcd_registers[LY_INDEX] = next_line;

            if next_line == VISIBLE_LINES {
                self.frames = self.frames.wrapping_add(1);
                self.frame_publication_pending = true;
                self.report_dirty = true;
                self.interrupt_flags.set(VBLANK_INTERRUPT_MASK);
            }

            if next_line == 0 {
                self.window_line = 0;
                self.snapshot_vram_for_rendering();
            }
        }

        self.update_stat_mode();
        if !self.stat_interrupt_armed {
            self.stat_interrupt_armed = true;
            self.stat_interrupt_line = false;
        }
    }

    fn lcd_enabled(&self) -> bool {
        self.lcd_registers[LCDC_INDEX] & LCDC_ENABLE_MASK != 0
    }

    fn current_mode(&self) -> u8 {
        let line = self.lcd_registers[LY_INDEX];
        if line >= VISIBLE_LINES {
            1
        } else if self.dots_into_line < MODE2_DOTS {
            2
        } else if self.dots_into_line < MODE2_DOTS + MODE3_DOTS {
            3
        } else {
            0
        }
    }

    fn update_lcd_state(&mut self, was_enabled: bool) {
        if !self.lcd_enabled() {
            self.lcd_registers[LY_INDEX] = 0;
            self.dots_into_line = 0;
            self.window_line = 0;
            self.stat_interrupt_line = false;
            self.stat_interrupt_armed = true;
            self.refresh_scanline_defaults();
            self.report_dirty = true;
            self.framebuffer_dirty = true;
            self.report_framebuffer_pending = true;
        } else if !was_enabled {
            self.lcd_registers[LY_INDEX] = 0;
            self.dots_into_line = 0;
            self.window_line = 0;
            self.stat_interrupt_line = false;
            self.stat_interrupt_armed = false;
            self.refresh_scanline_defaults();
            self.report_dirty = true;
        }
        self.update_stat_mode();
    }

    fn update_stat_mode(&mut self) {
        let mode = if self.lcd_enabled() {
            self.current_mode()
        } else {
            0
        };
        let ly = self.lcd_registers[LY_INDEX];
        let lyc = self.lcd_registers[LYC_INDEX];
        let coincidence = ly == lyc;

        let mut stat = self.lcd_registers[STAT_INDEX] & !(STAT_MODE_FLAG_MASK | STAT_LYC_FLAG_MASK);
        stat |= STAT_UNUSED_MASK | mode;
        if coincidence {
            stat |= STAT_LYC_FLAG_MASK;
        }

        if self.lcd_registers[STAT_INDEX] != stat {
            self.lcd_registers[STAT_INDEX] = stat;
        }
        self.refresh_stat_interrupt_line();
    }

    fn refresh_stat_interrupt_line(&mut self) {
        let stat = self.lcd_registers[STAT_INDEX];
        let mode = stat & STAT_MODE_FLAG_MASK;
        let coincidence = stat & STAT_LYC_FLAG_MASK != 0;
        let mode0 = mode == 0 && stat & STAT_MODE0_INTERRUPT_MASK != 0;
        let mode1 = mode == 1 && stat & STAT_MODE1_INTERRUPT_MASK != 0;
        let mode2 = mode == 2 && stat & STAT_MODE2_INTERRUPT_MASK != 0;
        let lyc = coincidence && stat & STAT_LYC_INTERRUPT_MASK != 0;
        let interrupt_line = mode0 || mode1 || mode2 || lyc;

        if !self.stat_interrupt_armed {
            self.stat_interrupt_line = interrupt_line;
            return;
        }

        if interrupt_line && !self.stat_interrupt_line {
            self.interrupt_flags.set(STAT_INTERRUPT_MASK);
        }

        self.stat_interrupt_line = interrupt_line;
    }

    fn publish_report_if_needed(&mut self, reports: &Sender<ComponentReport>) {
        self.try_send_pending_frame();
        self.prepare_published_frame();
        self.try_send_pending_frame();

        if !self.report_dirty {
            return;
        }

        let framebuffer = if self.report_framebuffer_pending {
            if self.framebuffer_dirty {
                self.render_framebuffer();
                self.framebuffer_dirty = false;
            }
            self.report_framebuffer_pending = false;
            Some(self.framebuffer.clone())
        } else {
            None
        };

        let _ = reports.send(ComponentReport::Ppu(PpuReport {
            frames: self.frames,
            selected_vram_bank: self.selected_vram_bank,
            framebuffer,
        }));
        self.report_dirty = false;
    }

    fn refill_spare_framebuffer(&mut self) {
        if self.framebuffer_spare.is_some() {
            return;
        }

        match self.frame_recycle.try_recv() {
            Ok(framebuffer) => self.framebuffer_spare = Some(framebuffer),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => {}
        }
    }

    fn prepare_published_frame(&mut self) {
        if !self.frame_publication_pending {
            return;
        }

        // Get a spare buffer to swap with. Priority:
        // 1. Reclaim from a stale pending frame that couldn't be sent (drops the
        //    older frame so the newest completed frame always gets published).
        // 2. Recycled buffer returned by the app.
        // 3. Fresh allocation (one-time cost when contention first occurs).
        let mut spare = if let Some(stale) = self.pending_published_frame.take() {
            stale.framebuffer
        } else {
            self.refill_spare_framebuffer();
            self.framebuffer_spare
                .take()
                .unwrap_or_else(|| vec![0u8; FRAMEBUFFER_SIZE])
        };

        if self.framebuffer_dirty {
            if !self.lcd_enabled() {
                self.render_framebuffer();
            }
            self.framebuffer_dirty = false;
        }

        let published_at = Instant::now();
        trace_frame_publish(
            self.frames,
            self.last_published_at
                .map(|last| published_at.duration_since(last)),
        );
        self.last_published_at = Some(published_at);

        std::mem::swap(&mut self.framebuffer, &mut spare);
        self.pending_published_frame = Some(PublishedFrame {
            frame: self.frames,
            framebuffer: spare,
            published_at,
        });
        self.frame_publication_pending = false;
    }

    fn try_send_pending_frame(&mut self) {
        let Some(frame) = self.pending_published_frame.take() else {
            return;
        };

        match self.frame_ready.try_send(frame) {
            Ok(()) => {}
            Err(TrySendError::Full(frame)) => {
                trace_frame_queue_stall(frame.frame, frame.published_at.elapsed());
                self.pending_published_frame = Some(frame);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    fn start_hdma(&mut self, control: u8) {
        if matches!(self.hardware_mode, HardwareMode::DmgCompatibility) {
            return;
        }

        // Writing bit 7 = 0 while HBlank DMA is active cancels it.
        if control & 0x80 == 0 {
            if let Some(hdma) = &self.hdma {
                if hdma.hblank_mode {
                    self.hdma = None;
                    return;
                }
            }
        }

        let source = (u16::from(self.hdma_registers[0]) << 8
            | u16::from(self.hdma_registers[1]))
            & 0xfff0;
        let destination = (u16::from(self.hdma_registers[2]) << 8
            | u16::from(self.hdma_registers[3]))
            & 0x1ff0;
        let blocks = (control & 0x7f) + 1;
        let hblank_mode = control & 0x80 != 0;

        if hblank_mode {
            self.hdma = Some(HdmaTransfer {
                source,
                destination: 0x8000 | destination,
                remaining_blocks: blocks,
                hblank_mode: true,
            });
        } else {
            // General-purpose DMA: transfer all blocks immediately.
            self.execute_gp_dma(source, 0x8000 | destination, blocks);
        }
    }

    fn execute_gp_dma(&mut self, mut source: u16, mut destination: u16, blocks: u8) {
        for _ in 0..blocks {
            for i in 0..16u16 {
                let byte = self.bus_read_hdma(source.wrapping_add(i));
                let dest_addr = destination.wrapping_add(i);
                let bank = self.selected_vram_bank as usize;
                let offset = (dest_addr & 0x1fff) as usize;
                if offset < VRAM_BANK_SIZE {
                    self.vram_banks[bank][offset] = byte;
                }
            }
            source = source.wrapping_add(16);
            destination = destination.wrapping_add(16);
        }
        self.hdma = None;
    }

    fn advance_hblank_dma(&mut self) {
        let Some(mut hdma) = self.hdma.take() else {
            return;
        };
        if !hdma.hblank_mode || hdma.remaining_blocks == 0 {
            return;
        }

        // Transfer one 16-byte block.
        for i in 0..16u16 {
            let byte = self.bus_read_hdma(hdma.source.wrapping_add(i));
            let dest_addr = hdma.destination.wrapping_add(i);
            let bank = self.selected_vram_bank as usize;
            let offset = (dest_addr & 0x1fff) as usize;
            if offset < VRAM_BANK_SIZE {
                self.vram_banks[bank][offset] = byte;
            }
        }
        hdma.source = hdma.source.wrapping_add(16);
        hdma.destination = hdma.destination.wrapping_add(16);
        hdma.remaining_blocks -= 1;

        if hdma.remaining_blocks > 0 {
            self.hdma = Some(hdma);
        }
    }

    fn bus_read_hdma(&self, address: u16) -> u8 {
        // HDMA reads from ROM/SRAM/WRAM — for simplicity, use a synchronous
        // bus read with a spin-wait. This is acceptable because HDMA blocks
        // the CPU anyway (the bus message is tiny).
        let mut pending = self.bus.read(address);
        loop {
            if let Some(result) = pending.try_take() {
                return match result {
                    super::component::ReadResult::Ready(v) => v,
                    super::component::ReadResult::NoData => 0xff,
                };
            }
            std::thread::yield_now();
        }
    }

    fn start_oam_dma(&mut self, source_high: u8) {
        self.oam_dma = Some(OamDmaTransfer {
            source_base: u16::from(source_high) << 8,
            next_index: 0,
            pending_read: None,
        });
    }

    fn advance_oam_dma(&mut self) {
        let mut transferred = 0;

        while transferred < OAM_DMA_BYTES_PER_SAMPLE {
            let ly = self.lcd_registers[LY_INDEX];
            let dots = self.dots_into_line;
            let mode = self.current_mode();
            let Some(dma) = self.oam_dma.as_mut() else {
                return;
            };

            if let Some((index, mut pending_read)) = dma.pending_read.take() {
                let Some(result) = pending_read.try_take() else {
                    dma.pending_read = Some((index, pending_read));
                    return;
                };
                let value = match result {
                    ReadResult::Ready(value) => value,
                    ReadResult::NoData => 0xff,
                };

                self.oam[index] = value;
                dma.next_index = index + 1;
                trace_ppu_memory_write(
                    self.frames,
                    0xfe00 + index as u16,
                    value,
                    ly,
                    dots,
                    mode,
                    "dma",
                );
                transferred += 1;

                if dma.next_index >= OAM_SIZE {
                    self.oam_dma = None;
                    return;
                }

                continue;
            }

            if dma.next_index >= OAM_SIZE {
                self.oam_dma = None;
                return;
            }

            let index = dma.next_index;
            let address = dma.source_base.wrapping_add(index as u16);
            dma.pending_read = Some((index, self.bus.read(address)));
        }
    }

    fn render_framebuffer(&mut self) {
        debug_assert_eq!(self.framebuffer.len(), FRAMEBUFFER_SIZE);

        if !self.lcd_enabled() {
            self.fill_framebuffer([0xff, 0xff, 0xff, 0xff]);
            return;
        }

        self.window_line = 0;
        self.snapshot_vram_for_rendering();
        for y in 0..SCREEN_HEIGHT {
            self.render_scanline(y);
        }
    }

    fn fill_framebuffer(&mut self, rgba: [u8; 4]) {
        for pixel in self.framebuffer.chunks_exact_mut(BYTES_PER_PIXEL) {
            pixel.copy_from_slice(&rgba);
        }
    }

    fn render_scanline(&mut self, y: usize) {
        let registers = self.scanline_registers[y];
        // On CGB, LCDC bit 0 is "Master Priority" — BG is always rendered,
        // only sprite priority changes.  On DMG, bit 0 actually disables BG.
        let bg_enabled = matches!(self.hardware_mode, HardwareMode::Cgb)
            || registers.lcdc & LCDC_BG_ENABLE_MASK != 0;
        let mut bg_pixels = [BgPixel::default(); SCREEN_WIDTH];
        let window_line = self.window_line;
        let mut window_used = false;

        for x in 0..SCREEN_WIDTH {
            let bg_pixel = if bg_enabled {
                let (pixel, used_window) =
                    self.background_pixel(x as u8, y as u8, window_line, registers);
                window_used |= used_window;
                pixel
            } else {
                BgPixel::default()
            };

            bg_pixels[x] = bg_pixel;
            let rgba = if matches!(self.hardware_mode, HardwareMode::Cgb) {
                cgb_palette_color(&self.bg_palette_ram, bg_pixel.palette, bg_pixel.color_index)
            } else {
                dmg_compat_bg_color(&self.bg_palette_ram, bg_pixel.color_index, registers.bgp)
            };
            self.write_pixel(y * SCREEN_WIDTH + x, rgba);
        }

        if window_used {
            self.window_line = self.window_line.wrapping_add(1);
        }

        self.render_scanline_sprites(y, &bg_pixels, registers);
    }

    fn render_scanline_sprites(
        &mut self,
        y: usize,
        bg_pixels: &[BgPixel],
        registers: ScanlineRegisters,
    ) {
        if registers.lcdc & LCDC_OBJ_ENABLE_MASK == 0 {
            return;
        }

        let sprite_height = if registers.lcdc & LCDC_OBJ_SIZE_MASK != 0 {
            16
        } else {
            8
        };
        let mut visible = [0usize; 10];
        let mut visible_count = 0;

        for sprite_index in 0..MAX_SPRITES {
            let base = sprite_index * OAM_ENTRY_SIZE;
            let sprite_y = self.oam[base] as i16 - 16;

            if (y as i16) < sprite_y || (y as i16) >= sprite_y + sprite_height {
                continue;
            }

            visible[visible_count] = sprite_index;
            visible_count += 1;
            if visible_count == 10 {
                break;
            }
        }

        // DMG: sprite priority is by X coordinate (lower X = on top),
        // ties broken by OAM position. CGB: strictly by OAM position.
        if !matches!(self.hardware_mode, HardwareMode::Cgb) {
            visible[..visible_count].sort_by_key(|&i| {
                let x = self.oam[i * OAM_ENTRY_SIZE + 1];
                (x, i as u8)
            });
        }

        for &sprite_index in visible[..visible_count].iter().rev() {
            self.render_sprite_on_scanline(sprite_index, y, sprite_height, bg_pixels, registers);
        }
    }

    fn render_sprite_on_scanline(
        &mut self,
        sprite_index: usize,
        y: usize,
        sprite_height: i16,
        bg_pixels: &[BgPixel],
        registers: ScanlineRegisters,
    ) {
        let base = sprite_index * OAM_ENTRY_SIZE;
        let sprite_y = self.oam[base] as i16 - 16;
        let sprite_x = self.oam[base + 1] as i16 - 8;
        let mut tile_number = self.oam[base + 2];
        let attributes = self.oam[base + 3];
        let line_in_sprite = y as i16 - sprite_y;
        let row_in_sprite = if attributes & SPRITE_ATTR_Y_FLIP_MASK != 0 {
            sprite_height - 1 - line_in_sprite
        } else {
            line_in_sprite
        };

        let mut row = row_in_sprite as usize;
        if sprite_height == 16 {
            tile_number &= 0xfe;
            tile_number = tile_number.wrapping_add((row / 8) as u8);
            row %= 8;
        }

        let tile_bank = if matches!(self.hardware_mode, HardwareMode::Cgb)
            && attributes & SPRITE_ATTR_BANK_MASK != 0
        {
            1
        } else {
            0
        };

        for pixel in 0..8 {
            let screen_x = sprite_x + pixel as i16;
            if !(0..SCREEN_WIDTH as i16).contains(&screen_x) {
                continue;
            }

            let tile_x = if attributes & SPRITE_ATTR_X_FLIP_MASK != 0 {
                7 - pixel
            } else {
                pixel
            };
            let color_index = self.tile_pixel(tile_bank, tile_number, row, tile_x, true);

            if color_index == 0 {
                continue;
            }

            let framebuffer_index = y * SCREEN_WIDTH + screen_x as usize;
            let bg_pixel = bg_pixels[screen_x as usize];
            if self.sprite_hidden_by_background(attributes, bg_pixel, registers.lcdc) {
                continue;
            }

            let rgba = if matches!(self.hardware_mode, HardwareMode::Cgb) {
                cgb_palette_color(
                    &self.obj_palette_ram,
                    attributes & CGB_OBJ_ATTR_PALETTE_MASK,
                    color_index,
                )
            } else {
                let (palette_index, palette_register) =
                    if attributes & SPRITE_ATTR_DMG_PALETTE_MASK != 0 {
                        (1, registers.obp1)
                    } else {
                        (0, registers.obp0)
                    };
                dmg_compat_obj_color(
                    &self.obj_palette_ram,
                    palette_index,
                    color_index,
                    palette_register,
                )
            };
            self.write_pixel(framebuffer_index, rgba);
        }
    }

    fn background_pixel(
        &self,
        x: u8,
        y: u8,
        window_line: u8,
        registers: ScanlineRegisters,
    ) -> (BgPixel, bool) {
        let wx = registers.wx as i16 - 7;
        let use_window =
            registers.lcdc & LCDC_WINDOW_ENABLE_MASK != 0 && y >= registers.wy && (x as i16) >= wx;

        let (map_x, map_y, tile_map_base) = if use_window {
            let tile_map_base = if registers.lcdc & LCDC_WINDOW_TILE_MAP_MASK != 0 {
                BG_MAP_OFFSET_9C00
            } else {
                BG_MAP_OFFSET_9800
            };

            ((x as i16 - wx) as u8, window_line, tile_map_base)
        } else {
            let tile_map_base = if registers.lcdc & LCDC_BG_TILE_MAP_MASK != 0 {
                BG_MAP_OFFSET_9C00
            } else {
                BG_MAP_OFFSET_9800
            };

            (
                registers.scx.wrapping_add(x),
                registers.scy.wrapping_add(y),
                tile_map_base,
            )
        };

        let tile_x = (map_x as usize / 8) & 0x1f;
        let tile_y = (map_y as usize / 8) & 0x1f;
        let map_index = tile_map_base + tile_y * 32 + tile_x;
        let tile_number = self.vram_banks[0][map_index];
        let attributes = if matches!(self.hardware_mode, HardwareMode::Cgb) {
            self.vram_banks[1][map_index]
        } else {
            0
        };
        let tile_bank = if attributes & CGB_BG_ATTR_BANK_MASK != 0 {
            1
        } else {
            0
        };
        let row_in_tile = if attributes & CGB_BG_ATTR_Y_FLIP_MASK != 0 {
            7 - (map_y as usize & 0x07)
        } else {
            map_y as usize & 0x07
        };
        let column_in_tile = if attributes & CGB_BG_ATTR_X_FLIP_MASK != 0 {
            7 - (map_x as usize & 0x07)
        } else {
            map_x as usize & 0x07
        };

        (
            BgPixel {
                color_index: self.tile_pixel(
                    tile_bank,
                    tile_number,
                    row_in_tile,
                    column_in_tile,
                    registers.lcdc & LCDC_TILE_DATA_MASK != 0,
                ),
                palette: attributes & CGB_BG_ATTR_PALETTE_MASK,
                priority: attributes & CGB_BG_ATTR_PRIORITY_MASK != 0,
            },
            use_window,
        )
    }

    fn capture_scanline_registers(&self) -> ScanlineRegisters {
        ScanlineRegisters {
            lcdc: self.lcd_registers[LCDC_INDEX],
            scy: self.lcd_registers[SCY_INDEX],
            scx: self.lcd_registers[SCX_INDEX],
            bgp: self.lcd_registers[BGP_INDEX],
            obp0: self.lcd_registers[OBP0_INDEX],
            obp1: self.lcd_registers[OBP1_INDEX],
            wy: self.lcd_registers[WY_INDEX],
            wx: self.lcd_registers[WX_INDEX],
        }
    }

    fn refresh_scanline_defaults(&mut self) {
        let registers = self.capture_scanline_registers();
        self.scanline_registers.fill(registers);
    }

    fn snapshot_vram_for_rendering(&mut self) {
        // No-op: rendering now reads directly from live vram_banks/oam.
        // Mode 3 VRAM write blocking ensures consistency during rendering.
    }

    fn tile_pixel(
        &self,
        tile_bank: usize,
        tile_number: u8,
        row_in_tile: usize,
        column_in_tile: usize,
        unsigned_indexing: bool,
    ) -> u8 {
        let tile_base = if unsigned_indexing {
            UNSIGNED_TILE_DATA_BASE + tile_number as usize * TILE_BYTES
        } else {
            (SIGNED_TILE_DATA_BASE + (tile_number as i8 as i32) * TILE_BYTES as i32) as usize
        };
        let row_base = tile_base + row_in_tile * 2;
        let low = self.vram_banks[tile_bank][row_base];
        let high = self.vram_banks[tile_bank][row_base + 1];
        let bit = 7 - column_in_tile;
        ((high >> bit) & 0x01) << 1 | ((low >> bit) & 0x01)
    }

    fn write_pixel(&mut self, pixel_index: usize, rgba: [u8; 4]) {
        let start = pixel_index * BYTES_PER_PIXEL;
        self.framebuffer[start..start + BYTES_PER_PIXEL].copy_from_slice(&rgba);
    }

    fn bg_palette_address(&self) -> usize {
        (self.bg_palette_index & 0x3f) as usize
    }

    fn obj_palette_address(&self) -> usize {
        (self.obj_palette_index & 0x3f) as usize
    }

    fn advance_bg_palette_index(&mut self) {
        if self.bg_palette_index & 0x80 != 0 {
            self.bg_palette_index = 0x80 | ((self.bg_palette_index + 1) & 0x3f);
        }
    }

    fn advance_obj_palette_index(&mut self) {
        if self.obj_palette_index & 0x80 != 0 {
            self.obj_palette_index = 0x80 | ((self.obj_palette_index + 1) & 0x3f);
        }
    }

    fn sprite_hidden_by_background(&self, attributes: u8, bg_pixel: BgPixel, lcdc: u8) -> bool {
        if matches!(self.hardware_mode, HardwareMode::Cgb) {
            // CGB: when LCDC bit 0 (Master Priority) is cleared, sprites
            // always appear on top regardless of BG/OAM priority flags.
            if lcdc & LCDC_BG_ENABLE_MASK == 0 {
                return false;
            }
            bg_pixel.color_index != 0
                && (bg_pixel.priority || attributes & SPRITE_ATTR_PRIORITY_MASK != 0)
        } else {
            attributes & SPRITE_ATTR_PRIORITY_MASK != 0 && bg_pixel.color_index != 0
        }
    }

}

fn map_palette(color_index: u8, palette: u8) -> u8 {
    (palette >> (color_index * 2)) & 0x03
}

fn sanitize_palette_index(value: u8) -> u8 {
    value & 0xbf
}

fn cgb_palette_color(palette_ram: &[u8; PALETTE_RAM_LEN], palette: u8, color_index: u8) -> [u8; 4] {
    let base = ((palette as usize & 0x07) * 8) + ((color_index as usize & 0x03) * 2);
    let low = palette_ram[base];
    let high = palette_ram[base + 1];
    let color = u16::from(low) | (u16::from(high) << 8);
    let red = expand_color((color & 0x1f) as u8);
    let green = expand_color(((color >> 5) & 0x1f) as u8);
    let blue = expand_color(((color >> 10) & 0x1f) as u8);
    [red, green, blue, 0xff]
}

fn dmg_compat_bg_color(palette_ram: &[u8; PALETTE_RAM_LEN], color_index: u8, bgp: u8) -> [u8; 4] {
    cgb_palette_color(palette_ram, 0, map_palette(color_index, bgp))
}

fn dmg_compat_obj_color(
    palette_ram: &[u8; PALETTE_RAM_LEN],
    palette: u8,
    color_index: u8,
    obp: u8,
) -> [u8; 4] {
    cgb_palette_color(palette_ram, palette, map_palette(color_index, obp))
}

fn trace_split_write(address: u16, value: u8, ly: u8, dots: u16, mode: u8) {
    tracing::trace!(addr = format_args!("{address:04x}"), value = format_args!("{value:02x}"), ly, dots, mode, "ppu split write");
}

fn trace_ppu_memory_write(
    frame: u64,
    address: u16,
    value: u8,
    ly: u8,
    dots: u16,
    mode: u8,
    kind: &str,
) {
    tracing::trace!(frame, addr = format_args!("{address:04x}"), value = format_args!("{value:02x}"), ly, dots, mode, kind, "ppu memory write");
}

fn trace_frame_queue_stall(frame: u64, age: std::time::Duration) {
    if age >= std::time::Duration::from_millis(2) {
        tracing::warn!(frame, age_us = age.as_micros() as u64, "frame queue stall");
    }
}

fn trace_frame_publish(frame: u64, delta: Option<std::time::Duration>) {
    let Some(delta) = delta else { return };
    if delta < std::time::Duration::from_millis(12) || delta > std::time::Duration::from_millis(20) {
        tracing::debug!(frame, delta_us = delta.as_micros() as u64, "frame publish timing");
    }
}

fn expand_color(component: u8) -> u8 {
    (component << 3) | (component >> 2)
}

impl Default for PpuThread {
    fn default() -> Self {
        let (cpu_sender, _cpu_receiver) = std::sync::mpsc::channel();
        let (ppu_sender, _ppu_receiver) = std::sync::mpsc::channel();
        let (wram_sender, _wram_receiver) = std::sync::mpsc::channel();
        let (cartridge_sender, _cartridge_receiver) = std::sync::mpsc::channel();
        let (timer_sender, _timer_receiver) = std::sync::mpsc::channel();
        let (apu_sender, _apu_receiver) = std::sync::mpsc::channel();
        let (frame_ready_sender, _frame_ready_receiver) = std::sync::mpsc::sync_channel(1);
        let (frame_recycle_sender, frame_recycle_receiver) = std::sync::mpsc::sync_channel(2);
        let _ = frame_recycle_sender.try_send(crate::emulator::component::default_framebuffer());
        Self::from_init_state(
            PpuInitState::default(),
            Bus::new(
                cpu_sender,
                ppu_sender,
                wram_sender,
                cartridge_sender,
                timer_sender,
                apu_sender,
            ),
            InterruptFlags::new(),
            MasterClock::new(),
            frame_ready_sender,
            frame_recycle_receiver,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, SyncSender};

    fn ppu_with_channels() -> (
        PpuThread,
        Receiver<PublishedFrame>,
        Sender<ComponentReport>,
        Receiver<ComponentReport>,
    ) {
        let (cpu_sender, _cpu_receiver) = std::sync::mpsc::channel();
        let (ppu_sender, _ppu_receiver) = std::sync::mpsc::channel();
        let (wram_sender, _wram_receiver) = std::sync::mpsc::channel();
        let (cartridge_sender, _cartridge_receiver) = std::sync::mpsc::channel();
        let (timer_sender, _timer_receiver) = std::sync::mpsc::channel();
        let (apu_sender, _apu_receiver) = std::sync::mpsc::channel();
        let (frame_ready_sender, frame_ready_receiver): (
            SyncSender<PublishedFrame>,
            Receiver<PublishedFrame>,
        ) = std::sync::mpsc::sync_channel(1);
        let (frame_recycle_sender, frame_recycle_receiver) = std::sync::mpsc::sync_channel(2);
        let _ = frame_recycle_sender.try_send(crate::emulator::component::default_framebuffer());
        let (reports_sender, reports_receiver) = std::sync::mpsc::channel();

        (
            PpuThread::from_init_state(
                PpuInitState::default(),
                Bus::new(
                    cpu_sender,
                    ppu_sender,
                    wram_sender,
                    cartridge_sender,
                    timer_sender,
                    apu_sender,
                ),
                InterruptFlags::new(),
                MasterClock::new(),
                frame_ready_sender,
                frame_recycle_receiver,
            ),
            frame_ready_receiver,
            reports_sender,
            reports_receiver,
        )
    }

    #[test]
    fn dmg_compat_background_palette_uses_cgb_palette_zero() {
        let mut palette_ram = [0u8; PALETTE_RAM_LEN];
        palette_ram[0] = 0x00;
        palette_ram[1] = 0x00;
        palette_ram[2] = 0xff;
        palette_ram[3] = 0x7f;

        assert_eq!(
            dmg_compat_bg_color(&palette_ram, 1, 0b0000_0100),
            [0xff, 0xff, 0xff, 0xff]
        );
    }

    #[test]
    fn dmg_compat_object_palette_uses_selected_obj_palette() {
        let mut palette_ram = [0u8; PALETTE_RAM_LEN];
        let base = 8;
        palette_ram[base + 4] = 0x1f;
        palette_ram[base + 5] = 0x00;

        assert_eq!(
            dmg_compat_obj_color(&palette_ram, 1, 2, 0b0010_0000),
            [0xff, 0x00, 0x00, 0xff]
        );
    }

    #[test]
    fn render_background_uses_latched_scx_per_scanline() {
        let mut ppu = PpuThread::default();
        ppu.lcd_registers[LCDC_INDEX] =
            LCDC_ENABLE_MASK | LCDC_BG_ENABLE_MASK | LCDC_TILE_DATA_MASK;
        ppu.bg_palette_ram[0] = 0xff;
        ppu.bg_palette_ram[1] = 0x7f;
        ppu.vram_banks[0][0x0000] = 0xff;
        ppu.vram_banks[0][0x0001] = 0xff;
        ppu.vram_banks[0][0x0010] = 0x00;
        ppu.vram_banks[0][0x0011] = 0x00;
        ppu.vram_banks[0][BG_MAP_OFFSET_9800] = 0x00;
        ppu.vram_banks[0][BG_MAP_OFFSET_9800 + 1] = 0x01;
        ppu.vram_banks[0][BG_MAP_OFFSET_9800 + 32 * 4] = 0x00;
        ppu.vram_banks[0][BG_MAP_OFFSET_9800 + 32 * 4 + 1] = 0x01;
        ppu.refresh_scanline_defaults();
        ppu.scanline_registers[32].scx = 8;

        ppu.render_framebuffer();

        assert_eq!(&ppu.framebuffer[0..4], &[0x00, 0x00, 0x00, 0xff]);
        let lower_left = 32 * SCREEN_WIDTH * BYTES_PER_PIXEL;
        assert_eq!(
            &ppu.framebuffer[lower_left..lower_left + 4],
            &[0xff, 0xff, 0xff, 0xff]
        );
    }

    #[test]
    fn non_frame_report_includes_framebuffer_without_live_publication() {
        let (mut ppu, frame_ready_receiver, reports_sender, reports_receiver) = ppu_with_channels();

        ppu.frame_publication_pending = false;
        ppu.report_framebuffer_pending = true;
        ppu.report_dirty = true;
        ppu.publish_report_if_needed(&reports_sender);

        let report = match reports_receiver.try_recv() {
            Ok(ComponentReport::Ppu(report)) => report,
            other => panic!("expected PPU report, got {other:?}"),
        };
        assert!(report.framebuffer.is_some());
        assert!(matches!(
            frame_ready_receiver.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
    }

    #[test]
    fn frame_publication_uses_live_channel_without_report_clone() {
        let (mut ppu, frame_ready_receiver, reports_sender, reports_receiver) = ppu_with_channels();

        ppu.frame_publication_pending = true;
        ppu.report_framebuffer_pending = false;
        ppu.report_dirty = true;
        ppu.publish_report_if_needed(&reports_sender);

        let report = match reports_receiver.try_recv() {
            Ok(ComponentReport::Ppu(report)) => report,
            other => panic!("expected PPU report, got {other:?}"),
        };
        assert!(report.framebuffer.is_none());
        assert!(frame_ready_receiver.try_recv().is_ok());
    }
}
