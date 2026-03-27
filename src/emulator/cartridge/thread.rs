use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::thread;

use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};

use crate::emulator::component::{
    CartridgeReport, Command, ComponentReport, MemoryCommand, ReadResult,
    SharedCartridgeRamState, SharedCartridgeReadState, WriteResult,
};

use super::controller::{CartridgeController, HuC1State, HuC3State, Mbc1State, Mbc2State, Mbc3State, Mbc5State, Mbc6State, Mbc7State, Mmm01State, PocketCameraState, Tama5State};
use super::error::CartridgeLoadError;
use super::image::{BootRomImage, CartridgeImage, CartridgeMetadata};
use super::save_state::{self, CartridgeSaveState};
use super::types::MbcKind;
use super::RAM_WINDOW_SIZE;

#[derive(Debug)]
pub struct CartridgeThread {
    metadata: CartridgeMetadata,
    boot_rom: BootRomImage,
    boot_rom_mapped: bool,
    controller: CartridgeController,
    shared_read_state: Option<SharedCartridgeReadState>,
    shared_ram_state: Option<SharedCartridgeRamState>,
    rom: Vec<u8>,
    save_path: Option<std::path::PathBuf>,
    steps: u64,
    last_reported_rom_bank: u8,
    last_reported_ram_bank: u8,
}

const CARTRIDGE_REPORT_INTERVAL: u64 = 4096;

impl CartridgeThread {
    pub fn spawn(
        image: CartridgeImage,
        boot_rom: BootRomImage,
        boot_seed: u64,
        shared_read_state: Option<SharedCartridgeReadState>,
        save_path: Option<std::path::PathBuf>,
        inbox: Receiver<Command>,
        reports: Sender<ComponentReport>,
    ) -> Result<(thread::JoinHandle<()>, Option<SharedCartridgeRamState>), CartridgeLoadError> {
        let cartridge = Self::from_image(image, boot_rom, boot_seed, shared_read_state, save_path)?;
        let shared_ram = cartridge.shared_ram_state.clone();
        let handle = thread::Builder::new()
            .name("cartridge".into())
            .spawn(move || cartridge.run(inbox, reports))
            .expect("failed to spawn cartridge thread");
        Ok((handle, shared_ram))
    }

    pub(super) fn from_image(
        image: CartridgeImage,
        boot_rom: BootRomImage,
        boot_seed: u64,
        shared_read_state: Option<SharedCartridgeReadState>,
        save_path: Option<std::path::PathBuf>,
    ) -> Result<Self, CartridgeLoadError> {
        image.ensure_runtime_supported()?;

        let controller = match image.metadata.mbc {
            MbcKind::RomOnly => CartridgeController::RomOnly,
            MbcKind::Mbc1 => CartridgeController::Mbc1(Mbc1State::default()),
            MbcKind::Mbc2 => CartridgeController::Mbc2(Mbc2State::new(boot_seed)),
            MbcKind::Mbc3 => CartridgeController::Mbc3(Mbc3State::new()),
            MbcKind::Mbc5 => CartridgeController::Mbc5(Mbc5State::default()),
            MbcKind::HuC1 => CartridgeController::HuC1(HuC1State::default()),
            MbcKind::HuC3 => CartridgeController::HuC3(HuC3State::new()),
            MbcKind::Mbc7 => CartridgeController::Mbc7(Mbc7State::new()),
            MbcKind::Mmm01 => CartridgeController::Mmm01(Mmm01State::new()),
            MbcKind::Mbc6 => CartridgeController::Mbc6(Mbc6State::new(&image.rom)),
            MbcKind::PocketCamera => CartridgeController::PocketCamera(PocketCameraState::new()),
            MbcKind::BandaiTama5 => CartridgeController::BandaiTama5(Tama5State::new()),
            unsupported => return Err(CartridgeLoadError::UnsupportedMbc(unsupported)),
        };

        // MBC1 hardware supports RAM even when the header doesn't declare it.
        // Provide at least 8KB for compatibility with test ROMs and games.
        let mut metadata = image.metadata;
        if matches!(metadata.mbc, MbcKind::Mbc1) && metadata.ram_size == 0 {
            metadata.ram_size = RAM_WINDOW_SIZE;
            metadata.ram_bank_count = 1;
        }
        let mut ram = vec![0; metadata.ram_size];
        if !ram.is_empty() {
            let mut rng = StdRng::seed_from_u64(boot_seed ^ 0x5eed_cafe_dead_beef);
            rng.fill_bytes(&mut ram);
        }

        // Load battery-backed save if present.
        let mut controller = controller;
        if metadata.has_battery() {
            if let Some(path) = &save_path {
                save_state::load_save_file(path, &mut ram, &mut controller, &metadata);
            }
        }

        let standard = controller.uses_standard_ram();
        let shared_ram_state = Some(SharedCartridgeRamState::new(
            ram,
            metadata.ram_bank_count,
            metadata.ram_size,
            standard,
        ));
        // Sync initial state.
        if let Some(ref shared) = shared_ram_state {
            shared.set_ram_enabled(controller.ram_enabled());
            shared.set_selected_ram_bank(controller.selected_ram_bank(&metadata));
        }

        Ok(Self {
            boot_rom,
            boot_rom_mapped: true,
            metadata,
            controller,
            shared_read_state,
            shared_ram_state,
            rom: image.rom,
            save_path,
            steps: 0,
            last_reported_rom_bank: 1,
            last_reported_ram_bank: 0,
        })
    }

    fn ram(&self) -> &[u8] {
        self.shared_ram_state.as_ref().map(|s| s.ram_slice()).unwrap_or(&[])
    }

    fn ram_mut(&self) -> &mut [u8] {
        self.shared_ram_state.as_ref().map(|s| s.ram_mut_slice()).unwrap_or(&mut [])
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
                    Ok(Command::SetHardwareMode(_)) => {}
                    Ok(Command::SaveState(respond_to)) => {
                        let state = save_state::create_save(&self.controller, self.ram(), self.boot_rom_mapped);
                        let bytes = bincode::serialize(&state).unwrap_or_default();
                        let _ = respond_to.send(bytes);
                    }
                    Ok(Command::LoadState(bytes)) => {
                        if let Ok(state) = bincode::deserialize::<CartridgeSaveState>(&bytes) {
                            let ram = self.shared_ram_state.as_ref().map(|s| s.ram_mut_slice()).unwrap_or(&mut []);
                            save_state::apply_save(
                                &mut self.controller,
                                ram,
                                &mut self.boot_rom_mapped,
                                state,
                            );
                            if let Some(shared) = &self.shared_read_state {
                                shared.set_boot_rom_mapped(self.boot_rom_mapped);
                            }
                            self.publish_shared_bank_state();
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


            self.steps += 1;
            let selected_rom_bank = self.controller.selected_rom_bank(&self.metadata) as u8;
            let selected_ram_bank = self.controller.selected_ram_bank(&self.metadata) as u8;
            if self.steps == 1
                || self.steps % CARTRIDGE_REPORT_INTERVAL == 0
                || selected_rom_bank != self.last_reported_rom_bank
                || selected_ram_bank != self.last_reported_ram_bank
            {
                self.last_reported_rom_bank = selected_rom_bank;
                self.last_reported_ram_bank = selected_ram_bank;
                let _ = reports.send(ComponentReport::Cartridge(CartridgeReport {
                    steps: self.steps,
                    selected_rom_bank,
                    selected_ram_bank,
                }));
            }
        }

        // Persist battery-backed RAM on shutdown.
        if self.metadata.has_battery() {
            if let Some(path) = &self.save_path {
                save_state::write_save_file(path, self.ram(), &self.controller, &self.metadata);
            }
        }
    }

    pub(super) fn handle_memory(&mut self, command: MemoryCommand) {
        match command {
            MemoryCommand::Read {
                address,
                respond_to,
            } => {
                let result = if address == 0xff50 {
                    ReadResult::Ready(if self.boot_rom_mapped { 0x00 } else { 0x01 })
                } else if self.boot_rom_mapped {
                    if let Some(byte) = self.boot_rom.read(address) {
                        ReadResult::Ready(byte)
                    } else {
                        self.controller.read(&self.metadata, &self.rom, self.ram(), address)
                    }
                } else {
                    self.controller.read(&self.metadata, &self.rom, self.ram(), address)
                };
                let _ = respond_to.send(result);
            }
            MemoryCommand::Write {
                address,
                value,
                respond_to,
            } => {
                let result = if address == 0xff50 {
                    if value != 0 && self.boot_rom_mapped {
                        self.boot_rom_mapped = false;
                        if let Some(shared) = &self.shared_read_state {
                            shared.set_boot_rom_mapped(false);
                        }
                    }
                    WriteResult::Accepted
                } else {
                    let ram = self.shared_ram_state.as_ref().map(|s| s.ram_mut_slice()).unwrap_or(&mut []);
                    let result = self.controller.write(&self.metadata, ram, address, value);
                    if matches!(result, WriteResult::Accepted) {
                        self.publish_shared_bank_state();
                    }
                    result
                };
                if let Some(respond_to) = respond_to {
                    let _ = respond_to.send(result);
                }
            }
        }
    }

    fn publish_shared_bank_state(&self) {
        if let Some(shared_read_state) = &self.shared_read_state {
            shared_read_state.set_lower_rom_bank(self.controller.lower_rom_bank(&self.metadata));
            shared_read_state.set_selected_rom_bank(self.controller.selected_rom_bank(&self.metadata));
        }
        if let Some(shared_ram) = &self.shared_ram_state {
            shared_ram.set_ram_enabled(self.controller.ram_enabled());
            shared_ram.set_selected_ram_bank(self.controller.selected_ram_bank(&self.metadata));
        }
    }
}
