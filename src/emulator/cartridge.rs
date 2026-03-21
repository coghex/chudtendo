use std::fmt;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Instant;

use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};

use super::component::{
    CartridgeReport, Command, ComponentReport, MemoryCommand, ReadResult, SharedCartridgeReadState,
    WriteResult,
};

const MIN_ROM_BYTES: usize = 0x150;
const ROM_BANK_SIZE: usize = 0x4000;
const RAM_WINDOW_SIZE: usize = 0x2000;
pub const DMG_BOOT_ROM_BYTES: usize = 0x0100;
pub const CGB_BOOT_ROM_BYTES: usize = 0x0900;
const HEADER_TITLE_START: usize = 0x134;
const HEADER_TITLE_END: usize = 0x143;
const HEADER_CGB_FLAG: usize = 0x143;
const HEADER_NEW_LICENSEE_START: usize = 0x144;
const HEADER_NEW_LICENSEE_END: usize = 0x146;
const HEADER_SGB_FLAG: usize = 0x146;
const HEADER_CARTRIDGE_TYPE: usize = 0x147;
const HEADER_ROM_SIZE: usize = 0x148;
const HEADER_RAM_SIZE: usize = 0x149;
const HEADER_OLD_LICENSEE: usize = 0x14b;
const HEADER_HEADER_CHECKSUM: usize = 0x14d;
const NINTENDO_LOGO: [u8; 48] = [
    0xce, 0xed, 0x66, 0x66, 0xcc, 0x0d, 0x00, 0x0b, 0x03, 0x73, 0x00, 0x83, 0x00, 0x0c, 0x00, 0x0d,
    0x00, 0x08, 0x11, 0x1f, 0x88, 0x89, 0x00, 0x0e, 0xdc, 0xcc, 0x6e, 0xe6, 0xdd, 0xdd, 0xd9, 0x99,
    0xbb, 0xbb, 0x67, 0x63, 0x6e, 0x0e, 0xec, 0xcc, 0xdd, 0xdc, 0x99, 0x9f, 0xbb, 0xb9, 0x33, 0x3e,
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootRomImage {
    bytes: Vec<u8>,
}

impl BootRomImage {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, BootRomLoadError> {
        let path = path.as_ref();
        let bytes = fs::read(path).map_err(|error| BootRomLoadError::Io {
            path: path.display().to_string(),
            message: error.to_string(),
        })?;
        Self::from_bytes(bytes)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, BootRomLoadError> {
        if bytes.len() != CGB_BOOT_ROM_BYTES && bytes.len() != DMG_BOOT_ROM_BYTES {
            return Err(BootRomLoadError::UnexpectedSize {
                actual_bytes: bytes.len(),
                expected_bytes: CGB_BOOT_ROM_BYTES,
            });
        }

        Ok(Self { bytes })
    }

    pub fn is_dmg(&self) -> bool {
        self.bytes.len() == DMG_BOOT_ROM_BYTES
    }

    fn read(&self, address: u16) -> Option<u8> {
        if self.is_dmg() {
            match address {
                0x0000..=0x00ff => self.bytes.get(address as usize).copied(),
                _ => None,
            }
        } else {
            match address {
                0x0000..=0x00ff | 0x0200..=0x08ff => self.bytes.get(address as usize).copied(),
                _ => None,
            }
        }
    }

    pub fn shared_bytes(&self) -> Arc<[u8]> {
        Arc::<[u8]>::from(self.bytes.clone())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CartridgeImage {
    metadata: CartridgeMetadata,
    rom: Vec<u8>,
}

impl CartridgeImage {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, CartridgeLoadError> {
        let path = path.as_ref();
        let rom = fs::read(path).map_err(|error| CartridgeLoadError::Io {
            path: path.display().to_string(),
            message: error.to_string(),
        })?;
        Self::from_bytes(rom)
    }

    pub fn from_bytes(rom: Vec<u8>) -> Result<Self, CartridgeLoadError> {
        if rom.len() < MIN_ROM_BYTES {
            return Err(CartridgeLoadError::RomTooSmall {
                actual_bytes: rom.len(),
                minimum_bytes: MIN_ROM_BYTES,
            });
        }

        let cartridge_type = rom[HEADER_CARTRIDGE_TYPE];
        let mbc = MbcKind::from_header(cartridge_type);
        let boot_mode = BootMode::from_header(rom[HEADER_CGB_FLAG], rom[HEADER_SGB_FLAG]);
        let rom_bank_count = rom_bank_count_from_code(rom[HEADER_ROM_SIZE])?;
        let declared_bytes = rom_bank_count * ROM_BANK_SIZE;

        if rom.len() != declared_bytes {
            return Err(CartridgeLoadError::RomSizeMismatch {
                declared_bytes,
                actual_bytes: rom.len(),
            });
        }

        let (ram_size, ram_bank_count) = ram_layout_from_code(rom[HEADER_RAM_SIZE])?;

        Ok(Self {
            metadata: CartridgeMetadata {
                title: parse_title(&rom[HEADER_TITLE_START..HEADER_TITLE_END]),
                title_checksum_bytes: rom[HEADER_TITLE_START..=HEADER_CGB_FLAG]
                    .try_into()
                    .expect("title checksum slice should be 16 bytes"),
                mbc,
                boot_mode,
                cartridge_type,
                rom_bank_count,
                ram_size,
                ram_bank_count,
                old_licensee_code: rom[HEADER_OLD_LICENSEE],
                new_licensee_code: rom[HEADER_NEW_LICENSEE_START..HEADER_NEW_LICENSEE_END]
                    .try_into()
                    .expect("new licensee slice should be 2 bytes"),
            },
            rom,
        })
    }

    pub fn placeholder() -> Self {
        let mut rom = vec![0; ROM_BANK_SIZE * 2];

        for (bank_index, bank) in rom.chunks_exact_mut(ROM_BANK_SIZE).enumerate() {
            bank.fill((bank_index as u8).wrapping_mul(0x11));
        }

        write_nintendo_logo(&mut rom);
        write_title(&mut rom, "CHUDTENDO");
        rom[HEADER_CGB_FLAG] = 0xc0;
        rom[HEADER_CARTRIDGE_TYPE] = 0x00;
        rom[HEADER_ROM_SIZE] = 0x00;
        rom[HEADER_RAM_SIZE] = 0x00;
        write_header_checksum(&mut rom);

        Self::from_bytes(rom).expect("placeholder cartridge should always parse")
    }

    pub fn ensure_runtime_supported(&self) -> Result<(), CartridgeLoadError> {
        if !self.metadata.boot_mode.is_supported() {
            return Err(CartridgeLoadError::UnsupportedBootMode(
                self.metadata.boot_mode,
            ));
        }

        if !self.metadata.mbc.is_supported() {
            return Err(CartridgeLoadError::UnsupportedMbc(self.metadata.mbc));
        }

        Ok(())
    }

    pub fn metadata(&self) -> &CartridgeMetadata {
        &self.metadata
    }

    pub fn shared_rom(&self) -> Arc<[u8]> {
        Arc::<[u8]>::from(self.rom.clone())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CartridgeMetadata {
    pub title: String,
    pub title_checksum_bytes: [u8; 16],
    pub mbc: MbcKind,
    pub boot_mode: BootMode,
    pub cartridge_type: u8,
    pub rom_bank_count: usize,
    pub ram_size: usize,
    pub ram_bank_count: usize,
    pub old_licensee_code: u8,
    pub new_licensee_code: [u8; 2],
}

impl CartridgeMetadata {
    pub fn cgb_compatibility_b(&self) -> u8 {
        if self.old_licensee_code == 0x01
            || (self.old_licensee_code == 0x33 && self.new_licensee_code == *b"01")
        {
            self.title_checksum_bytes
                .iter()
                .fold(0u8, |sum, value| sum.wrapping_add(*value))
        } else {
            0
        }
    }

    pub fn cgb_compatibility_hl(&self) -> u16 {
        match self.cgb_compatibility_b() {
            0x43 | 0x58 => 0x991a,
            _ => 0x007c,
        }
    }

    pub fn has_battery(&self) -> bool {
        matches!(
            self.cartridge_type,
            0x03 | 0x06 | 0x09 | 0x0d | 0x0f | 0x10 | 0x13 | 0x1b | 0x1e
        )
    }

    pub fn has_rtc(&self) -> bool {
        matches!(self.cartridge_type, 0x0f | 0x10)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MbcKind {
    RomOnly,
    Mbc1,
    Mbc2,
    Mmm01Stub,
    Mbc3,
    Mbc5,
    Mbc6Stub,
    Mbc7Stub,
    HuC1Stub,
    HuC3Stub,
    PocketCameraStub,
    BandaiTama5Stub,
    UnknownStub(u8),
}

impl MbcKind {
    pub fn from_header(cartridge_type: u8) -> Self {
        match cartridge_type {
            0x00 | 0x08 | 0x09 => Self::RomOnly,
            0x01..=0x03 => Self::Mbc1,
            0x05 | 0x06 => Self::Mbc2,
            0x0b..=0x0d => Self::Mmm01Stub,
            0x0f..=0x13 => Self::Mbc3,
            0x19..=0x1e => Self::Mbc5,
            0x20 => Self::Mbc6Stub,
            0x22 => Self::Mbc7Stub,
            0xfc => Self::PocketCameraStub,
            0xfd => Self::BandaiTama5Stub,
            0xfe => Self::HuC3Stub,
            0xff => Self::HuC1Stub,
            other => Self::UnknownStub(other),
        }
    }

    pub fn is_supported(self) -> bool {
        matches!(self, Self::RomOnly | Self::Mbc1 | Self::Mbc2 | Self::Mbc3 | Self::Mbc5)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::RomOnly => "ROM ONLY",
            Self::Mbc1 => "MBC1",
            Self::Mbc2 => "MBC2",
            Self::Mmm01Stub => "MMM01 (stub)",
            Self::Mbc3 => "MBC3",
            Self::Mbc5 => "MBC5",
            Self::Mbc6Stub => "MBC6 (stub)",
            Self::Mbc7Stub => "MBC7 (stub)",
            Self::HuC1Stub => "HuC1 (stub)",
            Self::HuC3Stub => "HuC3 (stub)",
            Self::PocketCameraStub => "Pocket Camera (stub)",
            Self::BandaiTama5Stub => "Bandai Tama5 (stub)",
            Self::UnknownStub(_) => "Unknown MBC (stub)",
        }
    }
}

impl fmt::Display for MbcKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownStub(code) => write!(formatter, "{} ({code:#04x})", self.name()),
            _ => formatter.write_str(self.name()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootMode {
    CgbEnhanced,
    CgbOnly,
    Dmg,
    SgbStub,
    UnknownStub(u8),
}

impl BootMode {
    pub fn from_header(cgb_flag: u8, sgb_flag: u8) -> Self {
        match cgb_flag {
            0x80 => Self::CgbEnhanced,
            0xc0 => Self::CgbOnly,
            0x00 if sgb_flag == 0x03 => Self::SgbStub,
            0x00 => Self::Dmg,
            other => Self::UnknownStub(other),
        }
    }

    pub fn is_supported(self) -> bool {
        matches!(self, Self::CgbEnhanced | Self::CgbOnly | Self::Dmg)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::CgbEnhanced => "CGB-enhanced",
            Self::CgbOnly => "CGB-only",
            Self::Dmg => "DMG compatibility",
            Self::SgbStub => "SGB (stub)",
            Self::UnknownStub(_) => "Unknown boot (stub)",
        }
    }
}

impl fmt::Display for BootMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownStub(flag) => write!(formatter, "{} ({flag:#04x})", self.name()),
            _ => formatter.write_str(self.name()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CartridgeLoadError {
    Io {
        path: String,
        message: String,
    },
    RomTooSmall {
        actual_bytes: usize,
        minimum_bytes: usize,
    },
    RomSizeMismatch {
        declared_bytes: usize,
        actual_bytes: usize,
    },
    UnsupportedRomSizeCode(u8),
    UnsupportedRamSizeCode(u8),
    UnsupportedMbc(MbcKind),
    UnsupportedBootMode(BootMode),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BootRomLoadError {
    Io {
        path: String,
        message: String,
    },
    UnexpectedSize {
        actual_bytes: usize,
        expected_bytes: usize,
    },
}

impl fmt::Display for CartridgeLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => {
                write!(formatter, "failed to read ROM `{path}`: {message}")
            }
            Self::RomTooSmall {
                actual_bytes,
                minimum_bytes,
            } => write!(
                formatter,
                "ROM image is too small: got {actual_bytes} bytes, need at least {minimum_bytes}"
            ),
            Self::RomSizeMismatch {
                declared_bytes,
                actual_bytes,
            } => write!(
                formatter,
                "ROM size mismatch: header declares {declared_bytes} bytes, file has {actual_bytes}"
            ),
            Self::UnsupportedRomSizeCode(code) => {
                write!(
                    formatter,
                    "unsupported ROM size code in header: {code:#04x}"
                )
            }
            Self::UnsupportedRamSizeCode(code) => {
                write!(
                    formatter,
                    "unsupported RAM size code in header: {code:#04x}"
                )
            }
            Self::UnsupportedMbc(kind) => write!(
                formatter,
                "unsupported cartridge controller for now: {kind}"
            ),
            Self::UnsupportedBootMode(mode) => {
                write!(formatter, "unsupported boot mode for now: {mode}")
            }
        }
    }
}

impl fmt::Display for BootRomLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => {
                write!(formatter, "failed to read boot ROM `{path}`: {message}")
            }
            Self::UnexpectedSize {
                actual_bytes,
                expected_bytes,
            } => write!(
                formatter,
                "unexpected CGB boot ROM size: got {actual_bytes} bytes, expected {expected_bytes}"
            ),
        }
    }
}

#[derive(Debug)]
pub struct CartridgeThread {
    metadata: CartridgeMetadata,
    boot_rom: BootRomImage,
    boot_rom_mapped: bool,
    controller: CartridgeController,
    shared_read_state: Option<SharedCartridgeReadState>,
    rom: Vec<u8>,
    ram: Vec<u8>,
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
    ) -> Result<thread::JoinHandle<()>, CartridgeLoadError> {
        let cartridge = Self::from_image(image, boot_rom, boot_seed, shared_read_state, save_path)?;

        Ok(thread::Builder::new()
            .name("cartridge".to_owned())
            .spawn(move || cartridge.run(inbox, reports))
            .expect("failed to spawn cartridge thread"))
    }

    fn from_image(
        image: CartridgeImage,
        boot_rom: BootRomImage,
        boot_seed: u64,
        shared_read_state: Option<SharedCartridgeReadState>,
        save_path: Option<std::path::PathBuf>,
    ) -> Result<Self, CartridgeLoadError> {
        image.ensure_runtime_supported()?;

        let mut controller = match image.metadata.mbc {
            MbcKind::RomOnly => CartridgeController::RomOnly,
            MbcKind::Mbc1 => CartridgeController::Mbc1(Mbc1State::default()),
            MbcKind::Mbc2 => CartridgeController::Mbc2(Mbc2State::new(boot_seed)),
            MbcKind::Mbc3 => CartridgeController::Mbc3(Mbc3State::new()),
            MbcKind::Mbc5 => CartridgeController::Mbc5(Mbc5State::default()),
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
        if metadata.has_battery() {
            if let Some(path) = &save_path {
                load_save_file(path, &mut ram, &mut controller, &metadata);
            }
        }

        Ok(Self {
            boot_rom,
            boot_rom_mapped: true,
            ram,
            metadata,
            controller,
            shared_read_state,
            rom: image.rom,
            save_path,
            steps: 0,
            last_reported_rom_bank: 1,
            last_reported_ram_bank: 0,
        })
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
            let selected_rom_bank = self.controller.selected_rom_bank(&self.metadata) as u8;
            let selected_ram_bank = self.controller.selected_ram_bank(&self.metadata) as u8;
            if self.steps == 1
                || self.steps % CARTRIDGE_REPORT_INTERVAL == 0
                || selected_rom_bank != self.last_reported_rom_bank
                || selected_ram_bank != self.last_reported_ram_bank
            {
                let _ = reports.send(ComponentReport::Cartridge(CartridgeReport {
                    steps: self.steps,
                    selected_rom_bank,
                    selected_ram_bank,
                }));
                self.last_reported_rom_bank = selected_rom_bank;
                self.last_reported_ram_bank = selected_ram_bank;
            }
            thread::yield_now();
        }

        // Save battery-backed RAM on shutdown.
        if self.metadata.has_battery() {
            if let Some(path) = &self.save_path {
                write_save_file(path, &self.ram, &self.controller, &self.metadata);
            }
        }
    }

    fn handle_memory(&mut self, command: MemoryCommand) {
        match command {
            MemoryCommand::Read {
                address,
                respond_to,
            } => {
                let result = if address == 0xff50 {
                    ReadResult::Ready(if self.boot_rom_mapped { 0x00 } else { 0x01 })
                } else if self.boot_rom_mapped {
                    self.boot_rom.read(address).map_or_else(
                        || {
                            self.controller
                                .read(&self.metadata, &self.rom, &self.ram, address)
                        },
                        ReadResult::Ready,
                    )
                } else {
                    self.controller
                        .read(&self.metadata, &self.rom, &self.ram, address)
                };
                let _ = respond_to.send(result);
            }
            MemoryCommand::Write {
                address,
                value,
                respond_to,
            } => {
                let result = if address == 0xff50 {
                    if self.boot_rom_mapped && value != 0x00 {
                        self.boot_rom_mapped = false;
                        if let Some(shared_read_state) = &self.shared_read_state {
                            shared_read_state.set_boot_rom_mapped(false);
                        }
                    }
                    WriteResult::Accepted
                } else {
                    let result =
                        self.controller
                            .write(&self.metadata, &mut self.ram, address, value);
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
        let Some(shared_read_state) = &self.shared_read_state else {
            return;
        };

        shared_read_state.set_lower_rom_bank(self.controller.lower_rom_bank(&self.metadata));
        shared_read_state.set_selected_rom_bank(self.controller.selected_rom_bank(&self.metadata));
    }
}

#[derive(Debug)]
enum CartridgeController {
    RomOnly,
    Mbc1(Mbc1State),
    Mbc2(Mbc2State),
    Mbc3(Mbc3State),
    Mbc5(Mbc5State),
}

impl CartridgeController {
    fn read(
        &self,
        metadata: &CartridgeMetadata,
        rom: &[u8],
        ram: &[u8],
        address: u16,
    ) -> ReadResult {
        match self {
            Self::RomOnly => match address {
                0x0000..=0x3fff => read_rom_bank(rom, metadata, 0, address),
                0x4000..=0x7fff => {
                    let bank = metadata.rom_bank_count.saturating_sub(1).min(1);
                    read_rom_bank(rom, metadata, bank, address - 0x4000)
                }
                0xa000..=0xbfff => read_ram_bank(ram, metadata, 0, address - 0xa000),
                _ => ReadResult::NoData,
            },
            Self::Mbc1(state) => state.read(metadata, rom, ram, address),
            Self::Mbc2(state) => state.read(metadata, rom, address),
            Self::Mbc3(state) => state.read(metadata, rom, ram, address),
            Self::Mbc5(state) => state.read(metadata, rom, ram, address),
        }
    }

    fn write(
        &mut self,
        metadata: &CartridgeMetadata,
        ram: &mut [u8],
        address: u16,
        value: u8,
    ) -> WriteResult {
        match self {
            Self::RomOnly => match address {
                0x0000..=0x7fff => WriteResult::Accepted,
                0xa000..=0xbfff => write_ram_bank(ram, metadata, 0, address - 0xa000, value),
                _ => WriteResult::NoData,
            },
            Self::Mbc1(state) => state.write(metadata, ram, address, value),
            Self::Mbc2(state) => state.write(metadata, address, value),
            Self::Mbc3(state) => state.write(metadata, ram, address, value),
            Self::Mbc5(state) => state.write(metadata, ram, address, value),
        }
    }

    fn selected_rom_bank(&self, metadata: &CartridgeMetadata) -> usize {
        match self {
            Self::RomOnly => metadata.rom_bank_count.saturating_sub(1).min(1),
            Self::Mbc1(state) => state.selected_rom_bank(metadata),
            Self::Mbc2(state) => state.selected_rom_bank(metadata),
            Self::Mbc3(state) => state.selected_rom_bank(metadata),
            Self::Mbc5(state) => state.selected_rom_bank(metadata),
        }
    }

    fn lower_rom_bank(&self, _metadata: &CartridgeMetadata) -> usize {
        match self {
            Self::RomOnly => 0,
            Self::Mbc1(state) => state.lower_rom_bank(_metadata),
            Self::Mbc2(_) | Self::Mbc3(_) | Self::Mbc5(_) => 0,
        }
    }

    fn selected_ram_bank(&self, metadata: &CartridgeMetadata) -> usize {
        match self {
            Self::RomOnly => 0.min(metadata.ram_bank_count.saturating_sub(1)),
            Self::Mbc1(state) => state.selected_ram_bank(metadata),
            Self::Mbc2(_) => 0,
            Self::Mbc3(state) => state.selected_ram_bank(metadata),
            Self::Mbc5(state) => state.selected_ram_bank(metadata),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mbc1Mode {
    RomBanking,
    RamBanking,
}

#[derive(Debug)]
struct Mbc1State {
    ram_enabled: bool,
    rom_bank_low5: u8,
    bank_high2: u8,
    mode: Mbc1Mode,
}

impl Mbc1State {
    fn read(
        &self,
        metadata: &CartridgeMetadata,
        rom: &[u8],
        ram: &[u8],
        address: u16,
    ) -> ReadResult {
        match address {
            0x0000..=0x3fff => read_rom_bank(rom, metadata, self.lower_rom_bank(metadata), address),
            0x4000..=0x7fff => read_rom_bank(
                rom,
                metadata,
                self.selected_rom_bank(metadata),
                address - 0x4000,
            ),
            0xa000..=0xbfff if self.ram_enabled => read_ram_bank(
                ram,
                metadata,
                self.selected_ram_bank(metadata),
                address - 0xa000,
            ),
            0xa000..=0xbfff => ReadResult::NoData,
            _ => ReadResult::NoData,
        }
    }

    fn write(
        &mut self,
        metadata: &CartridgeMetadata,
        ram: &mut [u8],
        address: u16,
        value: u8,
    ) -> WriteResult {
        match address {
            0x0000..=0x1fff => {
                self.ram_enabled = value & 0x0f == 0x0a;
                WriteResult::Accepted
            }
            0x2000..=0x3fff => {
                self.rom_bank_low5 = value & 0x1f;
                WriteResult::Accepted
            }
            0x4000..=0x5fff => {
                self.bank_high2 = value & 0x03;
                WriteResult::Accepted
            }
            0x6000..=0x7fff => {
                self.mode = if value & 0x01 == 0 {
                    Mbc1Mode::RomBanking
                } else {
                    Mbc1Mode::RamBanking
                };
                WriteResult::Accepted
            }
            0xa000..=0xbfff if self.ram_enabled => write_ram_bank(
                ram,
                metadata,
                self.selected_ram_bank(metadata),
                address - 0xa000,
                value,
            ),
            0xa000..=0xbfff => WriteResult::NoData,
            _ => WriteResult::NoData,
        }
    }

    fn lower_rom_bank(&self, metadata: &CartridgeMetadata) -> usize {
        match self.mode {
            Mbc1Mode::RomBanking => 0,
            Mbc1Mode::RamBanking => ((self.bank_high2 as usize) << 5) % metadata.rom_bank_count,
        }
    }

    fn selected_rom_bank(&self, metadata: &CartridgeMetadata) -> usize {
        let low = match self.rom_bank_low5 {
            0 => 1,
            value => value as usize,
        };
        let high = (self.bank_high2 as usize) << 5;
        (high | low) % metadata.rom_bank_count
    }

    fn selected_ram_bank(&self, metadata: &CartridgeMetadata) -> usize {
        if metadata.ram_bank_count == 0 {
            return 0;
        }

        match self.mode {
            Mbc1Mode::RomBanking => 0,
            Mbc1Mode::RamBanking => (self.bank_high2 as usize) % metadata.ram_bank_count,
        }
    }
}

impl Default for Mbc1State {
    fn default() -> Self {
        Self {
            ram_enabled: false,
            rom_bank_low5: 1,
            bank_high2: 0,
            mode: Mbc1Mode::RomBanking,
        }
    }
}

const MBC2_RAM_SIZE: usize = 512;

#[derive(Debug)]
struct Mbc2State {
    ram_enabled: bool,
    rom_bank: u8,
    ram: [u8; MBC2_RAM_SIZE],
}

impl Mbc2State {
    fn new(boot_seed: u64) -> Self {
        let mut ram = [0u8; MBC2_RAM_SIZE];
        let mut rng = StdRng::seed_from_u64(boot_seed ^ 0x0bc2_cafe_0000_0000);
        rng.fill_bytes(&mut ram);
        // MBC2 RAM is 4-bit; mask on init so stale upper bits never leak.
        for byte in &mut ram {
            *byte &= 0x0f;
        }
        Self {
            ram_enabled: false,
            rom_bank: 1,
            ram,
        }
    }

    fn read(&self, metadata: &CartridgeMetadata, rom: &[u8], address: u16) -> ReadResult {
        match address {
            0x0000..=0x3fff => read_rom_bank(rom, metadata, 0, address),
            0x4000..=0x7fff => {
                read_rom_bank(rom, metadata, self.selected_rom_bank(metadata), address - 0x4000)
            }
            0xa000..=0xbfff if self.ram_enabled => {
                let index = (address as usize - 0xa000) & 0x1ff;
                ReadResult::Ready(0xf0 | self.ram[index])
            }
            0xa000..=0xbfff => ReadResult::NoData,
            _ => ReadResult::NoData,
        }
    }

    fn write(&mut self, _metadata: &CartridgeMetadata, address: u16, value: u8) -> WriteResult {
        match address {
            0x0000..=0x3fff => {
                if address & 0x0100 == 0 {
                    self.ram_enabled = value & 0x0f == 0x0a;
                } else {
                    let bank = value & 0x0f;
                    self.rom_bank = if bank == 0 { 1 } else { bank };
                }
                WriteResult::Accepted
            }
            0xa000..=0xbfff if self.ram_enabled => {
                let index = (address as usize - 0xa000) & 0x1ff;
                self.ram[index] = value & 0x0f;
                WriteResult::Accepted
            }
            0xa000..=0xbfff => WriteResult::NoData,
            _ => WriteResult::NoData,
        }
    }

    fn selected_rom_bank(&self, metadata: &CartridgeMetadata) -> usize {
        (self.rom_bank as usize) % metadata.rom_bank_count
    }
}

#[derive(Debug)]
struct Mbc3State {
    ram_enabled: bool,
    rom_bank: u8,
    bank_select: u8,
    latch_prep: bool,
    rtc: RtcState,
}

#[derive(Debug)]
struct RtcState {
    seconds: u8,
    minutes: u8,
    hours: u8,
    day_low: u8,
    day_high: u8,
    latched_seconds: u8,
    latched_minutes: u8,
    latched_hours: u8,
    latched_day_low: u8,
    latched_day_high: u8,
    base_instant: Instant,
}

impl Mbc3State {
    fn new() -> Self {
        Self {
            ram_enabled: false,
            rom_bank: 1,
            bank_select: 0,
            latch_prep: false,
            rtc: RtcState::new(),
        }
    }

    fn read(
        &self,
        metadata: &CartridgeMetadata,
        rom: &[u8],
        ram: &[u8],
        address: u16,
    ) -> ReadResult {
        match address {
            0x0000..=0x3fff => read_rom_bank(rom, metadata, 0, address),
            0x4000..=0x7fff => {
                read_rom_bank(rom, metadata, self.selected_rom_bank(metadata), address - 0x4000)
            }
            0xa000..=0xbfff if !self.ram_enabled => ReadResult::NoData,
            0xa000..=0xbfff => match self.bank_select {
                0x00..=0x07 => {
                    read_ram_bank(ram, metadata, self.bank_select as usize, address - 0xa000)
                }
                0x08 => ReadResult::Ready(self.rtc.latched_seconds),
                0x09 => ReadResult::Ready(self.rtc.latched_minutes),
                0x0a => ReadResult::Ready(self.rtc.latched_hours),
                0x0b => ReadResult::Ready(self.rtc.latched_day_low),
                0x0c => ReadResult::Ready(self.rtc.latched_day_high),
                _ => ReadResult::NoData,
            },
            _ => ReadResult::NoData,
        }
    }

    fn write(
        &mut self,
        metadata: &CartridgeMetadata,
        ram: &mut [u8],
        address: u16,
        value: u8,
    ) -> WriteResult {
        match address {
            0x0000..=0x1fff => {
                self.ram_enabled = value & 0x0f == 0x0a;
                WriteResult::Accepted
            }
            0x2000..=0x3fff => {
                self.rom_bank = match value & 0x7f {
                    0 => 1,
                    bank => bank,
                };
                WriteResult::Accepted
            }
            0x4000..=0x5fff => {
                self.bank_select = value;
                WriteResult::Accepted
            }
            0x6000..=0x7fff => {
                if value == 0x00 {
                    self.latch_prep = true;
                } else if value == 0x01 && self.latch_prep {
                    self.rtc.latch();
                    self.latch_prep = false;
                } else {
                    self.latch_prep = false;
                }
                WriteResult::Accepted
            }
            0xa000..=0xbfff if !self.ram_enabled => WriteResult::NoData,
            0xa000..=0xbfff => match self.bank_select {
                0x00..=0x07 => {
                    write_ram_bank(ram, metadata, self.bank_select as usize, address - 0xa000, value)
                }
                0x08 => {
                    self.rtc.write_register(0x08, value);
                    WriteResult::Accepted
                }
                0x09 => {
                    self.rtc.write_register(0x09, value);
                    WriteResult::Accepted
                }
                0x0a => {
                    self.rtc.write_register(0x0a, value);
                    WriteResult::Accepted
                }
                0x0b => {
                    self.rtc.write_register(0x0b, value);
                    WriteResult::Accepted
                }
                0x0c => {
                    self.rtc.write_register(0x0c, value);
                    WriteResult::Accepted
                }
                _ => WriteResult::NoData,
            },
            _ => WriteResult::NoData,
        }
    }

    fn selected_rom_bank(&self, metadata: &CartridgeMetadata) -> usize {
        (self.rom_bank as usize) % metadata.rom_bank_count
    }

    fn selected_ram_bank(&self, metadata: &CartridgeMetadata) -> usize {
        if metadata.ram_bank_count == 0 {
            return 0;
        }
        (self.bank_select as usize).min(metadata.ram_bank_count - 1)
    }
}

impl RtcState {
    fn new() -> Self {
        Self {
            seconds: 0,
            minutes: 0,
            hours: 0,
            day_low: 0,
            day_high: 0,
            latched_seconds: 0,
            latched_minutes: 0,
            latched_hours: 0,
            latched_day_low: 0,
            latched_day_high: 0,
            base_instant: Instant::now(),
        }
    }

    fn is_halted(&self) -> bool {
        self.day_high & 0x40 != 0
    }

    fn advance(&mut self) {
        if self.is_halted() {
            return;
        }

        let elapsed_secs = self.base_instant.elapsed().as_secs();
        self.base_instant = Instant::now();

        if elapsed_secs == 0 {
            return;
        }

        let total = self.total_seconds() + elapsed_secs;
        self.set_from_total_seconds(total);
    }

    fn latch(&mut self) {
        self.advance();
        self.latched_seconds = self.seconds;
        self.latched_minutes = self.minutes;
        self.latched_hours = self.hours;
        self.latched_day_low = self.day_low;
        self.latched_day_high = self.day_high;
    }

    fn write_register(&mut self, register: u8, value: u8) {
        // Advance before writing so we don't lose elapsed time.
        self.advance();

        match register {
            0x08 => self.seconds = value & 0x3f,
            0x09 => self.minutes = value & 0x3f,
            0x0a => self.hours = value & 0x1f,
            0x0b => self.day_low = value,
            0x0c => {
                let was_halted = self.is_halted();
                self.day_high = value & 0xc1;
                if was_halted && !self.is_halted() {
                    // Resuming from halt — reset base so halted time isn't counted.
                    self.base_instant = Instant::now();
                }
            }
            _ => {}
        }
    }

    fn total_seconds(&self) -> u64 {
        let days = ((self.day_high as u64 & 0x01) << 8) | self.day_low as u64;
        days * 86400 + self.hours as u64 * 3600 + self.minutes as u64 * 60 + self.seconds as u64
    }

    fn set_from_total_seconds(&mut self, total: u64) {
        self.seconds = (total % 60) as u8;
        self.minutes = ((total / 60) % 60) as u8;
        self.hours = ((total / 3600) % 24) as u8;

        let days = total / 86400;
        if days > 511 {
            self.day_high |= 0x80; // sticky carry
        }
        let days = days & 0x1ff;
        self.day_low = (days & 0xff) as u8;
        // Preserve halt (bit 6) and carry (bit 7), update day bit 8.
        self.day_high = (self.day_high & 0xc0) | ((days >> 8) as u8 & 0x01);
    }
}

#[derive(Debug)]
struct Mbc5State {
    ram_enabled: bool,
    rom_bank_low: u8,
    rom_bank_high: u8,
    ram_bank: u8,
}

impl Mbc5State {
    fn read(
        &self,
        metadata: &CartridgeMetadata,
        rom: &[u8],
        ram: &[u8],
        address: u16,
    ) -> ReadResult {
        match address {
            0x0000..=0x3fff => read_rom_bank(rom, metadata, 0, address),
            0x4000..=0x7fff => {
                read_rom_bank(rom, metadata, self.selected_rom_bank(metadata), address - 0x4000)
            }
            0xa000..=0xbfff if self.ram_enabled => {
                read_ram_bank(ram, metadata, self.ram_bank as usize, address - 0xa000)
            }
            0xa000..=0xbfff => ReadResult::NoData,
            _ => ReadResult::NoData,
        }
    }

    fn write(
        &mut self,
        metadata: &CartridgeMetadata,
        ram: &mut [u8],
        address: u16,
        value: u8,
    ) -> WriteResult {
        match address {
            0x0000..=0x1fff => {
                self.ram_enabled = value & 0x0f == 0x0a;
                WriteResult::Accepted
            }
            0x2000..=0x2fff => {
                self.rom_bank_low = value;
                WriteResult::Accepted
            }
            0x3000..=0x3fff => {
                self.rom_bank_high = value & 0x01;
                WriteResult::Accepted
            }
            0x4000..=0x5fff => {
                self.ram_bank = value & 0x0f;
                WriteResult::Accepted
            }
            0xa000..=0xbfff if self.ram_enabled => {
                write_ram_bank(ram, metadata, self.ram_bank as usize, address - 0xa000, value)
            }
            0xa000..=0xbfff => WriteResult::NoData,
            _ => WriteResult::NoData,
        }
    }

    fn selected_rom_bank(&self, metadata: &CartridgeMetadata) -> usize {
        let bank = ((self.rom_bank_high as usize) << 8) | self.rom_bank_low as usize;
        bank % metadata.rom_bank_count
    }

    fn selected_ram_bank(&self, metadata: &CartridgeMetadata) -> usize {
        if metadata.ram_bank_count == 0 {
            return 0;
        }
        (self.ram_bank as usize) % metadata.ram_bank_count
    }
}

impl Default for Mbc5State {
    fn default() -> Self {
        Self {
            ram_enabled: false,
            rom_bank_low: 1,
            rom_bank_high: 0,
            ram_bank: 0,
        }
    }
}

fn read_rom_bank(rom: &[u8], metadata: &CartridgeMetadata, bank: usize, offset: u16) -> ReadResult {
    let bank = bank % metadata.rom_bank_count;
    let index = bank * ROM_BANK_SIZE + offset as usize;
    ReadResult::Ready(rom[index])
}

fn read_ram_bank(ram: &[u8], metadata: &CartridgeMetadata, bank: usize, offset: u16) -> ReadResult {
    let Some(index) = ram_index(metadata, bank, offset) else {
        return ReadResult::NoData;
    };
    ReadResult::Ready(ram[index])
}

fn write_ram_bank(
    ram: &mut [u8],
    metadata: &CartridgeMetadata,
    bank: usize,
    offset: u16,
    value: u8,
) -> WriteResult {
    let Some(index) = ram_index(metadata, bank, offset) else {
        return WriteResult::NoData;
    };
    ram[index] = value;
    WriteResult::Accepted
}

fn ram_index(metadata: &CartridgeMetadata, bank: usize, offset: u16) -> Option<usize> {
    if metadata.ram_bank_count == 0 || metadata.ram_size == 0 {
        return None;
    }

    let bank_size = metadata.ram_size / metadata.ram_bank_count;
    let offset = offset as usize;

    if offset >= bank_size {
        return None;
    }

    let bank = bank % metadata.ram_bank_count;
    Some(bank * bank_size + offset)
}

fn rom_bank_count_from_code(code: u8) -> Result<usize, CartridgeLoadError> {
    match code {
        0x00 => Ok(2),
        0x01 => Ok(4),
        0x02 => Ok(8),
        0x03 => Ok(16),
        0x04 => Ok(32),
        0x05 => Ok(64),
        0x06 => Ok(128),
        0x07 => Ok(256),
        0x08 => Ok(512),
        0x52 => Ok(72),
        0x53 => Ok(80),
        0x54 => Ok(96),
        other => Err(CartridgeLoadError::UnsupportedRomSizeCode(other)),
    }
}

fn ram_layout_from_code(code: u8) -> Result<(usize, usize), CartridgeLoadError> {
    match code {
        0x00 => Ok((0, 0)),
        0x01 => Ok((0x0800, 1)),
        0x02 => Ok((RAM_WINDOW_SIZE, 1)),
        0x03 => Ok((RAM_WINDOW_SIZE * 4, 4)),
        0x04 => Ok((RAM_WINDOW_SIZE * 16, 16)),
        0x05 => Ok((RAM_WINDOW_SIZE * 8, 8)),
        other => Err(CartridgeLoadError::UnsupportedRamSizeCode(other)),
    }
}

fn parse_title(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    let title = String::from_utf8_lossy(&bytes[..end]).trim().to_owned();

    if title.is_empty() {
        "UNTITLED".to_owned()
    } else {
        title
    }
}

fn write_title(rom: &mut [u8], title: &str) {
    let title_bytes = title.as_bytes();
    let slice = &mut rom[HEADER_TITLE_START..HEADER_TITLE_END];
    slice.fill(0);

    for (destination, source) in slice.iter_mut().zip(title_bytes.iter().copied()) {
        *destination = source;
    }
}

fn write_nintendo_logo(rom: &mut [u8]) {
    rom[0x0104..0x0134].copy_from_slice(&NINTENDO_LOGO);
}

fn write_header_checksum(rom: &mut [u8]) {
    let checksum = rom[HEADER_TITLE_START..=HEADER_HEADER_CHECKSUM - 1]
        .iter()
        .fold(0u8, |sum, value| sum.wrapping_sub(*value).wrapping_sub(1));
    rom[HEADER_HEADER_CHECKSUM] = checksum;
}

const RTC_SAVE_SIZE: usize = 48;

fn load_save_file(
    path: &std::path::Path,
    ram: &mut [u8],
    controller: &mut CartridgeController,
    metadata: &CartridgeMetadata,
) {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(_) => return,
    };

    match controller {
        CartridgeController::Mbc2(state) => {
            let len = data.len().min(MBC2_RAM_SIZE);
            state.ram[..len].copy_from_slice(&data[..len]);
            for byte in &mut state.ram {
                *byte &= 0x0f;
            }
        }
        CartridgeController::Mbc3(state) if metadata.has_rtc() => {
            let ram_len = data.len().min(ram.len());
            ram[..ram_len].copy_from_slice(&data[..ram_len]);
            if data.len() >= ram.len() + RTC_SAVE_SIZE {
                let rtc_offset = ram.len();
                state.rtc.seconds = data[rtc_offset];
                state.rtc.minutes = data[rtc_offset + 4];
                state.rtc.hours = data[rtc_offset + 8];
                state.rtc.day_low = data[rtc_offset + 12];
                state.rtc.day_high = data[rtc_offset + 16];
                state.rtc.latched_seconds = data[rtc_offset + 20];
                state.rtc.latched_minutes = data[rtc_offset + 24];
                state.rtc.latched_hours = data[rtc_offset + 28];
                state.rtc.latched_day_low = data[rtc_offset + 32];
                state.rtc.latched_day_high = data[rtc_offset + 36];
            }
        }
        _ => {
            let len = data.len().min(ram.len());
            if !ram.is_empty() {
                ram[..len].copy_from_slice(&data[..len]);
            }
        }
    }
}

fn write_save_file(
    path: &std::path::Path,
    ram: &[u8],
    controller: &CartridgeController,
    metadata: &CartridgeMetadata,
) {
    let data = match controller {
        CartridgeController::Mbc2(state) => state.ram.to_vec(),
        CartridgeController::Mbc3(state) if metadata.has_rtc() => {
            let mut data = ram.to_vec();
            let mut rtc_block = [0u8; RTC_SAVE_SIZE];
            rtc_block[0] = state.rtc.seconds;
            rtc_block[4] = state.rtc.minutes;
            rtc_block[8] = state.rtc.hours;
            rtc_block[12] = state.rtc.day_low;
            rtc_block[16] = state.rtc.day_high;
            rtc_block[20] = state.rtc.latched_seconds;
            rtc_block[24] = state.rtc.latched_minutes;
            rtc_block[28] = state.rtc.latched_hours;
            rtc_block[32] = state.rtc.latched_day_low;
            rtc_block[36] = state.rtc.latched_day_high;
            // Bytes 40-47: timestamp (unused for now, reserved for compatibility).
            data.extend_from_slice(&rtc_block);
            data
        }
        _ => ram.to_vec(),
    };

    if data.is_empty() {
        return;
    }

    if let Err(error) = fs::write(path, &data) {
        eprintln!("warning: failed to write save file {}: {error}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rom_bank_count_for_code(code: u8) -> usize {
        rom_bank_count_from_code(code).unwrap()
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

        write_nintendo_logo(&mut rom);
        write_title(&mut rom, title);
        rom[HEADER_CGB_FLAG] = cgb_flag;
        rom[HEADER_SGB_FLAG] = sgb_flag;
        rom[HEADER_CARTRIDGE_TYPE] = cartridge_type;
        rom[HEADER_ROM_SIZE] = rom_size_code;
        rom[HEADER_RAM_SIZE] = ram_size_code;
        write_header_checksum(&mut rom);
        rom
    }

    fn cartridge_read(cartridge: &mut CartridgeThread, address: u16) -> ReadResult {
        let (sender, receiver) = std::sync::mpsc::channel();
        cartridge.handle_memory(MemoryCommand::Read {
            address,
            respond_to: sender,
        });
        receiver.recv().unwrap()
    }

    fn cartridge_write(cartridge: &mut CartridgeThread, address: u16, value: u8) -> WriteResult {
        let (sender, receiver) = std::sync::mpsc::channel();
        cartridge.handle_memory(MemoryCommand::Write {
            address,
            value,
            respond_to: Some(sender),
        });
        receiver.recv().unwrap()
    }

    #[test]
    fn parses_header_metadata_for_cgb_mbc1_roms() {
        let image =
            CartridgeImage::from_bytes(build_test_rom("MBC1TEST", 0x80, 0x00, 0x03, 0x01, 0x03))
                .unwrap();

        assert_eq!(image.metadata().title, "MBC1TEST");
        assert_eq!(image.metadata().boot_mode, BootMode::CgbEnhanced);
        assert_eq!(image.metadata().mbc, MbcKind::Mbc1);
        assert_eq!(image.metadata().rom_bank_count, 4);
        assert_eq!(image.metadata().ram_bank_count, 4);
        assert_eq!(image.metadata().ram_size, RAM_WINDOW_SIZE * 4);
    }

    #[test]
    fn boot_rom_overlay_unmaps_permanently_after_ff50_write() {
        let image =
            CartridgeImage::from_bytes(build_test_rom("BOOTMAP", 0xc0, 0x00, 0x00, 0x00, 0x00))
                .unwrap();
        let mut boot_rom_bytes = vec![0xaa; CGB_BOOT_ROM_BYTES];
        boot_rom_bytes[0x0000] = 0x99;
        boot_rom_bytes[0x0200] = 0x77;
        let boot_rom = BootRomImage::from_bytes(boot_rom_bytes).unwrap();
        let mut cartridge = CartridgeThread::from_image(image, boot_rom, 0, None, None).unwrap();

        assert_eq!(
            cartridge_read(&mut cartridge, 0x0000),
            ReadResult::Ready(0x99)
        );
        assert_eq!(
            cartridge_read(&mut cartridge, 0x0200),
            ReadResult::Ready(0x77)
        );
        assert_eq!(
            cartridge_read(&mut cartridge, 0x0100),
            ReadResult::Ready(0x00)
        );
        assert_eq!(
            cartridge_read(&mut cartridge, 0xff50),
            ReadResult::Ready(0x00)
        );

        assert_eq!(
            cartridge_write(&mut cartridge, 0xff50, 0x00),
            WriteResult::Accepted
        );
        assert_eq!(
            cartridge_read(&mut cartridge, 0x0000),
            ReadResult::Ready(0x99)
        );

        assert_eq!(
            cartridge_write(&mut cartridge, 0xff50, 0x01),
            WriteResult::Accepted
        );
        assert_eq!(
            cartridge_read(&mut cartridge, 0xff50),
            ReadResult::Ready(0x01)
        );
        assert_eq!(
            cartridge_read(&mut cartridge, 0x0000),
            ReadResult::Ready(0x00)
        );
        assert_eq!(
            cartridge_read(&mut cartridge, 0x0200),
            ReadResult::Ready(0x00)
        );

        assert_eq!(
            cartridge_write(&mut cartridge, 0xff50, 0x00),
            WriteResult::Accepted
        );
        assert_eq!(
            cartridge_read(&mut cartridge, 0xff50),
            ReadResult::Ready(0x01)
        );
        assert_eq!(
            cartridge_read(&mut cartridge, 0x0000),
            ReadResult::Ready(0x00)
        );
    }

    #[test]
    fn runtime_support_rejects_non_cgb_boot_modes() {
        let image =
            CartridgeImage::from_bytes(build_test_rom("SGBTEST", 0x00, 0x03, 0x00, 0x00, 0x00))
                .unwrap();

        assert_eq!(
            image.ensure_runtime_supported(),
            Err(CartridgeLoadError::UnsupportedBootMode(BootMode::SgbStub))
        );
    }

    #[test]
    fn runtime_support_rejects_stubbed_mbc_types() {
        let image =
            CartridgeImage::from_bytes(build_test_rom("MBC6TEST", 0xc0, 0x00, 0x20, 0x01, 0x03))
                .unwrap();

        assert_eq!(
            image.ensure_runtime_supported(),
            Err(CartridgeLoadError::UnsupportedMbc(MbcKind::Mbc6Stub))
        );
    }
}
