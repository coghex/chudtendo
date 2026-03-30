use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::thread;

use serde::{Deserialize, Serialize};

use super::component::{
    Command, ComponentReport, HardwareMode, MemoryCommand, ReadResult, SharedWramState,
    WramInitState, WramReport, WriteResult,
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
    shared: SharedWramState,
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
                    Ok(Command::DivApuEdge) => {}
                    Ok(Command::SetHardwareMode(hardware_mode)) => {
                        self.hardware_mode = hardware_mode;
                        self.shared.set_hardware_mode(hardware_mode);
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
                    Ok(Command::RequestPpuFeatures(_)) => {} Ok(Command::Stop) => {
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

            let selected_bank = self.shared.selected_bank();
            self.steps += 1;
            if self.steps == 1
                || self.steps % WRAM_REPORT_INTERVAL == 0
                || selected_bank != self.last_reported_bank
            {
                let _ = reports.send(ComponentReport::Wram(WramReport {
                    steps: self.steps,
                    selected_bank,
                }));
                self.last_reported_bank = selected_bank;
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
                let result = self
                    .shared
                    .read(address)
                    .map(ReadResult::Ready)
                    .unwrap_or(ReadResult::NoData);
                let _ = respond_to.send(result);
            }
            MemoryCommand::Write {
                address,
                value,
                respond_to,
            } => {
                let result = if self.shared.write(address, value) {
                    WriteResult::Accepted
                } else {
                    WriteResult::NoData
                };
                if let Some(respond_to) = respond_to {
                    let _ = respond_to.send(result);
                }
            }
        }
    }

    fn from_init_state(init_state: WramInitState) -> Self {
        let shared = init_state.shared.unwrap_or_else(|| {
            SharedWramState::new(
                init_state.banks,
                super::component::select_wram_bank(init_state.selected_bank),
                init_state.hardware_mode,
            )
        });
        shared.set_hardware_mode(init_state.hardware_mode);
        let selected_bank = shared.selected_bank();
        Self {
            shared,
            hardware_mode: init_state.hardware_mode,
            steps: 0,
            last_reported_bank: selected_bank,
        }
    }

    fn create_save_state(&self) -> WramSaveState {
        WramSaveState {
            banks: self.shared.snapshot_banks(),
            selected_bank: self.shared.selected_bank(),
            hardware_mode: self.hardware_mode,
        }
    }

    fn apply_save_state(&mut self, state: WramSaveState) {
        self.shared.restore_banks(&state.banks);
        self.shared.set_selected_bank(super::component::select_wram_bank(state.selected_bank));
        self.hardware_mode = state.hardware_mode;
        self.shared.set_hardware_mode(state.hardware_mode);
    }
}

impl Default for WramThread {
    fn default() -> Self {
        Self::from_init_state(WramInitState::default())
    }
}
