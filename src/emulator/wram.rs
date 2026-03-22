use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::thread;

use serde::{Deserialize, Serialize};

use super::component::{
    Command, ComponentReport, HardwareMode, MemoryCommand, ReadResult, WramInitState, WramReport,
    WriteResult,
};

#[derive(Serialize, Deserialize)]
pub struct WramSaveState {
    pub banks: Vec<Vec<u8>>,
    pub selected_bank: u8,
    pub hardware_mode: HardwareMode,
}

const WRAM_REPORT_INTERVAL: u64 = 4096;

#[derive(Debug)]
pub struct WramThread {
    banks: Vec<Vec<u8>>,
    selected_bank: u8,
    hardware_mode: HardwareMode,
    steps: u64,
    last_reported_bank: u8,
}

impl WramThread {
    pub fn spawn(
        init_state: WramInitState,
        inbox: Receiver<Command>,
        reports: Sender<ComponentReport>,
    ) -> thread::JoinHandle<()> {
        thread::Builder::new()
            .name("wram".to_owned())
            .spawn(move || Self::from_init_state(init_state).run(inbox, reports))
            .expect("failed to spawn wram thread")
    }

    fn run(
        mut self,
        inbox: Receiver<Command>,
        reports: Sender<ComponentReport>,
    ) {
        let mut running = true;

        while running {
            loop {
                match inbox.try_recv() {
                    Ok(Command::Memory(command)) => self.handle_memory(command),
                    Ok(Command::SetHardwareMode(hardware_mode)) => {
                        self.hardware_mode = hardware_mode;
                    }
                    Ok(Command::SaveState(respond_to)) => {
                        let state = self.create_save_state();
                        let bytes = bincode::serialize(&state).unwrap_or_default();
                        let _ = respond_to.send(bytes);
                    }
                    Ok(Command::LoadState(bytes)) => {
                        if let Ok(state) = bincode::deserialize::<WramSaveState>(&bytes) {
                            self.apply_save_state(state);
                        }
                    }
                    Ok(Command::Stop) => {
                        running = false;
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        running = false;
                        break;
                    }
                }
            }

            if !running {
                break;
            }

            self.steps += 1;
            if self.steps == 1
                || self.steps % WRAM_REPORT_INTERVAL == 0
                || self.selected_bank != self.last_reported_bank
            {
                let _ = reports.send(ComponentReport::Wram(WramReport {
                    steps: self.steps,
                    selected_bank: self.selected_bank,
                }));
                self.last_reported_bank = self.selected_bank;
            }
            thread::yield_now();
        }
    }

    fn handle_memory(&mut self, command: MemoryCommand) {
        match command {
            MemoryCommand::Read {
                address,
                respond_to,
            } => {
                let result = match address {
                    0xff70 if matches!(self.hardware_mode, HardwareMode::DmgCompatibility) => {
                        ReadResult::Ready(0xff)
                    }
                    0xff70 => ReadResult::Ready(0xf8 | self.selected_bank),
                    _ => self
                        .translate(address)
                        .map(|(bank, offset)| ReadResult::Ready(self.banks[bank][offset]))
                        .unwrap_or(ReadResult::NoData),
                };
                let _ = respond_to.send(result);
            }
            MemoryCommand::Write {
                address,
                value,
                respond_to,
            } => {
                let result = match address {
                    0xff70 if matches!(self.hardware_mode, HardwareMode::DmgCompatibility) => {
                        WriteResult::Accepted
                    }
                    0xff70 => {
                        self.selected_bank = select_wram_bank(value);
                        WriteResult::Accepted
                    }
                    _ => match self.translate(address) {
                        Some((bank, offset)) => {
                            self.banks[bank][offset] = value;
                            WriteResult::Accepted
                        }
                        None => WriteResult::NoData,
                    },
                };
                if let Some(respond_to) = respond_to {
                    let _ = respond_to.send(result);
                }
            }
        }
    }

    fn translate(&self, address: u16) -> Option<(usize, usize)> {
        match address {
            0xc000..=0xcfff => Some((0, (address - 0xc000) as usize)),
            0xd000..=0xdfff => Some((self.selected_bank as usize, (address - 0xd000) as usize)),
            0xe000..=0xefff => Some((0, (address - 0xe000) as usize)),
            0xf000..=0xfdff => Some((self.selected_bank as usize, (address - 0xf000) as usize)),
            _ => None,
        }
    }

    fn from_init_state(init_state: WramInitState) -> Self {
        let selected_bank = select_wram_bank(init_state.selected_bank);
        Self {
            banks: init_state.banks,
            selected_bank,
            hardware_mode: init_state.hardware_mode,
            steps: 0,
            last_reported_bank: selected_bank,
        }
    }

    fn create_save_state(&self) -> WramSaveState {
        WramSaveState {
            banks: self.banks.clone(),
            selected_bank: self.selected_bank,
            hardware_mode: self.hardware_mode,
        }
    }

    fn apply_save_state(&mut self, state: WramSaveState) {
        self.banks = state.banks;
        self.selected_bank = state.selected_bank;
        self.hardware_mode = state.hardware_mode;
    }
}

impl Default for WramThread {
    fn default() -> Self {
        Self::from_init_state(WramInitState::default())
    }
}

fn select_wram_bank(value: u8) -> u8 {
    match value & 0x07 {
        0 => 1,
        selected => selected,
    }
}
