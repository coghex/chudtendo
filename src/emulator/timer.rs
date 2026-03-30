use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::thread;

use serde::{Deserialize, Serialize};

use super::bus::Bus;
use super::component::{
    Command, ComponentReport, InterruptFlags, MasterClock, MemoryCommand, ReadResult,
    SharedTimerState, TimerInitState, TimerReport, WriteResult,
};

#[derive(Serialize, Deserialize)]
pub struct TimerSaveState {
    pub div: u8,
    pub tima: u8,
    pub tma: u8,
    pub tac: u8,
    pub ticks: u64,
    pub cycles: u64,
    pub system_counter: u16,
    pub pending_reload: bool,
}

const TIMER_CLOCKS_PER_SAMPLE: u16 = 4;
const TIMER_INTERRUPT_MASK: u8 = 0x04;
const TIMER_REPORT_INTERVAL: u64 = 1024;

#[derive(Debug)]
pub struct TimerThread {
    apu: Sender<Command>,
    interrupt_flags: InterruptFlags,
    clock: MasterClock,
    shared: SharedTimerState,
    div: u8,
    tima: u8,
    tma: u8,
    tac: u8,
    ticks: u64,
    cycles: u64,
    system_counter: u16,
    pending_reload: bool,
    reload_just_happened: bool,
}

impl TimerThread {
    pub fn spawn(
        init_state: TimerInitState,
        inbox: Receiver<Command>,
        bus: Bus,
        interrupt_flags: InterruptFlags,
        reports: Sender<ComponentReport>,
        clock: MasterClock,
        shared: SharedTimerState,
    ) -> thread::JoinHandle<()> {
        thread::Builder::new()
            .name("timer".to_owned())
            .spawn(move || {
                Self::from_init_state(init_state, bus, interrupt_flags, clock.clone(), shared)
                    .run(inbox, reports, clock)
            })
            .expect("failed to spawn timer thread")
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
            if self.cycles < target {
                self.step();
                self.cycles += TIMER_CLOCKS_PER_SAMPLE as u64;
                self.publish_shared();

                if self.ticks == 1 || self.ticks % TIMER_REPORT_INTERVAL == 0 {
                    let _ =
                        reports.send(ComponentReport::Timer(TimerReport { ticks: self.ticks }));
                }
            } else {
                thread::yield_now();
            }
        }
    }

    fn chase_clock(&mut self) {
        let target = self.clock.target();
        while self.cycles < target {
            self.step();
            self.cycles += TIMER_CLOCKS_PER_SAMPLE as u64;
        }
        self.publish_shared();
    }

    fn chase_clock_for_register_write(&mut self) {
        let target = self.clock.target().saturating_add(TIMER_CLOCKS_PER_SAMPLE as u64);
        while self.cycles < target {
            self.step();
            self.cycles += TIMER_CLOCKS_PER_SAMPLE as u64;
        }
        self.publish_shared();
    }

    fn service_inbox(&mut self, inbox: &Receiver<Command>) -> bool {
        loop {
            match inbox.try_recv() {
                Ok(Command::Memory(command)) => self.handle_memory(command),
                Ok(Command::DivApuEdge) => {}
                Ok(Command::SetHardwareMode(_)) => {}
                Ok(Command::SaveState(respond_to)) => {
                    let state = self.create_save_state();
                    let bytes = bincode::serialize(&state).unwrap_or_default();
                    let _ = respond_to.send(bytes);
                }
                Ok(Command::LoadState(bytes)) => {
                    if let Ok(state) = bincode::deserialize::<TimerSaveState>(&bytes) {
                        self.apply_save_state(state);
                    }
                }
                Ok(Command::RequestPpuFeatures(_)) => {} Ok(Command::Stop) => return false,
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
    }

    fn create_save_state(&self) -> TimerSaveState {
        TimerSaveState {
            div: self.div,
            tima: self.tima,
            tma: self.tma,
            tac: self.tac,
            ticks: self.ticks,
            cycles: self.cycles,
            system_counter: self.system_counter,
            pending_reload: self.pending_reload,
        }
    }

    fn apply_save_state(&mut self, state: TimerSaveState) {
        self.div = state.div;
        self.tima = state.tima;
        self.tma = state.tma;
        self.tac = state.tac;
        self.ticks = state.ticks;
        self.cycles = state.cycles;
        self.system_counter = state.system_counter;
        self.pending_reload = state.pending_reload;
        self.reload_just_happened = false;
        self.publish_shared();
    }

    fn handle_memory(&mut self, command: MemoryCommand) {
        match command {
            MemoryCommand::Read {
                address,
                respond_to,
            } => {
                self.chase_clock();
                let result = match address {
                    0xff04 => ReadResult::Ready(self.div),
                    0xff05 => ReadResult::Ready(self.tima),
                    0xff06 => ReadResult::Ready(self.tma),
                    0xff07 => ReadResult::Ready(0xf8 | self.tac),
                    _ => ReadResult::NoData,
                };
                let _ = respond_to.send(result);
            }
            MemoryCommand::Write {
                address,
                value,
                respond_to,
            } => {
                self.chase_clock_for_register_write();
                let result = match address {
                    0xff04 => {
                        if self.apply_counter_change(0, self.tac) {
                            self.signal_div_apu_edge();
                        }
                        WriteResult::Accepted
                    }
                    0xff05 => {
                        if !self.reload_just_happened {
                            self.pending_reload = false;
                            self.tima = value;
                        }
                        WriteResult::Accepted
                    }
                    0xff06 => {
                        self.tma = value;
                        if self.reload_just_happened {
                            self.tima = value;
                        }
                        WriteResult::Accepted
                    }
                    0xff07 => {
                        if self.apply_counter_change(self.system_counter, value & 0x07) {
                            self.signal_div_apu_edge();
                        }
                        WriteResult::Accepted
                    }
                    _ => WriteResult::NoData,
                };
                self.publish_shared();
                if let Some(respond_to) = respond_to {
                    let _ = respond_to.send(result);
                }
            }
        }
    }

    fn from_init_state(
        init_state: TimerInitState,
        bus: Bus,
        interrupt_flags: InterruptFlags,
        clock: MasterClock,
        shared: SharedTimerState,
    ) -> Self {
        let tac = init_state.tac & 0x07;
        let tima = init_state.tima;
        let tma = init_state.tma;
        shared.publish(init_state.div, tima, tma, tac, 0);
        let system_counter = (init_state.div as u16) << 8;
        Self {
            apu: bus.apu_sender(),
            interrupt_flags,
            clock,
            shared,
            div: init_state.div,
            tima,
            tma,
            tac,
            ticks: 0,
            cycles: 0,
            system_counter,
            pending_reload: false,
            reload_just_happened: false,
        }
    }

    fn step(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
        self.reload_just_happened = false;
        let reloading = self.pending_reload;

        let next_counter = self.system_counter.wrapping_add(TIMER_CLOCKS_PER_SAMPLE);
        if self.apply_counter_change(next_counter, self.tac) {
            self.signal_div_apu_edge();
        }

        if reloading {
            self.pending_reload = false;
            self.tima = self.tma;
            self.interrupt_flags.set(TIMER_INTERRUPT_MASK);
            self.reload_just_happened = true;
        }
    }

    fn publish_shared(&self) {
        self.shared.publish(self.div, self.tima, self.tma, self.tac, self.cycles);
    }

    fn signal_div_apu_edge(&self) {
        let _ = self.apu.send(Command::DivApuEdge);
    }

    fn apply_counter_change(&mut self, next_counter: u16, next_tac: u8) -> bool {
        let previous_signal = timer_signal(self.system_counter, self.tac);
        let previous_div_apu = div_apu_signal(self.system_counter);
        self.system_counter = next_counter;
        self.tac = next_tac & 0x07;
        self.div = (self.system_counter >> 8) as u8;
        let next_signal = timer_signal(self.system_counter, self.tac);
        let next_div_apu = div_apu_signal(self.system_counter);

        if previous_signal && !next_signal {
            self.increment_tima();
        }

        previous_div_apu && !next_div_apu
    }

    fn increment_tima(&mut self) {
        if self.pending_reload {
            return;
        }

        let (next_tima, overflowed) = self.tima.overflowing_add(1);
        self.tima = next_tima;

        if overflowed {
            self.tima = 0x00;
            self.pending_reload = true;
        }
    }
}

fn timer_signal(counter: u16, tac: u8) -> bool {
    if tac & 0x04 == 0 {
        return false;
    }

    let bit_index = match tac & 0x03 {
        0x00 => 9,
        0x01 => 3,
        0x02 => 5,
        0x03 => 7,
        _ => unreachable!(),
    };

    counter & (1 << bit_index) != 0
}

fn div_apu_signal(counter: u16) -> bool {
    counter & (1 << 12) != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn test_timer(apu: Sender<Command>) -> TimerThread {
        let (cpu_tx, _cpu_rx) = mpsc::channel();
        let (ppu_tx, _ppu_rx) = mpsc::channel();
        let (wram_tx, _wram_rx) = mpsc::channel();
        let (cart_tx, _cart_rx) = mpsc::channel();
        let (timer_tx, _timer_rx) = mpsc::channel();
        let bus = Bus::new(cpu_tx, ppu_tx, wram_tx, cart_tx, timer_tx, apu);

        TimerThread::from_init_state(
            TimerInitState::default(),
            bus,
            InterruptFlags::new(),
            MasterClock::new(),
            SharedTimerState::new(),
        )
    }

    #[test]
    fn writing_div_with_bit4_high_emits_div_apu_edge() {
        let (apu_tx, apu_rx) = mpsc::channel();
        let mut timer = test_timer(apu_tx);
        timer.system_counter = 0x1000;
        timer.div = 0x10;

        timer.handle_memory(MemoryCommand::Write {
            address: 0xff04,
            value: 0x00,
            respond_to: None,
        });

        assert!(matches!(apu_rx.try_recv(), Ok(Command::DivApuEdge)));
        assert_eq!(timer.system_counter, 0);
    }
}
