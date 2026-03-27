use std::sync::mpsc::{Receiver, Sender, SyncSender, TryRecvError};
use std::thread;

use serde::{Deserialize, Serialize};

use super::component::{
    ApuReport, CPU_CLOCK_HZ, Command, ComponentReport, HardwareMode, MasterClock, MemoryCommand,
    ReadResult, SharedApuState, WriteResult,
};

#[derive(Serialize, Deserialize)]
pub struct PulseChannelSave {
    pub enabled: bool,
    pub dac_enabled: bool,
    pub duty: u8,
    pub duty_position: u8,
    pub length_timer: u16,
    pub length_enabled: bool,
    pub volume: u8,
    pub envelope_direction: bool,
    pub envelope_pace: u8,
    pub envelope_timer: u8,
    pub period: u16,
    pub period_timer: u16,
    pub sweep_enabled: bool,
    pub sweep_pace: u8,
    pub sweep_direction: bool,
    pub sweep_step: u8,
    pub sweep_timer: u8,
    pub sweep_shadow: u16,
    pub sweep_negate_used: bool,
}

#[derive(Serialize, Deserialize)]
pub struct WaveChannelSave {
    pub enabled: bool,
    pub dac_enabled: bool,
    pub length_timer: u16,
    pub length_enabled: bool,
    pub output_level: u8,
    pub period: u16,
    pub period_timer: u16,
    pub sample_index: u8,
    pub sample_buffer: u8,
    pub just_accessed_ram: bool,
    pub last_access_cycle: u64,
    pub last_access_sample_index: u8,
}

#[derive(Serialize, Deserialize)]
pub struct NoiseChannelSave {
    pub enabled: bool,
    pub dac_enabled: bool,
    pub length_timer: u16,
    pub length_enabled: bool,
    pub volume: u8,
    pub envelope_direction: bool,
    pub envelope_pace: u8,
    pub envelope_timer: u8,
    pub clock_shift: u8,
    pub width_mode: bool,
    pub divisor_code: u8,
    pub lfsr: u16,
    pub period_timer: u16,
}

#[derive(Serialize, Deserialize)]
pub struct ApuSaveState {
    pub master_enable: bool,
    pub nr50: u8,
    pub nr51: u8,
    pub registers: Vec<u8>,
    pub channel1: PulseChannelSave,
    pub channel2: PulseChannelSave,
    pub channel3: WaveChannelSave,
    pub channel4: NoiseChannelSave,
    pub wave_ram: Vec<u8>,
    pub frame_sequencer_step: u8,
    pub frame_sequencer_counter: u32,
    pub sample_counter: u32,
    pub samples_emitted: u64,
    pub cycles: u64,
    pub hardware_mode: HardwareMode,
}

const CPU_CLOCK: u32 = CPU_CLOCK_HZ as u32;
const SAMPLE_RATE: u32 = 48_000;
const FRAME_SEQUENCER_PERIOD: u32 = 8192;
const APU_REPORT_INTERVAL: u64 = 4096; // ~0.98ms worth of T-cycles

const DUTY_TABLE: [[u8; 8]; 4] = [
    [0, 0, 0, 0, 0, 0, 0, 1], // 12.5%
    [1, 0, 0, 0, 0, 0, 0, 1], // 25%
    [1, 0, 0, 0, 0, 1, 1, 1], // 50%
    [0, 1, 1, 1, 1, 1, 1, 0], // 75%
];

// Read masks: bits that read as 1 for write-only / unused positions.
const READ_MASKS: [u8; 0x17] = [
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

const NOISE_DIVISORS: [u16; 8] = [8, 16, 32, 48, 64, 80, 96, 112];

pub struct ApuThread {
    clock: MasterClock,
    cycles: u64,
    hardware_mode: super::component::HardwareMode,
    shared: SharedApuState,
    master_enable: bool,
    nr50: u8,
    nr51: u8,
    registers: [u8; 0x17], // FF10-FF26 raw storage for reads
    channel1: PulseChannel,
    channel2: PulseChannel,
    channel3: WaveChannel,
    channel4: NoiseChannel,
    wave_ram: [u8; 16],
    frame_sequencer_step: u8,
    frame_sequencer_counter: u32,
    sample_counter: u32,
    samples_emitted: u64,
    sample_sender: SyncSender<[f32; 2]>,
    /// First-order high-pass filter state (capacitor coupling).
    /// Removes DC offset, matching the Game Boy's analog output stage.
    hpf_left: f32,
    hpf_right: f32,
}

struct PulseChannel {
    enabled: bool,
    dac_enabled: bool,
    duty: u8,
    duty_position: u8,
    length_timer: u16,
    length_enabled: bool,
    volume: u8,
    envelope_direction: bool,
    envelope_pace: u8,
    envelope_timer: u8,
    period: u16,
    period_timer: u16,
    sweep_enabled: bool,
    sweep_pace: u8,
    sweep_direction: bool,
    sweep_step: u8,
    sweep_timer: u8,
    sweep_shadow: u16,
    sweep_negate_used: bool,
}

struct WaveChannel {
    enabled: bool,
    dac_enabled: bool,
    length_timer: u16,
    length_enabled: bool,
    output_level: u8,
    period: u16,
    period_timer: u16,
    sample_index: u8,
    sample_buffer: u8,
    just_accessed_ram: bool,
    last_access_cycle: u64,
    last_access_sample_index: u8,
}

struct NoiseChannel {
    enabled: bool,
    dac_enabled: bool,
    length_timer: u16,
    length_enabled: bool,
    volume: u8,
    envelope_direction: bool,
    envelope_pace: u8,
    envelope_timer: u8,
    clock_shift: u8,
    width_mode: bool,
    divisor_code: u8,
    lfsr: u16,
    period_timer: u16,
}

impl ApuThread {
    pub fn spawn(
        inbox: Receiver<Command>,
        reports: Sender<ComponentReport>,
        sample_sender: SyncSender<[f32; 2]>,
        clock: MasterClock,
        hardware_mode: super::component::HardwareMode,
        shared: SharedApuState,
    ) -> thread::JoinHandle<()> {
        thread::Builder::new()
            .name("apu".to_owned())
            .spawn(move || {
                let mut apu = Self::new(sample_sender, clock.clone(), shared);
                apu.hardware_mode = hardware_mode;
                apu.run(inbox, reports, clock)
            })
            .expect("failed to spawn apu thread")
    }

    fn new(sample_sender: SyncSender<[f32; 2]>, clock: MasterClock, shared: SharedApuState) -> Self {
        Self {
            clock,
            cycles: 0,
            hardware_mode: super::component::HardwareMode::Cgb,
            shared,
            master_enable: false,
            nr50: 0,
            nr51: 0,
            registers: [0; 0x17],
            channel1: PulseChannel::new(),
            channel2: PulseChannel::new(),
            channel3: WaveChannel::new(),
            channel4: NoiseChannel::new(),
            wave_ram: [0; 16],
            frame_sequencer_step: 0,
            frame_sequencer_counter: 0,
            sample_counter: 0,
            samples_emitted: 0,
            sample_sender,
            hpf_left: 0.0,
            hpf_right: 0.0,
        }
    }

    fn run(
        mut self,
        inbox: Receiver<Command>,
        reports: Sender<ComponentReport>,
        _clock: MasterClock,
    ) {
        loop {
            match self.service_inbox(&inbox) {
                InboxResult::Continue => {}
                InboxResult::Stop => break,
            }
            // Independently chase close to the master clock so audio samples
            // are generated at a steady rate.  Stop 8 cycles short so the
            // wave-specific offset chases in handle_memory still have room
            // to position the APU precisely for DMG wave access timing.
            let target = self.clock.target().saturating_sub(8);
            while self.cycles < target {
                if self.master_enable {
                    self.tick();
                }
                self.cycles += 1;
            }
            thread::yield_now();
        }
    }

    fn service_inbox(&mut self, inbox: &Receiver<Command>) -> InboxResult {
        loop {
            match inbox.try_recv() {
                Ok(Command::Memory(command)) => self.handle_memory(command),
                Ok(Command::SetHardwareMode(mode)) => self.hardware_mode = mode,
                Ok(Command::SaveState(respond_to)) => {
                    let state = self.create_save_state();
                    let bytes = bincode::serialize(&state).unwrap_or_default();
                    let _ = respond_to.send(bytes);
                }
                Ok(Command::LoadState(bytes)) => {
                    if let Ok(state) = bincode::deserialize::<ApuSaveState>(&bytes) {
                        self.apply_save_state(state);
                    }
                }
                Ok(Command::RequestPpuFeatures(_)) => {} Ok(Command::Stop) => return InboxResult::Stop,
                Err(TryRecvError::Empty) => return InboxResult::Continue,
                Err(TryRecvError::Disconnected) => return InboxResult::Stop,
            }
        }
    }

    fn create_save_state(&self) -> ApuSaveState {
        ApuSaveState {
            master_enable: self.master_enable,
            nr50: self.nr50,
            nr51: self.nr51,
            registers: self.registers.to_vec(),
            channel1: PulseChannelSave {
                enabled: self.channel1.enabled,
                dac_enabled: self.channel1.dac_enabled,
                duty: self.channel1.duty,
                duty_position: self.channel1.duty_position,
                length_timer: self.channel1.length_timer,
                length_enabled: self.channel1.length_enabled,
                volume: self.channel1.volume,
                envelope_direction: self.channel1.envelope_direction,
                envelope_pace: self.channel1.envelope_pace,
                envelope_timer: self.channel1.envelope_timer,
                period: self.channel1.period,
                period_timer: self.channel1.period_timer,
                sweep_enabled: self.channel1.sweep_enabled,
                sweep_pace: self.channel1.sweep_pace,
                sweep_direction: self.channel1.sweep_direction,
                sweep_step: self.channel1.sweep_step,
                sweep_timer: self.channel1.sweep_timer,
                sweep_shadow: self.channel1.sweep_shadow,
                sweep_negate_used: self.channel1.sweep_negate_used,
            },
            channel2: PulseChannelSave {
                enabled: self.channel2.enabled,
                dac_enabled: self.channel2.dac_enabled,
                duty: self.channel2.duty,
                duty_position: self.channel2.duty_position,
                length_timer: self.channel2.length_timer,
                length_enabled: self.channel2.length_enabled,
                volume: self.channel2.volume,
                envelope_direction: self.channel2.envelope_direction,
                envelope_pace: self.channel2.envelope_pace,
                envelope_timer: self.channel2.envelope_timer,
                period: self.channel2.period,
                period_timer: self.channel2.period_timer,
                sweep_enabled: false,
                sweep_pace: 0,
                sweep_direction: false,
                sweep_step: 0,
                sweep_timer: 0,
                sweep_shadow: 0,
                sweep_negate_used: false,
            },
            channel3: WaveChannelSave {
                enabled: self.channel3.enabled,
                dac_enabled: self.channel3.dac_enabled,
                length_timer: self.channel3.length_timer,
                length_enabled: self.channel3.length_enabled,
                output_level: self.channel3.output_level,
                period: self.channel3.period,
                period_timer: self.channel3.period_timer,
                sample_index: self.channel3.sample_index,
                sample_buffer: self.channel3.sample_buffer,
                just_accessed_ram: self.channel3.just_accessed_ram,
                last_access_cycle: self.channel3.last_access_cycle,
                last_access_sample_index: self.channel3.last_access_sample_index,
            },
            channel4: NoiseChannelSave {
                enabled: self.channel4.enabled,
                dac_enabled: self.channel4.dac_enabled,
                length_timer: self.channel4.length_timer,
                length_enabled: self.channel4.length_enabled,
                volume: self.channel4.volume,
                envelope_direction: self.channel4.envelope_direction,
                envelope_pace: self.channel4.envelope_pace,
                envelope_timer: self.channel4.envelope_timer,
                clock_shift: self.channel4.clock_shift,
                width_mode: self.channel4.width_mode,
                divisor_code: self.channel4.divisor_code,
                lfsr: self.channel4.lfsr,
                period_timer: self.channel4.period_timer,
            },
            wave_ram: self.wave_ram.to_vec(),
            frame_sequencer_step: self.frame_sequencer_step,
            frame_sequencer_counter: self.frame_sequencer_counter,
            sample_counter: self.sample_counter,
            samples_emitted: self.samples_emitted,
            cycles: self.cycles,
            hardware_mode: self.hardware_mode,
        }
    }

    fn apply_save_state(&mut self, state: ApuSaveState) {
        self.master_enable = state.master_enable;
        self.nr50 = state.nr50;
        self.nr51 = state.nr51;
        let len = state.registers.len().min(self.registers.len());
        self.registers[..len].copy_from_slice(&state.registers[..len]);

        self.channel1.enabled = state.channel1.enabled;
        self.channel1.dac_enabled = state.channel1.dac_enabled;
        self.channel1.duty = state.channel1.duty;
        self.channel1.duty_position = state.channel1.duty_position;
        self.channel1.length_timer = state.channel1.length_timer;
        self.channel1.length_enabled = state.channel1.length_enabled;
        self.channel1.volume = state.channel1.volume;
        self.channel1.envelope_direction = state.channel1.envelope_direction;
        self.channel1.envelope_pace = state.channel1.envelope_pace;
        self.channel1.envelope_timer = state.channel1.envelope_timer;
        self.channel1.period = state.channel1.period;
        self.channel1.period_timer = state.channel1.period_timer;
        self.channel1.sweep_enabled = state.channel1.sweep_enabled;
        self.channel1.sweep_pace = state.channel1.sweep_pace;
        self.channel1.sweep_direction = state.channel1.sweep_direction;
        self.channel1.sweep_step = state.channel1.sweep_step;
        self.channel1.sweep_timer = state.channel1.sweep_timer;
        self.channel1.sweep_shadow = state.channel1.sweep_shadow;
        self.channel1.sweep_negate_used = state.channel1.sweep_negate_used;

        self.channel2.enabled = state.channel2.enabled;
        self.channel2.dac_enabled = state.channel2.dac_enabled;
        self.channel2.duty = state.channel2.duty;
        self.channel2.duty_position = state.channel2.duty_position;
        self.channel2.length_timer = state.channel2.length_timer;
        self.channel2.length_enabled = state.channel2.length_enabled;
        self.channel2.volume = state.channel2.volume;
        self.channel2.envelope_direction = state.channel2.envelope_direction;
        self.channel2.envelope_pace = state.channel2.envelope_pace;
        self.channel2.envelope_timer = state.channel2.envelope_timer;
        self.channel2.period = state.channel2.period;
        self.channel2.period_timer = state.channel2.period_timer;

        self.channel3.enabled = state.channel3.enabled;
        self.channel3.dac_enabled = state.channel3.dac_enabled;
        self.channel3.length_timer = state.channel3.length_timer;
        self.channel3.length_enabled = state.channel3.length_enabled;
        self.channel3.output_level = state.channel3.output_level;
        self.channel3.period = state.channel3.period;
        self.channel3.period_timer = state.channel3.period_timer;
        self.channel3.sample_index = state.channel3.sample_index;
        self.channel3.sample_buffer = state.channel3.sample_buffer;
        self.channel3.just_accessed_ram = state.channel3.just_accessed_ram;
        self.channel3.last_access_cycle = state.channel3.last_access_cycle;
        self.channel3.last_access_sample_index = state.channel3.last_access_sample_index;

        self.channel4.enabled = state.channel4.enabled;
        self.channel4.dac_enabled = state.channel4.dac_enabled;
        self.channel4.length_timer = state.channel4.length_timer;
        self.channel4.length_enabled = state.channel4.length_enabled;
        self.channel4.volume = state.channel4.volume;
        self.channel4.envelope_direction = state.channel4.envelope_direction;
        self.channel4.envelope_pace = state.channel4.envelope_pace;
        self.channel4.envelope_timer = state.channel4.envelope_timer;
        self.channel4.clock_shift = state.channel4.clock_shift;
        self.channel4.width_mode = state.channel4.width_mode;
        self.channel4.divisor_code = state.channel4.divisor_code;
        self.channel4.lfsr = state.channel4.lfsr;
        self.channel4.period_timer = state.channel4.period_timer;

        let len = state.wave_ram.len().min(self.wave_ram.len());
        self.wave_ram[..len].copy_from_slice(&state.wave_ram[..len]);
        self.frame_sequencer_step = state.frame_sequencer_step;
        self.frame_sequencer_counter = state.frame_sequencer_counter;
        self.sample_counter = state.sample_counter;
        self.samples_emitted = state.samples_emitted;
        self.cycles = state.cycles;
        self.hardware_mode = state.hardware_mode;
        self.sync_shared();
    }

    fn chase_clock(&mut self) {
        let target = self.clock.target();
        while self.cycles < target {
            if self.master_enable {
                self.tick();
            }
            self.cycles += 1;
        }
    }

    fn chase_clock_for_wave_read(&mut self) {
        let target = self.clock.target().saturating_sub(6);
        while self.cycles < target {
            if self.master_enable {
                self.tick();
            }
            self.cycles += 1;
        }
    }

    fn chase_clock_for_wave_write(&mut self) {
        let target = self.clock.target().saturating_sub(4);
        while self.cycles < target {
            if self.master_enable {
                self.tick();
            }
            self.cycles += 1;
        }
    }

    fn handle_memory(&mut self, command: MemoryCommand) {
        match command {
            MemoryCommand::Read {
                address,
                respond_to,
            } => {
                if matches!(address, 0xff30..=0xff3f)
                    && matches!(
                        self.hardware_mode,
                        super::component::HardwareMode::DmgCompatibility
                    )
                {
                    self.chase_clock_for_wave_read();
                } else {
                    self.chase_clock();
                }
                let result = self.read_register(address);
                let _ = respond_to.send(result);
            }
            MemoryCommand::Write {
                address,
                value,
                respond_to,
            } => {
                if matches!(address, 0xff30..=0xff3f)
                    && matches!(
                        self.hardware_mode,
                        super::component::HardwareMode::DmgCompatibility
                    )
                {
                    self.chase_clock_for_wave_write();
                } else {
                    self.chase_clock();
                }
                self.write_register(address, value);
                self.sync_shared();
                if let Some(respond_to) = respond_to {
                    let _ = respond_to.send(WriteResult::Accepted);
                }
            }
        }
    }

    fn read_register(&self, address: u16) -> ReadResult {
        match address {
            0xff10..=0xff26 => {
                let index = (address - 0xff10) as usize;
                if address == 0xff26 {
                    let status = 0x70
                        | if self.master_enable { 0x80 } else { 0x00 }
                        | if self.channel1.enabled { 0x01 } else { 0x00 }
                        | if self.channel2.enabled { 0x02 } else { 0x00 }
                        | if self.channel3.enabled { 0x04 } else { 0x00 }
                        | if self.channel4.enabled { 0x08 } else { 0x00 };
                    ReadResult::Ready(status)
                } else {
                    ReadResult::Ready(self.registers[index] | READ_MASKS[index])
                }
            }
            0xff27..=0xff2f => ReadResult::Ready(0xff),
            0xff30..=0xff3f => {
                if self.channel3.enabled {
                    if matches!(self.hardware_mode, super::component::HardwareMode::DmgCompatibility) {
                        // DMG: CPU can only read wave RAM on the cycle CH3 accesses it.
                        // Approximate by checking if the period timer just expired.
                        if self.channel3.period_timer <= 2 {
                            let idx = (self.channel3.sample_index.wrapping_add(1)) & 31;
                            ReadResult::Ready(self.wave_ram[(idx / 2) as usize])
                        } else {
                            ReadResult::Ready(0xff)
                        }
                    } else {
                        // CGB: always returns the byte CH3 is currently reading.
                        ReadResult::Ready(
                            self.wave_ram[(self.channel3.sample_index / 2) as usize],
                        )
                    }
                } else {
                    ReadResult::Ready(self.wave_ram[(address - 0xff30) as usize])
                }
            }
            _ => ReadResult::NoData,
        }
    }

    fn write_register(&mut self, address: u16, value: u8) {
        match address {
            0xff26 => {
                let was_enabled = self.master_enable;
                self.master_enable = value & 0x80 != 0;
                if was_enabled && !self.master_enable {
                    self.power_off();
                } else if !was_enabled && self.master_enable {
                    self.frame_sequencer_counter = 0;
                    self.frame_sequencer_step = 0;
                }
            }
            0xff10..=0xff25 if !self.master_enable => {
                // On DMG, length registers are writable while APU is off.
                if matches!(
                    self.hardware_mode,
                    super::component::HardwareMode::DmgCompatibility
                ) {
                    match address {
                        0xff11 => {
                            self.channel1.length_timer = 64 - (value & 0x3f) as u16;
                        }
                        0xff16 => {
                            self.channel2.length_timer = 64 - (value & 0x3f) as u16;
                        }
                        0xff1b => {
                            self.channel3.length_timer = 256 - value as u16;
                        }
                        0xff20 => {
                            self.channel4.length_timer = 64 - (value & 0x3f) as u16;
                        }
                        _ => {}
                    }
                }
            }
            0xff10..=0xff14 => self.write_channel1(address, value),
            0xff15 => {}
            0xff16..=0xff19 => self.write_channel2(address, value),
            0xff1a..=0xff1e => self.write_channel3(address, value),
            0xff1f => {}
            0xff20..=0xff23 => self.write_channel4(address, value),
            0xff24 => {
                self.nr50 = value;
                self.registers[0x14] = value;
            }
            0xff25 => {
                self.nr51 = value;
                self.registers[0x15] = value;
            }
            0xff30..=0xff3f => {
                if self.channel3.enabled {
                    if matches!(
                        self.hardware_mode,
                        super::component::HardwareMode::DmgCompatibility
                    ) {
                        // DMG: write redirects to the byte CH3 is reading, but only
                        // during the brief cycle when wave RAM is being accessed.
                        let since_access = self.cycles.wrapping_sub(self.channel3.last_access_cycle);
                        if since_access <= 2 {
                            self.wave_ram[(self.channel3.sample_index / 2) as usize] = value;
                        }
                    } else {
                        // CGB: always writes to the byte CH3 is currently reading.
                        self.wave_ram[(self.channel3.sample_index / 2) as usize] = value;
                    }
                } else {
                    self.wave_ram[(address - 0xff30) as usize] = value;
                }
            }
            _ => {}
        }
    }

    /// Push current register state and NR52 status to the shared atomics
    /// so the CPU can read them without a channel round-trip.
    fn sync_shared(&self) {
        for i in 0..0x17 {
            self.shared.set_register(i, self.registers[i]);
        }
        let status = if self.master_enable { 0x80 } else { 0x00 }
            | if self.channel1.enabled { 0x01 } else { 0x00 }
            | if self.channel2.enabled { 0x02 } else { 0x00 }
            | if self.channel3.enabled { 0x04 } else { 0x00 }
            | if self.channel4.enabled { 0x08 } else { 0x00 };
        self.shared.set_nr52_status(status);
        self.shared.set_ch3_enabled(self.channel3.enabled);
        self.shared.wave_ram_mut_slice().copy_from_slice(&self.wave_ram);
    }

    fn write_channel1(&mut self, address: u16, value: u8) {
        let index = (address - 0xff10) as usize;
        self.registers[index] = value;

        match address {
            0xff10 => {
                self.channel1.sweep_pace = (value >> 4) & 0x07;
                let new_direction = value & 0x08 != 0;
                if self.channel1.sweep_negate_used && self.channel1.sweep_direction && !new_direction
                {
                    self.channel1.enabled = false;
                }
                self.channel1.sweep_direction = new_direction;
                self.channel1.sweep_step = value & 0x07;
            }
            0xff11 => {
                self.channel1.duty = (value >> 6) & 0x03;
                self.channel1.length_timer = 64 - (value & 0x3f) as u16;
            }
            0xff12 => {
                self.channel1.dac_enabled = value & 0xf8 != 0;
                if !self.channel1.dac_enabled {
                    self.channel1.enabled = false;
                }
                self.channel1.volume = value >> 4;
                self.channel1.envelope_direction = value & 0x08 != 0;
                self.channel1.envelope_pace = value & 0x07;
            }
            0xff13 => {
                self.channel1.period =
                    (self.channel1.period & 0x700) | value as u16;
            }
            0xff14 => {
                self.channel1.period =
                    (self.channel1.period & 0xff) | ((value as u16 & 0x07) << 8);
                let was_length_enabled = self.channel1.length_enabled;
                self.channel1.length_enabled = value & 0x40 != 0;
                if !was_length_enabled && self.channel1.length_enabled
                    && self.frame_sequencer_step % 2 == 1
                    && self.channel1.length_timer > 0
                {
                    self.channel1.length_timer -= 1;
                    if self.channel1.length_timer == 0 && value & 0x80 == 0 {
                        self.channel1.enabled = false;
                    }
                }
                if value & 0x80 != 0 {
                    self.trigger_channel1();
                }
            }
            _ => {}
        }
    }

    fn write_channel2(&mut self, address: u16, value: u8) {
        let index = (address - 0xff10) as usize;
        self.registers[index] = value;

        match address {
            0xff16 => {
                self.channel2.duty = (value >> 6) & 0x03;
                self.channel2.length_timer = 64 - (value & 0x3f) as u16;
            }
            0xff17 => {
                self.channel2.dac_enabled = value & 0xf8 != 0;
                if !self.channel2.dac_enabled {
                    self.channel2.enabled = false;
                }
                self.channel2.volume = value >> 4;
                self.channel2.envelope_direction = value & 0x08 != 0;
                self.channel2.envelope_pace = value & 0x07;
            }
            0xff18 => {
                self.channel2.period =
                    (self.channel2.period & 0x700) | value as u16;
            }
            0xff19 => {
                self.channel2.period =
                    (self.channel2.period & 0xff) | ((value as u16 & 0x07) << 8);
                let was_length_enabled = self.channel2.length_enabled;
                self.channel2.length_enabled = value & 0x40 != 0;
                if !was_length_enabled && self.channel2.length_enabled
                    && self.frame_sequencer_step % 2 == 1
                    && self.channel2.length_timer > 0
                {
                    self.channel2.length_timer -= 1;
                    if self.channel2.length_timer == 0 && value & 0x80 == 0 {
                        self.channel2.enabled = false;
                    }
                }
                if value & 0x80 != 0 {
                    self.trigger_channel2();
                }
            }
            _ => {}
        }
    }

    fn write_channel3(&mut self, address: u16, value: u8) {
        let index = (address - 0xff10) as usize;
        self.registers[index] = value;

        match address {
            0xff1a => {
                self.channel3.dac_enabled = value & 0x80 != 0;
                if !self.channel3.dac_enabled {
                    self.channel3.enabled = false;
                }
            }
            0xff1b => {
                self.channel3.length_timer = 256 - value as u16;
            }
            0xff1c => {
                self.channel3.output_level = (value >> 5) & 0x03;
            }
            0xff1d => {
                self.channel3.period =
                    (self.channel3.period & 0x700) | value as u16;
            }
            0xff1e => {
                self.channel3.period =
                    (self.channel3.period & 0xff) | ((value as u16 & 0x07) << 8);
                let was_length_enabled = self.channel3.length_enabled;
                self.channel3.length_enabled = value & 0x40 != 0;
                if !was_length_enabled && self.channel3.length_enabled
                    && self.frame_sequencer_step % 2 == 1
                    && self.channel3.length_timer > 0
                {
                    self.channel3.length_timer -= 1;
                    if self.channel3.length_timer == 0 && value & 0x80 == 0 {
                        self.channel3.enabled = false;
                    }
                }
                if value & 0x80 != 0 {
                    self.trigger_channel3();
                }
            }
            _ => {}
        }
    }

    fn write_channel4(&mut self, address: u16, value: u8) {
        let index = (address - 0xff10) as usize;
        self.registers[index] = value;

        match address {
            0xff20 => {
                self.channel4.length_timer = 64 - (value & 0x3f) as u16;
            }
            0xff21 => {
                self.channel4.dac_enabled = value & 0xf8 != 0;
                if !self.channel4.dac_enabled {
                    self.channel4.enabled = false;
                }
                self.channel4.volume = value >> 4;
                self.channel4.envelope_direction = value & 0x08 != 0;
                self.channel4.envelope_pace = value & 0x07;
            }
            0xff22 => {
                self.channel4.clock_shift = value >> 4;
                self.channel4.width_mode = value & 0x08 != 0;
                self.channel4.divisor_code = value & 0x07;
            }
            0xff23 => {
                let was_length_enabled = self.channel4.length_enabled;
                self.channel4.length_enabled = value & 0x40 != 0;
                if !was_length_enabled && self.channel4.length_enabled
                    && self.frame_sequencer_step % 2 == 1
                    && self.channel4.length_timer > 0
                {
                    self.channel4.length_timer -= 1;
                    if self.channel4.length_timer == 0 && value & 0x80 == 0 {
                        self.channel4.enabled = false;
                    }
                }
                if value & 0x80 != 0 {
                    self.trigger_channel4();
                }
            }
            _ => {}
        }
    }

    fn trigger_channel1(&mut self) {
        self.channel1.enabled = self.channel1.dac_enabled;
        if self.channel1.length_timer == 0 {
            self.channel1.length_timer = 64;
            if self.channel1.length_enabled && self.frame_sequencer_step % 2 == 1 {
                self.channel1.length_timer -= 1;
            }
        }
        self.channel1.period_timer = (2048 - self.channel1.period) * 4;
        self.channel1.volume = self.registers[0x02] >> 4;
        self.channel1.envelope_timer = if self.channel1.envelope_pace == 0 {
            8
        } else {
            self.channel1.envelope_pace
        };
        self.channel1.sweep_shadow = self.channel1.period;
        self.channel1.sweep_timer = if self.channel1.sweep_pace == 0 {
            8
        } else {
            self.channel1.sweep_pace
        };
        self.channel1.sweep_negate_used = false;
        self.channel1.sweep_enabled =
            self.channel1.sweep_pace != 0 || self.channel1.sweep_step != 0;
        if self.channel1.sweep_step != 0 {
            let check = self.sweep_calculate();
            if !self.channel1.sweep_direction && check > 0x7ff {
                self.channel1.enabled = false;
            }
        }
    }

    fn trigger_channel2(&mut self) {
        let ch = &mut self.channel2;
        ch.enabled = ch.dac_enabled;
        if ch.length_timer == 0 {
            ch.length_timer = 64;
            if ch.length_enabled && self.frame_sequencer_step % 2 == 1 {
                ch.length_timer -= 1;
            }
        }
        ch.period_timer = (2048 - ch.period) * 4;
        ch.volume = self.registers[0x07] >> 4;
        ch.envelope_timer = if ch.envelope_pace == 0 { 8 } else { ch.envelope_pace };
    }

    fn trigger_channel3(&mut self) {
        // DMG: retriggering while CH3 is active AND the timer is about to
        // expire (within 2 T-cycles of a wave RAM read) corrupts wave RAM.
        if matches!(
            self.hardware_mode,
            super::component::HardwareMode::DmgCompatibility
        ) && self.channel3.enabled
            && self.channel3.period_timer <= 3
        {
            let pos = (self.channel3.sample_index / 2) as usize;
            if pos < 4 {
                // Current position in first 4 bytes: copy current byte to byte 0.
                self.wave_ram[0] = self.wave_ram[pos];
            } else {
                // Current position beyond first 4 bytes: overwrite first 4 bytes
                // with 4 bytes from the aligned position.
                let aligned = pos & !3;
                self.wave_ram[0] = self.wave_ram[aligned];
                self.wave_ram[1] = self.wave_ram[aligned + 1];
                self.wave_ram[2] = self.wave_ram[aligned + 2];
                self.wave_ram[3] = self.wave_ram[aligned + 3];
            }
        }

        let ch = &mut self.channel3;
        ch.enabled = ch.dac_enabled;
        if ch.length_timer == 0 {
            ch.length_timer = 256;
            if ch.length_enabled && self.frame_sequencer_step % 2 == 1 {
                ch.length_timer -= 1;
            }
        }
        // Timer reloads on trigger. Offset compensates for the trigger
        // happening at the end of the write M-cycle. DMG and CGB differ.
        let trigger_delay = if matches!(
            self.hardware_mode,
            super::component::HardwareMode::DmgCompatibility
        ) {
            2
        } else {
            6
        };
        ch.period_timer = (2048 - ch.period) * 2 + trigger_delay;
        ch.sample_index = 0;
    }

    fn trigger_channel4(&mut self) {
        let ch = &mut self.channel4;
        ch.enabled = ch.dac_enabled;
        if ch.length_timer == 0 {
            ch.length_timer = 64;
            if ch.length_enabled && self.frame_sequencer_step % 2 == 1 {
                ch.length_timer -= 1;
            }
        }
        ch.lfsr = 0x7fff;
        ch.volume = self.registers[0x11] >> 4;
        ch.envelope_timer = if ch.envelope_pace == 0 { 8 } else { ch.envelope_pace };
        ch.period_timer = ch.noise_period();
    }

    fn power_off(&mut self) {
        let is_dmg = matches!(
            self.hardware_mode,
            super::component::HardwareMode::DmgCompatibility
        );

        // Save length timers. On DMG, ALL are preserved. On CGB, all except NR41.
        let ch1_length = self.channel1.length_timer;
        let ch2_length = self.channel2.length_timer;
        let ch3_length = self.channel3.length_timer;
        let ch4_length = self.channel4.length_timer;

        // Clear registers.
        for i in 0..0x16 {
            self.registers[i] = 0;
        }
        self.nr50 = 0;
        self.nr51 = 0;

        // Disable all channels.
        self.channel1.enabled = false;
        self.channel1.dac_enabled = false;
        self.channel2.enabled = false;
        self.channel2.dac_enabled = false;
        self.channel3.enabled = false;
        self.channel3.dac_enabled = false;
        self.channel4.enabled = false;
        self.channel4.dac_enabled = false;

        if !is_dmg {
            // CGB: reset all channel state except length timers.
            self.channel1 = PulseChannel::new();
            self.channel2 = PulseChannel::new();
            self.channel3 = WaveChannel::new();
            self.channel4 = NoiseChannel::new();

            self.channel1.length_timer = ch1_length;
            self.channel2.length_timer = ch2_length;
            self.channel3.length_timer = ch3_length;
            // CGB: NR41 length is cleared.
        } else {
            // DMG: disable channels and clear register-derived state.
            // Length timers are preserved.
            self.channel1 = PulseChannel::new();
            self.channel2 = PulseChannel::new();
            let ch3_len = ch3_length;
            self.channel3 = WaveChannel::new();
            self.channel4 = NoiseChannel::new();

            self.channel1.length_timer = ch1_length;
            self.channel2.length_timer = ch2_length;
            self.channel3.length_timer = ch3_len;
            self.channel4.length_timer = ch4_length;
        }
    }

    fn tick(&mut self) {
        self.frame_sequencer_counter += 1;
        if self.frame_sequencer_counter >= FRAME_SEQUENCER_PERIOD {
            self.frame_sequencer_counter = 0;
            self.clock_frame_sequencer();
            // Channel enabled flags may have changed (length counter expiry,
            // sweep overflow).  Update NR52 so the CPU sees current status.
            let status = if self.master_enable { 0x80 } else { 0x00 }
                | if self.channel1.enabled { 0x01 } else { 0x00 }
                | if self.channel2.enabled { 0x02 } else { 0x00 }
                | if self.channel3.enabled { 0x04 } else { 0x00 }
                | if self.channel4.enabled { 0x08 } else { 0x00 };
            self.shared.set_nr52_status(status);
            self.shared.set_ch3_enabled(self.channel3.enabled);
        }

        // Pulse channels: period timer ticks every 4 T-cycles.
        // We tick once per T-cycle but the period is pre-scaled by 4.
        self.channel1.tick_period();
        self.channel2.tick_period();

        // Wave channel: period is pre-scaled by 2 for the 2 MHz tick rate.
        self.channel3.tick_period(&self.wave_ram);
        if self.channel3.just_accessed_ram {
            self.channel3.last_access_cycle = self.cycles;
            self.channel3.last_access_sample_index = self.channel3.sample_index;
            self.channel3.just_accessed_ram = false;
        }

        // Noise channel: ticks at variable rate via period timer.
        self.channel4.tick_period();

        self.sample_counter += SAMPLE_RATE;
        if self.sample_counter >= CPU_CLOCK {
            self.sample_counter -= CPU_CLOCK;
            self.emit_sample();
        }
    }

    fn clock_frame_sequencer(&mut self) {
        match self.frame_sequencer_step {
            0 | 4 => self.clock_length(),
            2 | 6 => {
                self.clock_length();
                self.clock_sweep();
            }
            7 => self.clock_envelope(),
            _ => {}
        }
        self.frame_sequencer_step = (self.frame_sequencer_step + 1) & 7;
    }

    fn clock_length(&mut self) {
        tick_length(&mut self.channel1.length_timer, self.channel1.length_enabled, &mut self.channel1.enabled);
        tick_length(&mut self.channel2.length_timer, self.channel2.length_enabled, &mut self.channel2.enabled);
        tick_length(&mut self.channel3.length_timer, self.channel3.length_enabled, &mut self.channel3.enabled);
        tick_length(&mut self.channel4.length_timer, self.channel4.length_enabled, &mut self.channel4.enabled);
    }

    fn clock_envelope(&mut self) {
        tick_envelope(&mut self.channel1.volume, self.channel1.envelope_direction, self.channel1.envelope_pace, &mut self.channel1.envelope_timer);
        tick_envelope(&mut self.channel2.volume, self.channel2.envelope_direction, self.channel2.envelope_pace, &mut self.channel2.envelope_timer);
        tick_envelope(&mut self.channel4.volume, self.channel4.envelope_direction, self.channel4.envelope_pace, &mut self.channel4.envelope_timer);
    }

    fn clock_sweep(&mut self) {
        let ch = &mut self.channel1;
        if !ch.sweep_enabled {
            return;
        }
        ch.sweep_timer = ch.sweep_timer.saturating_sub(1);
        if ch.sweep_timer > 0 {
            return;
        }
        ch.sweep_timer = if ch.sweep_pace == 0 { 8 } else { ch.sweep_pace };
        if ch.sweep_pace == 0 {
            return;
        }

        let new_period = self.sweep_calculate();
        if !self.channel1.sweep_direction && new_period > 0x7ff {
            self.channel1.enabled = false;
            return;
        }
        if self.channel1.sweep_step != 0 {
            self.channel1.period = new_period & 0x7ff;
            self.channel1.sweep_shadow = new_period & 0x7ff;
            // Overflow check again with new value (addition only).
            let check = self.sweep_calculate();
            if !self.channel1.sweep_direction && check > 0x7ff {
                self.channel1.enabled = false;
            }
        }
    }

    fn sweep_calculate(&mut self) -> u16 {
        let shadow = self.channel1.sweep_shadow;
        let delta = shadow >> self.channel1.sweep_step;
        if self.channel1.sweep_direction {
            self.channel1.sweep_negate_used = true;
            // Two's complement: negate delta then add. Result wraps in 11-bit space.
            shadow.wrapping_add(!delta).wrapping_add(1)
        } else {
            shadow.wrapping_add(delta)
        }
    }

    fn emit_sample(&mut self) {
        let ch1 = self.channel1.output();
        let ch2 = self.channel2.output();
        let ch3 = self.channel3.output();
        let ch4 = self.channel4.output();

        let mut left: f32 = 0.0;
        let mut right: f32 = 0.0;

        if self.nr51 & 0x10 != 0 { left += ch1; }
        if self.nr51 & 0x20 != 0 { left += ch2; }
        if self.nr51 & 0x40 != 0 { left += ch3; }
        if self.nr51 & 0x80 != 0 { left += ch4; }

        if self.nr51 & 0x01 != 0 { right += ch1; }
        if self.nr51 & 0x02 != 0 { right += ch2; }
        if self.nr51 & 0x04 != 0 { right += ch3; }
        if self.nr51 & 0x08 != 0 { right += ch4; }

        let left_vol = ((self.nr50 >> 4) & 0x07) as f32 + 1.0;
        let right_vol = (self.nr50 & 0x07) as f32 + 1.0;

        left = left * 0.25 * (left_vol * 0.125);
        right = right * 0.25 * (right_vol * 0.125);

        // High-pass filter: models the Game Boy's capacitor-coupled output.
        // Removes DC offset that causes buzzing during quiet passages.
        // Charge factor ~0.998 gives a ~20 Hz cutoff at 48 kHz sample rate.
        const HPF_DECAY: f32 = 0.002; // 1.0 - 0.998 charge factor; ~20 Hz cutoff at 48 kHz
        let hpf_left_out = left - self.hpf_left;
        self.hpf_left += hpf_left_out * HPF_DECAY;
        let hpf_right_out = right - self.hpf_right;
        self.hpf_right += hpf_right_out * HPF_DECAY;

        self.samples_emitted += 1;
        let _ = self.sample_sender.try_send([hpf_left_out, hpf_right_out]);
    }
}

impl PulseChannel {
    fn new() -> Self {
        Self {
            enabled: false,
            dac_enabled: false,
            duty: 0,
            duty_position: 0,
            length_timer: 0,
            length_enabled: false,
            volume: 0,
            envelope_direction: false,
            envelope_pace: 0,
            envelope_timer: 0,
            period: 0,
            period_timer: 0,
            sweep_enabled: false,
            sweep_pace: 0,
            sweep_direction: false,
            sweep_step: 0,
            sweep_timer: 0,
            sweep_shadow: 0,
            sweep_negate_used: false,
        }
    }

    fn tick_period(&mut self) {
        if self.period_timer > 0 {
            self.period_timer -= 1;
        }
        if self.period_timer == 0 {
            self.period_timer = (2048 - self.period) * 4;
            self.duty_position = (self.duty_position + 1) & 7;
        }
    }

    fn output(&self) -> f32 {
        if !self.enabled || !self.dac_enabled {
            return 0.0;
        }
        let digital = if DUTY_TABLE[self.duty as usize][self.duty_position as usize] != 0 {
            self.volume
        } else {
            0
        };
        digital as f32 * (2.0 / 15.0) - 1.0
    }
}

impl WaveChannel {
    fn new() -> Self {
        Self {
            enabled: false,
            dac_enabled: false,
            length_timer: 0,
            length_enabled: false,
            output_level: 0,
            period: 0,
            period_timer: 0,
            sample_index: 0,
            sample_buffer: 0,
            just_accessed_ram: false,
            last_access_cycle: 0,
            last_access_sample_index: 0,
        }
    }

    fn tick_period(&mut self, wave_ram: &[u8; 16]) {
        if self.period_timer > 0 {
            self.period_timer -= 1;
        }
        if self.period_timer == 0 {
            self.period_timer = (2048 - self.period) * 2;
            self.sample_index = (self.sample_index + 1) & 31;
            let byte = wave_ram[(self.sample_index / 2) as usize];
            self.sample_buffer = if self.sample_index & 1 == 0 {
                byte >> 4
            } else {
                byte & 0x0f
            };
            self.just_accessed_ram = true;
        }
    }

    fn output(&self) -> f32 {
        if !self.enabled || !self.dac_enabled {
            return 0.0;
        }
        let shifted = match self.output_level {
            0 => 0,
            1 => self.sample_buffer,
            2 => self.sample_buffer >> 1,
            3 => self.sample_buffer >> 2,
            _ => 0,
        };
        shifted as f32 * (2.0 / 15.0) - 1.0
    }
}

impl NoiseChannel {
    fn new() -> Self {
        Self {
            enabled: false,
            dac_enabled: false,
            length_timer: 0,
            length_enabled: false,
            volume: 0,
            envelope_direction: false,
            envelope_pace: 0,
            envelope_timer: 0,
            clock_shift: 0,
            width_mode: false,
            divisor_code: 0,
            lfsr: 0x7fff,
            period_timer: 0,
        }
    }

    fn noise_period(&self) -> u16 {
        let divisor = NOISE_DIVISORS[self.divisor_code as usize & 7];
        divisor.checked_shl(self.clock_shift as u32).unwrap_or(0)
    }

    fn tick_period(&mut self) {
        if self.period_timer > 0 {
            self.period_timer -= 1;
        }
        if self.period_timer == 0 {
            self.period_timer = self.noise_period().max(1);
            let xor = (self.lfsr & 1) ^ ((self.lfsr >> 1) & 1);
            self.lfsr = (self.lfsr >> 1) | (xor << 14);
            if self.width_mode {
                self.lfsr = (self.lfsr & !0x40) | (xor << 6);
            }
        }
    }

    fn output(&self) -> f32 {
        if !self.enabled || !self.dac_enabled {
            return 0.0;
        }
        let digital = if self.lfsr & 1 == 0 { self.volume } else { 0 };
        digital as f32 * (2.0 / 15.0) - 1.0
    }
}

fn tick_length(length_timer: &mut u16, length_enabled: bool, enabled: &mut bool) {
    if !length_enabled || *length_timer == 0 {
        return;
    }
    *length_timer -= 1;
    if *length_timer == 0 {
        *enabled = false;
    }
}

fn tick_envelope(volume: &mut u8, direction: bool, pace: u8, timer: &mut u8) {
    if pace == 0 {
        return;
    }
    *timer = timer.saturating_sub(1);
    if *timer > 0 {
        return;
    }
    *timer = pace;
    if direction && *volume < 15 {
        *volume += 1;
    } else if !direction && *volume > 0 {
        *volume -= 1;
    }
}

enum InboxResult {
    Continue,
    Stop,
}
