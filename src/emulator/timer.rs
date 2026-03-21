use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::thread;

use super::bus::Bus;
use super::component::{
    Command, ComponentReport, InterruptFlags, MasterClock, MemoryCommand, ReadResult,
    TimerInitState, TimerReport, WriteResult,
};

const TIMER_CLOCKS_PER_SAMPLE: u16 = 4;
const TIMER_INTERRUPT_MASK: u8 = 0x04;
const TIMER_REPORT_INTERVAL: u64 = 1024;

#[derive(Debug)]
pub struct TimerThread {
    interrupt_flags: InterruptFlags,
    clock: MasterClock,
    div: u8,
    tima: u8,
    tma: u8,
    tac: u8,
    ticks: u64,
    cycles: u64,
    system_counter: u16,
    pending_reload: bool,
}

impl TimerThread {
    pub fn spawn(
        init_state: TimerInitState,
        inbox: Receiver<Command>,
        bus: Bus,
        interrupt_flags: InterruptFlags,
        reports: Sender<ComponentReport>,
        clock: MasterClock,
    ) -> thread::JoinHandle<()> {
        thread::Builder::new()
            .name("timer".to_owned())
            .spawn(move || {
                Self::from_init_state(init_state, bus, interrupt_flags, clock.clone())
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
    }

    fn service_inbox(&mut self, inbox: &Receiver<Command>) -> bool {
        loop {
            match inbox.try_recv() {
                Ok(Command::Memory(command)) => self.handle_memory(command),
                Ok(Command::SetHardwareMode(_)) => {}
                Ok(Command::Stop) => return false,
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
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
                self.chase_clock();
                let result = match address {
                    0xff04 => {
                        self.apply_counter_change(0, self.tac);
                        WriteResult::Accepted
                    }
                    0xff05 => {
                        self.pending_reload = false;
                        self.tima = value;
                        WriteResult::Accepted
                    }
                    0xff06 => {
                        self.tma = value;
                        WriteResult::Accepted
                    }
                    0xff07 => {
                        self.apply_counter_change(self.system_counter, value & 0x07);
                        WriteResult::Accepted
                    }
                    _ => WriteResult::NoData,
                };
                if let Some(respond_to) = respond_to {
                    let _ = respond_to.send(result);
                }
            }
        }
    }

    fn from_init_state(
        init_state: TimerInitState,
        _bus: Bus,
        interrupt_flags: InterruptFlags,
        clock: MasterClock,
    ) -> Self {
        Self {
            interrupt_flags,
            clock,
            div: 0,
            tima: init_state.tima,
            tma: init_state.tma,
            tac: init_state.tac & 0x07,
            ticks: 0,
            cycles: 0,
            system_counter: 0,
            pending_reload: false,
        }
    }

    fn step(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);

        let next_counter = self.system_counter.wrapping_add(TIMER_CLOCKS_PER_SAMPLE);
        self.apply_counter_change(next_counter, self.tac);

        if self.pending_reload {
            self.pending_reload = false;
            self.tima = self.tma;
            self.interrupt_flags.set(TIMER_INTERRUPT_MASK);
        }
    }

    fn apply_counter_change(&mut self, next_counter: u16, next_tac: u8) {
        let previous_signal = timer_signal(self.system_counter, self.tac);
        self.system_counter = next_counter;
        self.tac = next_tac & 0x07;
        self.div = (self.system_counter >> 8) as u8;
        let next_signal = timer_signal(self.system_counter, self.tac);

        if previous_signal && !next_signal {
            self.increment_tima();
        }
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
