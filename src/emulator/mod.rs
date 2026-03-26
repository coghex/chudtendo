mod apu;
mod bus;
mod cartridge;
mod component;
mod cpu;
mod ppu;
mod timer;
mod wram;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use serde::{Deserialize, Serialize};

pub use bus::{PendingRead, PendingWrite};
pub use cartridge::{
    BootMode, BootRomImage, BootRomLoadError, CartridgeImage, CartridgeLoadError,
    CartridgeMetadata, MbcKind,
};
pub use component::{
    CpuRegisters, HardwareMode, PublishedFrame, ReadResult, SCREEN_HEIGHT, SCREEN_WIDTH, Snapshot,
    ThreadDelays, WriteResult,
};

use bus::Bus;
use component::{
    Command, ComponentReport, CpuInitState, InterruptFlags, MasterClock, PpuInitState,
    SharedCartridgeReadState, TimerInitState, WramInitState,
};

/// The full on-disk save state bundle.
#[derive(Clone, Serialize, Deserialize)]
struct FullSaveState {
    cpu: Vec<u8>,
    ppu: Vec<u8>,
    wram: Vec<u8>,
    cartridge: Vec<u8>,
    timer: Vec<u8>,
    apu: Vec<u8>,
    clock_cycles: u64,
}

/// A rewind snapshot bundles the component state with a copy of the
/// framebuffer so we can display it immediately during rewind playback
/// without waiting for the PPU to re-render.
#[derive(Clone)]
struct RewindSnapshot {
    state: FullSaveState,
    framebuffer: Vec<u8>,
}

const REWIND_BUFFER_DEFAULT_CAPACITY: usize = 450; // ~30 seconds at 1 capture per 4 frames

struct RewindBuffer {
    buffer: std::collections::VecDeque<RewindSnapshot>,
    capacity: usize,
}

impl RewindBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            buffer: std::collections::VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    fn push(&mut self, snapshot: RewindSnapshot) {
        if self.buffer.len() >= self.capacity {
            self.buffer.pop_front();
        }
        self.buffer.push_back(snapshot);
    }

    fn pop(&mut self) -> Option<RewindSnapshot> {
        self.buffer.pop_back()
    }

    fn clear(&mut self) {
        self.buffer.clear();
    }

    fn len(&self) -> usize {
        self.buffer.len()
    }
}

impl std::fmt::Debug for RewindBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RewindBuffer")
            .field("len", &self.buffer.len())
            .field("capacity", &self.capacity)
            .finish()
    }
}

#[derive(Debug)]
struct WorkerHandle {
    name: &'static str,
    handle: thread::JoinHandle<()>,
}

#[derive(Clone, Copy, Debug, Default)]
struct ReportDrainStats {
    total: usize,
    cpu: usize,
    ppu: usize,
    wram: usize,
    cartridge: usize,
    timer: usize,
}

#[derive(Debug)]
struct ComponentInboxes {
    cpu: Option<Receiver<Command>>,
    ppu: Option<Receiver<Command>>,
    wram: Option<Receiver<Command>>,
    cartridge: Option<Receiver<Command>>,
    timer: Option<Receiver<Command>>,
    apu: Option<Receiver<Command>>,
}

#[derive(Clone, Debug)]
pub struct EmulatorConfig {
    pub delays: ThreadDelays,
    pub cartridge: CartridgeImage,
    pub boot_seed: u64,
    pub boot_rom_path: PathBuf,
    pub save_path: Option<PathBuf>,
}

impl EmulatorConfig {
    pub fn with_cartridge(cartridge: CartridgeImage) -> Self {
        Self {
            delays: ThreadDelays::default(),
            cartridge,
            boot_seed: rand::random(),
            boot_rom_path: default_boot_rom_path(),
            save_path: None,
        }
    }
}

impl Default for EmulatorConfig {
    fn default() -> Self {
        Self {
            delays: ThreadDelays::default(),
            cartridge: CartridgeImage::placeholder(),
            boot_seed: rand::random(),
            boot_rom_path: default_boot_rom_path(),
            save_path: None,
        }
    }
}

#[derive(Debug)]
pub struct Emulator {
    config: EmulatorConfig,
    bus: Bus,
    inboxes: ComponentInboxes,
    reports: Receiver<ComponentReport>,
    frame_ready: Option<Receiver<PublishedFrame>>,
    frame_recycle: Option<SyncSender<Vec<u8>>>,
    sample_ready: Option<Receiver<[f32; 2]>>,
    sample_sender: Option<SyncSender<[f32; 2]>>,
    serial_output: Option<Receiver<u8>>,
    serial_output_sender: Option<SyncSender<u8>>,
    clock: Option<MasterClock>,
    joypad: crate::input::JoypadState,
    cached_snapshot: Snapshot,
    workers: Vec<WorkerHandle>,
    speed: std::sync::Arc<std::sync::atomic::AtomicU32>,
    paused: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ppu_progress: Option<component::PpuProgress>,
    rewind_buffer: RewindBuffer,
}

impl Emulator {
    pub fn new() -> Self {
        Self::with_config(EmulatorConfig::default())
    }

    pub fn with_config(config: EmulatorConfig) -> Self {
        let (cpu_sender, cpu_receiver) = mpsc::channel();
        let (ppu_sender, ppu_receiver) = mpsc::channel();
        let (wram_sender, wram_receiver) = mpsc::channel();
        let (cartridge_sender, cartridge_receiver) = mpsc::channel();
        let (timer_sender, timer_receiver) = mpsc::channel();
        let (apu_sender, apu_receiver) = mpsc::channel();
        let (_report_sender, report_receiver) = mpsc::channel();
        let (sample_sender, sample_receiver) = mpsc::sync_channel(2048);
        let (serial_out_sender, serial_out_receiver) = mpsc::sync_channel(4096);

        Self {
            config,
            bus: Bus::new(
                cpu_sender,
                ppu_sender,
                wram_sender,
                cartridge_sender,
                timer_sender,
                apu_sender,
            ),
            inboxes: ComponentInboxes {
                cpu: Some(cpu_receiver),
                ppu: Some(ppu_receiver),
                wram: Some(wram_receiver),
                cartridge: Some(cartridge_receiver),
                timer: Some(timer_receiver),
                apu: Some(apu_receiver),
            },
            reports: report_receiver,
            frame_ready: None,
            frame_recycle: None,
            sample_ready: Some(sample_receiver),
            sample_sender: Some(sample_sender),
            serial_output: Some(serial_out_receiver),
            serial_output_sender: Some(serial_out_sender),
            clock: None,
            joypad: crate::input::JoypadState::new(),
            cached_snapshot: Snapshot::default(),
            workers: Vec::new(),
            speed: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(100)),
            paused: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            ppu_progress: None,
            rewind_buffer: RewindBuffer::new(REWIND_BUFFER_DEFAULT_CAPACITY),
        }
    }

    pub fn from_rom_bytes(rom: Vec<u8>) -> Result<Self, CartridgeLoadError> {
        let cartridge = CartridgeImage::from_bytes(rom)?;
        cartridge.ensure_runtime_supported()?;
        Ok(Self::with_config(EmulatorConfig::with_cartridge(cartridge)))
    }

    pub fn from_rom_file<P: AsRef<Path>>(path: P) -> Result<Self, CartridgeLoadError> {
        let path = path.as_ref();
        let cartridge = CartridgeImage::from_file(path)?;
        cartridge.ensure_runtime_supported()?;
        let mut config = EmulatorConfig::with_cartridge(cartridge);
        config.save_path = Some(path.with_extension("sav"));
        Ok(Self::with_config(config))
    }

    pub fn cartridge_metadata(&self) -> &CartridgeMetadata {
        self.config.cartridge.metadata()
    }

    pub fn set_dmg_mode(&mut self) {
        self.config.boot_rom_path = dmg_boot_rom_path();
    }

    pub fn joypad(&self) -> &crate::input::JoypadState {
        &self.joypad
    }

    pub fn take_sample_receiver(&mut self) -> Option<Receiver<[f32; 2]>> {
        self.sample_ready.take()
    }

    pub fn take_serial_receiver(&mut self) -> Option<Receiver<u8>> {
        self.serial_output.take()
    }

    pub fn set_ppu_delay(&mut self, delay: Duration) -> Result<(), String> {
        if !self.workers.is_empty() {
            return Err("cannot update PPU delay after emulator start".to_owned());
        }

        self.config.delays.ppu = delay;
        Ok(())
    }

    pub fn start(&mut self) -> Result<(), String> {
        if !self.workers.is_empty() {
            return Ok(());
        }

        let (report_sender, report_receiver) = mpsc::channel();
        let (frame_ready_sender, frame_ready_receiver) = mpsc::sync_channel(1);
        let (frame_recycle_sender, frame_recycle_receiver) = mpsc::sync_channel(2);
        self.reports = report_receiver;
        self.frame_ready = Some(frame_ready_receiver);
        self.frame_recycle = Some(frame_recycle_sender.clone());
        let _ = frame_recycle_sender.try_send(component::default_framebuffer());
        let boot_rom = BootRomImage::from_file(&self.config.boot_rom_path)
            .map_err(|error| error.to_string())?;
        let is_dmg_boot = boot_rom.is_dmg();
        let shared_cartridge = SharedCartridgeReadState::new(
            self.config.cartridge.shared_rom(),
            boot_rom.shared_bytes(),
            0,
            1,
        );
        let mut cpu_init = build_cpu_init_state(
            self.config.cartridge.metadata(),
            self.config.boot_seed,
            Some(shared_cartridge.clone()),
        );
        let interrupt_flags = InterruptFlags::new();
        cpu_init.joypad = self.joypad.clone();
        cpu_init.serial_output = self.serial_output_sender.take();
        cpu_init.interrupt_flags = interrupt_flags.clone();
        cpu_init.speed = self.speed.clone();
        cpu_init.paused = self.paused.clone();
        if is_dmg_boot {
            cpu_init.hardware_mode = HardwareMode::DmgCompatibility;
            cpu_init.boot_target_mode = HardwareMode::DmgCompatibility;
        }
        let mut ppu_init = build_ppu_init_state(self.config.cartridge.metadata());
        let timer_init = build_timer_init_state(self.config.cartridge.metadata());
        let mut wram_init =
            build_wram_init_state(self.config.cartridge.metadata(), self.config.boot_seed);
        if is_dmg_boot {
            ppu_init.hardware_mode = HardwareMode::DmgCompatibility;
            wram_init.hardware_mode = HardwareMode::DmgCompatibility;
            // Initialize CGB palette RAM with DMG grayscale so rendering works.
            // DMG boot ROM doesn't touch CGB palettes, so we provide defaults.
            // 4 shades: white, light gray, dark gray, black (RGB555 values).
            let dmg_colors: [[u8; 2]; 4] = [
                [0xff, 0x7f], // white  (0x7FFF)
                [0x10, 0x4a], // light  (0x4A10)
                [0x08, 0x21], // dark   (0x2108)
                [0x00, 0x00], // black  (0x0000)
            ];
            for palette in 0..8 {
                for color in 0..4 {
                    let base = palette * 8 + color * 2;
                    ppu_init.bg_palette_ram[base] = dmg_colors[color][0];
                    ppu_init.bg_palette_ram[base + 1] = dmg_colors[color][1];
                    ppu_init.obj_palette_ram[base] = dmg_colors[color][0];
                    ppu_init.obj_palette_ram[base + 1] = dmg_colors[color][1];
                }
            }
        }

        let cpu_inbox = self
            .inboxes
            .cpu
            .take()
            .ok_or_else(|| "cpu inbox unavailable".to_owned())?;
        let ppu_inbox = self
            .inboxes
            .ppu
            .take()
            .ok_or_else(|| "ppu inbox unavailable".to_owned())?;
        let wram_inbox = self
            .inboxes
            .wram
            .take()
            .ok_or_else(|| "wram inbox unavailable".to_owned())?;
        let cartridge_inbox = self
            .inboxes
            .cartridge
            .take()
            .ok_or_else(|| "cartridge inbox unavailable".to_owned())?;
        let timer_inbox = self
            .inboxes
            .timer
            .take()
            .ok_or_else(|| "timer inbox unavailable".to_owned())?;

        let clock = MasterClock::new();
        self.clock = Some(clock.clone());

        let (ppu_handle, ppu_progress) = ppu::PpuThread::spawn(
            ppu_init,
            self.bus.clone(),
            interrupt_flags.clone(),
            ppu_inbox,
            report_sender.clone(),
            frame_ready_sender,
            frame_recycle_receiver,
            clock.clone(),
        );
        self.workers.push(WorkerHandle {
            name: "ppu",
            handle: ppu_handle,
        });
        self.ppu_progress = Some(ppu_progress.clone());
        cpu_init.ppu_progress = ppu_progress;
        self.workers.push(WorkerHandle {
            name: "cpu",
            handle: cpu::CpuThread::spawn(
                cpu_init,
                self.bus.clone(),
                cpu_inbox,
                report_sender.clone(),
                clock.clone(),
            ),
        });
        self.workers.push(WorkerHandle {
            name: "wram",
            handle: wram::WramThread::spawn(
                wram_init,
                wram_inbox,
                report_sender.clone(),
            ),
        });
        self.workers.push(WorkerHandle {
            name: "cartridge",
            handle: cartridge::CartridgeThread::spawn(
                self.config.cartridge.clone(),
                boot_rom,
                self.config.boot_seed,
                Some(shared_cartridge),
                self.config.save_path.clone(),
                cartridge_inbox,
                report_sender.clone(),
            )
            .map_err(|error| error.to_string())?,
        });
        self.workers.push(WorkerHandle {
            name: "timer",
            handle: timer::TimerThread::spawn(
                timer_init,
                timer_inbox,
                self.bus.clone(),
                interrupt_flags.clone(),
                report_sender.clone(),
                clock.clone(),
            ),
        });

        let apu_inbox = self
            .inboxes
            .apu
            .take()
            .ok_or_else(|| "apu inbox unavailable".to_owned())?;
        let sample_sender = self
            .sample_sender
            .take()
            .ok_or_else(|| "apu sample sender unavailable".to_owned())?;
        self.workers.push(WorkerHandle {
            name: "apu",
            handle: apu::ApuThread::spawn(
                apu_inbox,
                report_sender,
                sample_sender,
                clock.clone(),
                if is_dmg_boot {
                    HardwareMode::DmgCompatibility
                } else {
                    HardwareMode::Cgb
                },
            ),
        });

        Ok(())
    }

    pub fn read(&self, address: u16) -> PendingRead {
        self.bus.read(address)
    }

    pub fn write(&self, address: u16, value: u8) -> PendingWrite {
        self.bus.write(address, value)
    }

    fn drain_reports(&mut self, context: &str) -> ReportDrainStats {
        let start = Instant::now();
        let mut stats = ReportDrainStats::default();

        while let Ok(report) = self.reports.try_recv() {
            match report {
                ComponentReport::Cpu(report) => {
                    stats.total += 1;
                    stats.cpu += 1;
                    self.cached_snapshot.cpu_steps = report.steps;
                    self.cached_snapshot.cpu_af = report.registers.af;
                    self.cached_snapshot.cpu_bc = report.registers.bc;
                    self.cached_snapshot.cpu_de = report.registers.de;
                    self.cached_snapshot.cpu_hl = report.registers.hl;
                    self.cached_snapshot.cpu_sp = report.registers.sp;
                    self.cached_snapshot.cpu_pc = report.registers.pc;
                }
                ComponentReport::Ppu(report) => {
                    stats.total += 1;
                    stats.ppu += 1;
                    self.cached_snapshot.ppu_frames = report.frames;
                    self.cached_snapshot.selected_vram_bank = report.selected_vram_bank;
                    if let Some(framebuffer) = report.framebuffer {
                        self.cached_snapshot.framebuffer = framebuffer;
                    }
                }
                ComponentReport::Wram(report) => {
                    stats.total += 1;
                    stats.wram += 1;
                    self.cached_snapshot.wram_steps = report.steps;
                    self.cached_snapshot.selected_wram_bank = report.selected_bank;
                }
                ComponentReport::Cartridge(report) => {
                    stats.total += 1;
                    stats.cartridge += 1;
                    self.cached_snapshot.cartridge_steps = report.steps;
                    self.cached_snapshot.selected_rom_bank = report.selected_rom_bank;
                    self.cached_snapshot.selected_ram_bank = report.selected_ram_bank;
                }
                ComponentReport::Timer(report) => {
                    stats.total += 1;
                    stats.timer += 1;
                    self.cached_snapshot.timer_ticks = report.ticks;
                }
                ComponentReport::Apu(_) => {
                    stats.total += 1;
                }
            }
        }

        trace_report_drain(context, start.elapsed(), stats);
        stats
    }

    fn drain_published_frames_into_snapshot(&mut self) {
        let mut frames = Vec::new();
        if let Some(receiver) = &self.frame_ready {
            while let Ok(frame) = receiver.try_recv() {
                frames.push(frame);
            }
        }

        for frame in frames {
            self.cached_snapshot.ppu_frames = self.cached_snapshot.ppu_frames.max(frame.frame);
            self.cached_snapshot
                .framebuffer
                .clone_from(&frame.framebuffer);
            self.recycle_framebuffer(frame.framebuffer);
        }
    }

    pub fn snapshot(&mut self) -> Snapshot {
        self.drain_reports("snapshot");
        self.drain_published_frames_into_snapshot();
        self.cached_snapshot.clone()
    }

    pub fn status_snapshot(&mut self) -> Snapshot {
        self.drain_reports("status_snapshot");
        self.cached_snapshot.clone()
    }

    pub fn take_latest_frame(&mut self) -> Option<PublishedFrame> {
        let started = Instant::now();
        let report_stats = self.drain_reports("take_latest_frame");

        let mut drained = Vec::new();
        if let Some(receiver) = &self.frame_ready {
            while let Ok(frame) = receiver.try_recv() {
                drained.push(frame);
            }
        }

        let drained_frames = drained.len();
        let first_frame = drained.first().map(|frame| frame.frame);
        let latest_frame_number = drained.last().map(|frame| frame.frame);
        let latest_age = drained.last().map(|frame| frame.published_at.elapsed());

        let mut latest = None;
        for frame in drained {
            if let Some(previous) = latest.replace(frame) {
                self.recycle_framebuffer(previous.framebuffer);
            }
        }

        if let Some(frame) = &latest {
            self.cached_snapshot.ppu_frames = self.cached_snapshot.ppu_frames.max(frame.frame);
        }

        trace_take_latest_frame(
            started.elapsed(),
            report_stats,
            drained_frames,
            first_frame,
            latest_frame_number,
            latest_age,
        );
        latest
    }

    pub fn recycle_framebuffer(&self, framebuffer: Vec<u8>) {
        if let Some(sender) = &self.frame_recycle {
            let _ = sender.try_send(framebuffer);
        }
    }

    pub fn stop(&mut self) {
        if self.workers.is_empty() {
            return;
        }

        self.bus.stop_components();

        while let Some(worker) = self.workers.pop() {
            worker
                .handle
                .join()
                .unwrap_or_else(|_| panic!("{} thread panicked while shutting down", worker.name));
        }

        self.frame_ready = None;
        self.frame_recycle = None;
    }

    fn state_path(&self, slot: u8) -> Option<PathBuf> {
        self.config
            .save_path
            .as_ref()
            .map(|p| p.with_extension(format!("state{slot}")))
    }

    /// Returns the modification timestamp for a save-state slot, or `None` if
    /// the slot is empty / the path is not configured.
    pub fn slot_timestamp(&self, slot: u8) -> Option<std::time::SystemTime> {
        let path = self.state_path(slot)?;
        std::fs::metadata(&path).ok()?.modified().ok()
    }

    fn capture_state(&self) -> Result<FullSaveState, String> {
        let receivers = self.bus.save_state_components();

        let timeout = Duration::from_secs(2);
        let deadline = Instant::now() + timeout;
        let mut blobs: Vec<Option<Vec<u8>>> = vec![None; 6];

        while blobs.iter().any(|b| b.is_none()) {
            if Instant::now() > deadline {
                return Err("timeout waiting for component save state".to_owned());
            }
            for (i, rx) in receivers.iter().enumerate() {
                if blobs[i].is_none() {
                    if let Ok(bytes) = rx.try_recv() {
                        blobs[i] = Some(bytes);
                    }
                }
            }
            thread::yield_now();
        }

        let mut blobs = blobs.into_iter().map(|b| b.unwrap());
        let clock_cycles = self
            .clock
            .as_ref()
            .map(|c| c.target())
            .unwrap_or(0);

        Ok(FullSaveState {
            cpu: blobs.next().unwrap(),
            ppu: blobs.next().unwrap(),
            wram: blobs.next().unwrap(),
            cartridge: blobs.next().unwrap(),
            timer: blobs.next().unwrap(),
            apu: blobs.next().unwrap(),
            clock_cycles,
        })
    }

    fn restore_state(&self, full: &FullSaveState) {
        if let Some(clock) = &self.clock {
            clock.store(full.clock_cycles);
        }
        if let Some(ref progress) = self.ppu_progress {
            progress.set(full.clock_cycles);
        }
        self.bus.load_state_components([
            full.cpu.clone(),
            full.ppu.clone(),
            full.wram.clone(),
            full.cartridge.clone(),
            full.timer.clone(),
            full.apu.clone(),
        ]);
    }

    pub fn save_state(&self, slot: u8) -> Result<(), String> {
        let path = self
            .state_path(slot)
            .ok_or_else(|| "no ROM path configured; cannot determine save state path".to_owned())?;

        let full = self.capture_state()?;

        let bytes =
            bincode::serialize(&full).map_err(|e| format!("failed to serialize save state: {e}"))?;
        std::fs::write(&path, &bytes)
            .map_err(|e| format!("failed to write save state to {}: {e}", path.display()))?;

        Ok(())
    }

    pub fn load_state(&self, slot: u8) -> Result<(), String> {
        let path = self
            .state_path(slot)
            .ok_or_else(|| "no ROM path configured; cannot determine save state path".to_owned())?;

        let bytes = std::fs::read(&path)
            .map_err(|e| format!("failed to read save state from {}: {e}", path.display()))?;
        let full: FullSaveState = bincode::deserialize(&bytes)
            .map_err(|e| format!("failed to deserialize save state: {e}"))?;

        self.restore_state(&full);

        Ok(())
    }

    /// Capture a snapshot into the rewind buffer.
    ///
    /// Pauses the CPU so the clock stops advancing, waits for all
    /// components to settle, then captures a consistent snapshot.
    /// Capture a snapshot into the rewind buffer.
    ///
    /// Pauses the CPU so the clock stops advancing, waits for all
    /// components to settle, then captures a consistent snapshot
    /// along with the current framebuffer for instant display during
    /// rewind playback.
    pub fn rewind_capture(&mut self, framebuffer: &[u8]) -> Result<(), String> {
        // Pause the CPU so the master clock stops advancing.  Other
        // clock-driven components (PPU, timer, APU) will finish their
        // current work up to the frozen clock target and then idle.
        self.pause();

        // Wait for the CPU to finish its current instruction quantum and
        // enter the pause loop, and for other components to drain to the
        // frozen clock target.  1ms is enough for a 4-instruction quantum
        // (~16 T-cycles) at 4 MHz, with margin.
        thread::sleep(Duration::from_millis(1));

        let result = self.capture_state();

        self.resume();

        let state = result?;
        self.rewind_buffer.push(RewindSnapshot {
            state,
            framebuffer: framebuffer.to_vec(),
        });
        Ok(())
    }

    /// Step one snapshot backwards in the rewind buffer and restore it.
    /// Returns the framebuffer to display, or `None` if the buffer is empty.
    pub fn rewind_step(&mut self) -> Option<Vec<u8>> {
        let snapshot = self.rewind_buffer.pop()?;
        self.restore_state(&snapshot.state);
        Some(snapshot.framebuffer)
    }

    /// Discard all rewind history (e.g. after reset or ROM load).
    pub fn rewind_clear(&mut self) {
        self.rewind_buffer.clear();
    }

    /// Number of rewind snapshots currently buffered.
    pub fn rewind_len(&self) -> usize {
        self.rewind_buffer.len()
    }

    // --- Headless agent API ---

    /// Get PPU OAM + LCD registers in a single command (no bus reads).
    pub fn ppu_features(&self) -> Option<component::PpuFeatures> {
        let rx = self.bus.request_ppu_features();
        // Brief poll — PPU should respond within a few ms.
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            if let Ok(features) = rx.try_recv() {
                return Some(features);
            }
            if std::time::Instant::now() > deadline {
                return None;
            }
            std::thread::yield_now();
        }
    }

    /// Set emulator speed multiplier. 1.0 = normal, 2.0 = double, 0.0 = uncapped.
    pub fn set_speed(&self, multiplier: f32) {
        let fixed = if multiplier <= 0.0 { 0 } else { (multiplier * 100.0) as u32 };
        self.speed.store(fixed, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn pause(&self) {
        self.paused.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn resume(&self) {
        self.paused.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn toggle_pause(&self) -> bool {
        let was_paused = self.paused.fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
        !was_paused // returns new paused state
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn reset(&mut self) -> Result<(), String> {
        self.stop();
        // Re-create from the same config, preserving the cartridge image.
        *self = Self::with_config(self.config.clone());
        self.start()
    }

    pub fn config(&self) -> &EmulatorConfig {
        &self.config
    }

    /// Mute audio by dropping the sample receiver. The APU's `try_send`
    /// silently fails on the disconnected channel, so no samples accumulate
    /// and the APU thread never blocks.
    pub fn mute_audio(&mut self) {
        self.sample_ready.take();
    }

    /// Drain any pending audio samples without playing them.
    /// Returns the number of samples drained.
    pub fn drain_audio(&mut self) -> usize {
        let mut count = 0;
        if let Some(rx) = &self.sample_ready {
            while rx.try_recv().is_ok() {
                count += 1;
            }
        }
        count
    }

    /// Block until the PPU has rendered at least `n` frames beyond the
    /// current count, draining reports along the way. Returns the snapshot
    /// after the target frame is reached.
    pub fn run_frames(&mut self, n: u64) -> Snapshot {
        let snap = self.snapshot();
        let target = snap.ppu_frames + n;
        loop {
            let snap = self.snapshot();
            if snap.ppu_frames >= target {
                return snap;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Read a contiguous range of memory addresses.
    /// Useful for bulk reads of VRAM (0x8000-0x9FFF), OAM (0xFE00-0xFE9F), etc.
    pub fn read_range(&self, start: u16, len: u16) -> Vec<u8> {
        let mut data = Vec::with_capacity(len as usize);
        for offset in 0..len {
            let addr = start.wrapping_add(offset);
            let mut pending = self.bus.read(addr);
            let read_start = std::time::Instant::now();
            loop {
                if let Some(result) = pending.try_take() {
                    data.push(match result {
                        ReadResult::Ready(v) => v,
                        ReadResult::NoData => 0xff,
                    });
                    break;
                }
                if read_start.elapsed() > Duration::from_secs(2) {
                    eprintln!("[read_range] TIMEOUT reading addr 0x{addr:04x} (offset {offset}/{len})");
                    data.push(0xff);
                    break;
                }
                std::thread::yield_now();
            }
        }
        data
    }

    /// Read the 40 OAM sprite entries as structured data.
    /// Each entry: (y, x, tile_index, attributes).
    pub fn oam_entries(&self) -> Vec<OamEntry> {
        let raw = self.read_range(0xfe00, 160);
        raw.chunks_exact(4)
            .map(|e| OamEntry {
                y: e[0],
                x: e[1],
                tile: e[2],
                flags: e[3],
            })
            .collect()
    }

    /// Read the background tile map (32x32 tile indices).
    /// `map_select`: false = 0x9800, true = 0x9C00 (LCDC bit 3).
    pub fn tile_map(&self, map_select: bool) -> Vec<u8> {
        let base = if map_select { 0x9c00 } else { 0x9800 };
        self.read_range(base, 1024)
    }

    /// Read the current LCD register values (LCDC, STAT, SCY, SCX, LY, LYC,
    /// DMA, BGP, OBP0, OBP1, WY, WX) as a compact struct.
    pub fn lcd_registers(&self) -> LcdRegisters {
        let raw = self.read_range(0xff40, 12);
        LcdRegisters {
            lcdc: raw[0],
            stat: raw[1],
            scy: raw[2],
            scx: raw[3],
            ly: raw[4],
            lyc: raw[5],
            dma: raw[6],
            bgp: raw[7],
            obp0: raw[8],
            obp1: raw[9],
            wy: raw[10],
            wx: raw[11],
        }
    }
}

/// A single OAM sprite entry.
#[derive(Clone, Copy, Debug, Default)]
pub struct OamEntry {
    pub y: u8,
    pub x: u8,
    pub tile: u8,
    pub flags: u8,
}

impl OamEntry {
    pub fn screen_x(&self) -> i16 { self.x as i16 - 8 }
    pub fn screen_y(&self) -> i16 { self.y as i16 - 16 }
    pub fn x_flip(&self) -> bool { self.flags & 0x20 != 0 }
    pub fn y_flip(&self) -> bool { self.flags & 0x40 != 0 }
    pub fn behind_bg(&self) -> bool { self.flags & 0x80 != 0 }
    pub fn cgb_palette(&self) -> u8 { self.flags & 0x07 }
    pub fn cgb_vram_bank(&self) -> u8 { (self.flags >> 3) & 1 }
}

/// LCD register snapshot.
#[derive(Clone, Copy, Debug, Default)]
pub struct LcdRegisters {
    pub lcdc: u8,
    pub stat: u8,
    pub scy: u8,
    pub scx: u8,
    pub ly: u8,
    pub lyc: u8,
    pub dma: u8,
    pub bgp: u8,
    pub obp0: u8,
    pub obp1: u8,
    pub wy: u8,
    pub wx: u8,
}

fn trace_report_drain(context: &str, duration: Duration, stats: ReportDrainStats) {
    if stats.total > 0
        && (duration >= Duration::from_micros(500) || stats.total >= 32)
    {
        tracing::debug!(
            context,
            delta_us = duration.as_micros() as u64,
            total = stats.total,
            cpu = stats.cpu,
            ppu = stats.ppu,
            "report drain"
        );
    }
}

fn trace_take_latest_frame(
    duration: Duration,
    _report_stats: ReportDrainStats,
    drained_frames: usize,
    _first_frame: Option<u64>,
    latest_frame: Option<u64>,
    latest_age: Option<Duration>,
) {
    let age_us = latest_age.map_or(0, |age| age.as_micros() as u64);
    if duration >= Duration::from_micros(500)
        || drained_frames > 1
        || latest_age.is_some_and(|age| age >= Duration::from_millis(2))
    {
        tracing::debug!(
            delta_us = duration.as_micros() as u64,
            frames = drained_frames,
            latest = latest_frame.unwrap_or(0),
            age_us,
            "take latest frame"
        );
    }
}

impl Default for Emulator {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Emulator {
    fn drop(&mut self) {
        self.stop();
    }
}

fn build_cpu_init_state(
    metadata: &CartridgeMetadata,
    boot_seed: u64,
    shared_cartridge: Option<SharedCartridgeReadState>,
) -> CpuInitState {
    let mut init_state = CpuInitState::default();
    init_state.hardware_mode = HardwareMode::Cgb;
    init_state.boot_target_mode = hardware_mode_for_boot(metadata.boot_mode);
    init_state.registers = CpuRegisters {
        pc: 0x0000,
        ..CpuRegisters::default()
    };

    fill_bytes(&mut init_state.hram, boot_seed ^ 0xcafe_f00d_1234_5678);
    init_state.interrupt_enable = 0x00;
    init_state.shared_cartridge = shared_cartridge;

    init_state
}

fn build_ppu_init_state(metadata: &CartridgeMetadata) -> PpuInitState {
    let mut init_state = PpuInitState::default();
    let _ = metadata;
    init_state.hardware_mode = HardwareMode::Cgb;
    init_state
}

fn build_timer_init_state(_metadata: &CartridgeMetadata) -> TimerInitState {
    TimerInitState {
        div: 0x00,
        tima: 0x00,
        tma: 0x00,
        tac: 0x00,
    }
}

fn build_wram_init_state(metadata: &CartridgeMetadata, boot_seed: u64) -> WramInitState {
    let mut init_state = WramInitState::default();
    let _ = metadata;
    init_state.hardware_mode = HardwareMode::Cgb;
    let mut rng = StdRng::seed_from_u64(boot_seed ^ 0x1234_5678_dead_beef);

    for bank in &mut init_state.banks {
        rng.fill_bytes(bank);
    }

    init_state
}

fn hardware_mode_for_boot(boot_mode: BootMode) -> HardwareMode {
    match boot_mode {
        BootMode::Dmg => HardwareMode::DmgCompatibility,
        BootMode::CgbEnhanced | BootMode::CgbOnly => HardwareMode::Cgb,
        _ => HardwareMode::Cgb,
    }
}

fn default_boot_rom_path() -> PathBuf {
    cgb_boot_rom_path()
}

fn cgb_boot_rom_path() -> PathBuf {
    load_config_path("cgb_boot_rom").unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("data")
            .join("cgb_boot.bin")
    })
}

fn dmg_boot_rom_path() -> PathBuf {
    load_config_path("dmg_boot_rom").unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("data")
            .join("dmg_boot.bin")
    })
}

fn load_config_path(key: &str) -> Option<PathBuf> {
    let config_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.yaml");
    let contents = std::fs::read_to_string(config_path).ok()?;
    let config: std::collections::HashMap<String, String> =
        serde_yaml::from_str(&contents).ok()?;
    config.get(key).map(PathBuf::from)
}

fn fill_bytes<const N: usize>(buffer: &mut [u8; N], seed: u64) {
    let mut rng = StdRng::seed_from_u64(seed);
    rng.fill_bytes(buffer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::thread;
    use std::time::Duration;

    const ROM_BANK_SIZE: usize = 0x4000;
    const TEST_NINTENDO_LOGO: [u8; 48] = [
        0xce, 0xed, 0x66, 0x66, 0xcc, 0x0d, 0x00, 0x0b, 0x03, 0x73, 0x00, 0x83, 0x00, 0x0c, 0x00,
        0x0d, 0x00, 0x08, 0x11, 0x1f, 0x88, 0x89, 0x00, 0x0e, 0xdc, 0xcc, 0x6e, 0xe6, 0xdd, 0xdd,
        0xd9, 0x99, 0xbb, 0xbb, 0x67, 0x63, 0x6e, 0x0e, 0xec, 0xcc, 0xdd, 0xdc, 0x99, 0x9f, 0xbb,
        0xb9, 0x33, 0x3e,
    ];

    fn rom_bank_count_for_code(code: u8) -> usize {
        match code {
            0x00 => 2,
            0x01 => 4,
            0x02 => 8,
            0x03 => 16,
            _ => panic!("unsupported test ROM size code: {code:#04x}"),
        }
    }

    fn write_test_logo(rom: &mut [u8]) {
        rom[0x0104..0x0134].copy_from_slice(&TEST_NINTENDO_LOGO);
    }

    fn write_test_header_checksum(rom: &mut [u8]) {
        let checksum = rom[0x0134..=0x014c]
            .iter()
            .fold(0u8, |sum, value| sum.wrapping_sub(*value).wrapping_sub(1));
        rom[0x014d] = checksum;
    }

    fn build_test_rom(
        title: &str,
        cgb_flag: u8,
        sgb_flag: u8,
        cartridge_type: u8,
        rom_size_code: u8,
        ram_size_code: u8,
    ) -> Vec<u8> {
        let mut rom = vec![0; rom_bank_count_for_code(rom_size_code) * ROM_BANK_SIZE];

        for (bank_index, bank) in rom.chunks_exact_mut(ROM_BANK_SIZE).enumerate() {
            bank.fill((bank_index as u8).wrapping_mul(0x11));
        }

        write_test_logo(&mut rom);
        let title_bytes = title.as_bytes();
        let title_slice = &mut rom[0x134..0x143];
        title_slice.fill(0);
        for (destination, source) in title_slice.iter_mut().zip(title_bytes.iter().copied()) {
            *destination = source;
        }

        rom[0x100] = 0x76;
        rom[0x101] = 0x00;
        rom[0x143] = cgb_flag;
        rom[0x146] = sgb_flag;
        rom[0x147] = cartridge_type;
        rom[0x148] = rom_size_code;
        rom[0x149] = ram_size_code;
        write_test_header_checksum(&mut rom);
        rom
    }

    fn await_read(emulator: &Emulator, address: u16) -> ReadResult {
        let mut pending = emulator.read(address);

        for _ in 0..50 {
            if let Some(result) = pending.try_take() {
                return result;
            }

            thread::sleep(Duration::from_millis(1));
        }

        panic!("timed out waiting for read at {address:#06x}");
    }

    fn await_write(emulator: &Emulator, address: u16, value: u8) -> WriteResult {
        let mut pending = emulator.write(address, value);

        for _ in 0..50 {
            if let Some(result) = pending.try_take() {
                return result;
            }

            thread::sleep(Duration::from_millis(1));
        }

        panic!("timed out waiting for write at {address:#06x}");
    }

    fn wait_for_read<F>(emulator: &Emulator, address: u16, predicate: F) -> u8
    where
        F: Fn(u8) -> bool,
    {
        for _ in 0..200 {
            match await_read(emulator, address) {
                ReadResult::Ready(value) if predicate(value) => return value,
                _ => thread::sleep(Duration::from_millis(1)),
            }
        }

        panic!("timed out waiting for condition at {address:#06x}");
    }

    fn wait_for_snapshot<F>(emulator: &mut Emulator, predicate: F) -> Snapshot
    where
        F: Fn(&Snapshot) -> bool,
    {
        for _ in 0..200 {
            let snapshot = emulator.snapshot();
            if predicate(&snapshot) {
                return snapshot;
            }
            thread::sleep(Duration::from_millis(1));
        }

        panic!("timed out waiting for snapshot condition");
    }

    fn wait_for_boot_completion(emulator: &mut Emulator) {
        for _ in 0..5000 {
            if await_read(emulator, 0xff50) == ReadResult::Ready(0x01) {
                let _ = emulator.snapshot();
                return;
            }

            thread::sleep(Duration::from_millis(1));
        }

        panic!("timed out waiting for boot ROM unmap");
    }

    fn start_and_finish_boot(emulator: &mut Emulator) {
        emulator.start().unwrap();
        wait_for_boot_completion(emulator);
    }

    fn fast_delays() -> ThreadDelays {
        ThreadDelays {
            cpu: Duration::ZERO,
            ppu: Duration::ZERO,
            wram: Duration::ZERO,
            cartridge: Duration::ZERO,
            timer: Duration::ZERO,
            apu: Duration::ZERO,
        }
    }

    fn stop_boot_rom_path() -> std::path::PathBuf {
        let path = std::env::temp_dir().join("chudtendo-stop-boot.bin");
        let mut bytes = vec![0; 0x0900];
        bytes[0] = 0x10;
        bytes[1] = 0x00;
        fs::write(&path, bytes).expect("failed to write stop boot rom fixture");
        path
    }

    fn emulator_with_stopped_cpu() -> Emulator {
        let mut config = EmulatorConfig::default();
        config.delays = fast_delays();
        config.boot_rom_path = stop_boot_rom_path();
        Emulator::with_config(config)
    }

    #[test]
    fn rom_only_cartridge_keeps_fixed_bank_mapping() {
        let cartridge =
            CartridgeImage::from_bytes(build_test_rom("ROMONLY", 0xc0, 0x00, 0x00, 0x00, 0x00))
                .unwrap();
        let mut config = EmulatorConfig::with_cartridge(cartridge);
        config.delays = fast_delays();
        let mut emulator = Emulator::with_config(config);
        start_and_finish_boot(&mut emulator);

        assert_eq!(await_read(&emulator, 0x0000), ReadResult::Ready(0x00));
        assert_eq!(await_read(&emulator, 0x0200), ReadResult::Ready(0x00));
        assert_eq!(await_read(&emulator, 0xff50), ReadResult::Ready(0x01));
        assert_eq!(await_read(&emulator, 0x4000), ReadResult::Ready(0x11));
        assert_eq!(await_write(&emulator, 0x2000, 0x02), WriteResult::Accepted);
        assert_eq!(await_read(&emulator, 0x4000), ReadResult::Ready(0x11));
    }

    #[test]
    fn cpu_owns_hram_and_interrupt_registers() {
        let mut emulator = emulator_with_stopped_cpu();
        emulator.start().unwrap();

        assert_eq!(await_write(&emulator, 0xff80, 0x42), WriteResult::Accepted);
        assert_eq!(await_read(&emulator, 0xff80), ReadResult::Ready(0x42));
        assert_eq!(await_write(&emulator, 0xffff, 0x1f), WriteResult::Accepted);
        assert_eq!(await_read(&emulator, 0xffff), ReadResult::Ready(0x1f));
    }

    #[test]
    fn dmg_boot_rom_executes_from_reset_vector() {
        let cartridge =
            CartridgeImage::from_bytes(build_test_rom("DMGBOOT", 0x00, 0x00, 0x01, 0x01, 0x00))
                .unwrap();
        let mut config = EmulatorConfig::with_cartridge(cartridge);
        config.boot_seed = 1;
        config.delays = fast_delays();
        let mut emulator = Emulator::with_config(config);
        start_and_finish_boot(&mut emulator);

        let snapshot = emulator.snapshot();

        assert!(snapshot.cpu_steps > 0);
        assert!(snapshot.cpu_pc >= 0x0100);
        assert_eq!(await_read(&emulator, 0x0000), ReadResult::Ready(0x00));
        assert!(matches!(
            await_read(&emulator, 0xff00),
            ReadResult::Ready(value) if value & 0xcf == 0xcf
        ));
        assert_eq!(await_read(&emulator, 0xff0f), ReadResult::Ready(0xe1));
        assert_eq!(await_read(&emulator, 0xff40), ReadResult::Ready(0x91));
        assert!(matches!(
            await_read(&emulator, 0xff44),
            ReadResult::Ready(ly) if ly < 154
        ));
        assert_eq!(await_read(&emulator, 0xff47), ReadResult::Ready(0xfc));
        assert_eq!(await_read(&emulator, 0xff48), ReadResult::Ready(0x00));
        assert_eq!(await_read(&emulator, 0xff49), ReadResult::Ready(0x00));
        assert_eq!(await_read(&emulator, 0xff4f), ReadResult::Ready(0xff));
        assert_eq!(await_read(&emulator, 0xff50), ReadResult::Ready(0x01));
        assert_eq!(await_read(&emulator, 0xff70), ReadResult::Ready(0xff));
        assert_eq!(await_read(&emulator, 0xffff), ReadResult::Ready(0x00));

        let samples = [
            await_read(&emulator, 0xc000),
            await_read(&emulator, 0xc001),
            await_read(&emulator, 0xff80),
            await_read(&emulator, 0xff81),
        ];

        assert!(
            samples
                .iter()
                .any(|sample| *sample != ReadResult::Ready(0x00))
        );
    }

    #[test]
    fn wram_translates_echo_and_bank_select() {
        let mut emulator = emulator_with_stopped_cpu();
        emulator.start().unwrap();

        assert_eq!(await_write(&emulator, 0xc123, 0x44), WriteResult::Accepted);
        assert_eq!(await_read(&emulator, 0xe123), ReadResult::Ready(0x44));
        assert_eq!(await_write(&emulator, 0xd010, 0x33), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xff70, 0x02), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xd010, 0x99), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xff70, 0x01), WriteResult::Accepted);
        assert_eq!(await_read(&emulator, 0xd010), ReadResult::Ready(0x33));
        assert_eq!(await_write(&emulator, 0xff70, 0x02), WriteResult::Accepted);
        assert_eq!(await_read(&emulator, 0xd010), ReadResult::Ready(0x99));
    }

    #[test]
    fn ppu_owns_vram_oam_and_banked_registers() {
        let mut emulator = emulator_with_stopped_cpu();
        emulator.start().unwrap();

        assert_eq!(await_write(&emulator, 0xff4f, 0x01), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0x8001, 0xab), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xff4f, 0x00), WriteResult::Accepted);
        assert_eq!(await_read(&emulator, 0x8001), ReadResult::Ready(0x00));
        assert_eq!(await_write(&emulator, 0xff4f, 0x01), WriteResult::Accepted);
        assert_eq!(await_read(&emulator, 0x8001), ReadResult::Ready(0xab));
        assert_eq!(await_write(&emulator, 0xfe00, 0x55), WriteResult::Accepted);
        assert_eq!(await_read(&emulator, 0xfe00), ReadResult::Ready(0x55));
    }

    #[test]
    fn ppu_advances_ly_and_requests_vblank() {
        let mut emulator = emulator_with_stopped_cpu();
        emulator.start().unwrap();

        assert_eq!(await_write(&emulator, 0xff40, 0x91), WriteResult::Accepted);
        let ly = wait_for_read(&emulator, 0xff44, |value| value != 0);
        let interrupt_flags = wait_for_read(&emulator, 0xff0f, |value| value & 0x01 != 0);

        assert!(ly < 154);
        assert_eq!(interrupt_flags & 0x01, 0x01);
        assert!(matches!(
            await_read(&emulator, 0xff41),
            ReadResult::Ready(stat) if stat & 0x80 == 0x80
        ));
    }

    #[test]
    fn ppu_cgb_palette_ram_tracks_index_and_autoincrement() {
        let mut emulator = emulator_with_stopped_cpu();
        emulator.start().unwrap();

        assert_eq!(await_write(&emulator, 0xff68, 0x80), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xff69, 0x1f), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xff69, 0x00), WriteResult::Accepted);
        assert_eq!(await_read(&emulator, 0xff68), ReadResult::Ready(0x82));

        assert_eq!(await_write(&emulator, 0xff68, 0x00), WriteResult::Accepted);
        assert_eq!(await_read(&emulator, 0xff69), ReadResult::Ready(0x1f));
        assert_eq!(await_write(&emulator, 0xff68, 0x01), WriteResult::Accepted);
        assert_eq!(await_read(&emulator, 0xff69), ReadResult::Ready(0x00));
    }

    #[test]
    fn ppu_renders_cgb_palette_colors_before_dmg_handoff() {
        let mut emulator = emulator_with_stopped_cpu();
        emulator.start().unwrap();

        assert_eq!(await_write(&emulator, 0xff68, 0x8a), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xff69, 0x1f), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xff69, 0x00), WriteResult::Accepted);

        assert_eq!(await_write(&emulator, 0x8000, 0xff), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0x8001, 0x00), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0x9800, 0x00), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xff4f, 0x01), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0x9800, 0x01), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xff4f, 0x00), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xff40, 0x91), WriteResult::Accepted);

        let snapshot = wait_for_snapshot(&mut emulator, |snapshot| {
            snapshot.framebuffer[0..4] == [0xff, 0x00, 0x00, 0xff]
        });

        assert_eq!(&snapshot.framebuffer[0..4], &[0xff, 0x00, 0x00, 0xff]);
    }

    #[test]
    fn ppu_renders_background_tiles_into_framebuffer() {
        let mut emulator = emulator_with_stopped_cpu();
        emulator.start().unwrap();

        assert_eq!(await_write(&emulator, 0xff47, 0xe4), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0x8000, 0xff), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0x8001, 0xff), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0x9800, 0x00), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xff40, 0x91), WriteResult::Accepted);

        let snapshot = wait_for_snapshot(&mut emulator, |snapshot| {
            snapshot.framebuffer[0..4] == [0x00, 0x00, 0x00, 0xff]
        });

        assert_eq!(&snapshot.framebuffer[0..4], &[0x00, 0x00, 0x00, 0xff]);
    }

    #[test]
    fn ppu_renders_sprites_over_background() {
        let mut emulator = emulator_with_stopped_cpu();
        emulator.start().unwrap();

        assert_eq!(await_write(&emulator, 0xff47, 0xe4), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xff48, 0xe4), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0x9800, 0x00), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0x8010, 0xff), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0x8011, 0xff), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xfe00, 0x10), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xfe01, 0x08), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xfe02, 0x01), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xfe03, 0x00), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xff40, 0x93), WriteResult::Accepted);

        let snapshot = wait_for_snapshot(&mut emulator, |snapshot| {
            snapshot.framebuffer[0..4] == [0x00, 0x00, 0x00, 0xff]
        });

        assert_eq!(&snapshot.framebuffer[0..4], &[0x00, 0x00, 0x00, 0xff]);
    }

    #[test]
    fn ppu_ff46_dma_copies_source_bytes_into_oam() {
        let mut emulator = emulator_with_stopped_cpu();
        emulator.start().unwrap();

        for offset in 0..0x00a0u16 {
            assert_eq!(
                await_write(&emulator, 0xc000 + offset, offset.wrapping_mul(3) as u8),
                WriteResult::Accepted
            );
        }

        assert_eq!(await_write(&emulator, 0xff46, 0xc0), WriteResult::Accepted);

        let first = wait_for_read(&emulator, 0xfe00, |value| value == 0x00);
        let last = wait_for_read(&emulator, 0xfe9f, |value| value == 0xdd);

        assert_eq!(first, 0x00);
        assert_eq!(last, 0xdd);
    }

    #[test]
    fn disabling_lcd_resets_ly_and_blanks_framebuffer() {
        let mut emulator = emulator_with_stopped_cpu();
        emulator.start().unwrap();

        assert_eq!(await_write(&emulator, 0xff47, 0xe4), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0x8000, 0xff), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0x8001, 0xff), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0x9800, 0x00), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xff40, 0x91), WriteResult::Accepted);
        let _ = wait_for_snapshot(&mut emulator, |snapshot| {
            snapshot.framebuffer[0..4] == [0x00, 0x00, 0x00, 0xff]
        });

        assert_eq!(await_write(&emulator, 0xff40, 0x00), WriteResult::Accepted);
        let snapshot = wait_for_snapshot(&mut emulator, |snapshot| {
            snapshot.framebuffer[0..4] == [0xff, 0xff, 0xff, 0xff]
        });

        assert_eq!(await_read(&emulator, 0xff44), ReadResult::Ready(0x00));
        assert_eq!(&snapshot.framebuffer[0..4], &[0xff, 0xff, 0xff, 0xff]);
    }

    #[test]
    fn mbc1_cartridge_switches_rom_and_ram_banks() {
        let cartridge =
            CartridgeImage::from_bytes(build_test_rom("MBC1TEST", 0xc0, 0x00, 0x03, 0x01, 0x03))
                .unwrap();
        let mut config = EmulatorConfig::with_cartridge(cartridge);
        config.delays = fast_delays();
        let mut emulator = Emulator::with_config(config);
        start_and_finish_boot(&mut emulator);

        assert_eq!(await_read(&emulator, 0x4000), ReadResult::Ready(0x11));
        assert_eq!(await_write(&emulator, 0x2000, 0x02), WriteResult::Accepted);
        assert_eq!(await_read(&emulator, 0x4000), ReadResult::Ready(0x22));
        assert_eq!(await_write(&emulator, 0x0000, 0x0a), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xa123, 0x5c), WriteResult::Accepted);
        assert_eq!(await_read(&emulator, 0xa123), ReadResult::Ready(0x5c));
        assert_eq!(await_write(&emulator, 0x6000, 0x01), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0x4000, 0x01), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xa123, 0x82), WriteResult::Accepted);
        assert_eq!(await_read(&emulator, 0xa123), ReadResult::Ready(0x82));
        assert_eq!(await_write(&emulator, 0x4000, 0x00), WriteResult::Accepted);
        assert_eq!(await_read(&emulator, 0xa123), ReadResult::Ready(0x5c));
    }

    #[test]
    fn unsupported_boot_modes_fail_before_runtime() {
        let rom = build_test_rom("SGBTEST", 0x00, 0x03, 0x00, 0x00, 0x00);

        assert_eq!(
            Emulator::from_rom_bytes(rom).unwrap_err(),
            CartridgeLoadError::UnsupportedBootMode(BootMode::SgbStub)
        );
    }

    #[test]
    fn unsupported_mbc_types_fail_before_runtime() {
        let rom = build_test_rom("UNKTEST", 0xc0, 0x00, 0xbf, 0x01, 0x03);

        assert_eq!(
            Emulator::from_rom_bytes(rom).unwrap_err(),
            CartridgeLoadError::UnsupportedMbc(MbcKind::UnknownStub(0xbf))
        );
    }

    #[test]
    fn timer_owns_timer_registers_and_unused_space_returns_no_data() {
        let mut emulator = emulator_with_stopped_cpu();
        emulator.start().unwrap();

        assert_eq!(await_write(&emulator, 0xff06, 0x77), WriteResult::Accepted);
        assert_eq!(await_read(&emulator, 0xff06), ReadResult::Ready(0x77));
        assert_eq!(await_read(&emulator, 0xfea0), ReadResult::NoData);
        assert_eq!(await_write(&emulator, 0xfea0, 0x12), WriteResult::NoData);
    }

    #[test]
    fn timer_reloads_tima_and_requests_interrupt() {
        let mut emulator = emulator_with_stopped_cpu();
        emulator.start().unwrap();

        assert_eq!(await_write(&emulator, 0xff06, 0x77), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xff05, 0xff), WriteResult::Accepted);
        assert_eq!(await_write(&emulator, 0xff07, 0x04), WriteResult::Accepted);
        assert_eq!(await_read(&emulator, 0xff07), ReadResult::Ready(0xfc));

        let interrupt_flags = wait_for_read(&emulator, 0xff0f, |value| value & 0x04 != 0);
        let tima = await_read(&emulator, 0xff05);

        // TIMA reloads to TMA (0x77) on overflow, but the timer thread may
        // tick additional times before the read arrives. Accept TMA or above.
        match tima {
            ReadResult::Ready(v) => assert!(v >= 0x77, "TIMA {v:#04x} below TMA 0x77"),
            other => panic!("expected Ready, got {other:?}"),
        }
        assert_eq!(interrupt_flags & 0x04, 0x04);
    }
}
