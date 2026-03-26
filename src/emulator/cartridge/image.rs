use std::fs;
use std::path::Path;
use std::sync::Arc;

use super::error::{BootRomLoadError, CartridgeLoadError};
use super::types::{BootMode, MbcKind};
use super::{
    CGB_BOOT_ROM_BYTES, DMG_BOOT_ROM_BYTES, HEADER_CARTRIDGE_TYPE, HEADER_CGB_FLAG,
    HEADER_HEADER_CHECKSUM, HEADER_NEW_LICENSEE_END, HEADER_NEW_LICENSEE_START,
    HEADER_OLD_LICENSEE, HEADER_RAM_SIZE, HEADER_ROM_SIZE, HEADER_SGB_FLAG, HEADER_TITLE_END,
    HEADER_TITLE_START, MIN_ROM_BYTES, NINTENDO_LOGO, RAM_WINDOW_SIZE, ROM_BANK_SIZE,
};

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

    pub(super) fn read(&self, address: u16) -> Option<u8> {
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
    pub(super) metadata: CartridgeMetadata,
    pub(super) rom: Vec<u8>,
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
            0x03 | 0x06 | 0x09 | 0x0d | 0x0f | 0x10 | 0x13 | 0x1b | 0x1e | 0x20 | 0x22 | 0xfc | 0xfd | 0xfe | 0xff
        )
    }

    pub fn has_rtc(&self) -> bool {
        matches!(self.cartridge_type, 0x0f | 0x10)
    }
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

pub(super) fn write_title(rom: &mut [u8], title: &str) {
    let title_bytes = title.as_bytes();
    let slice = &mut rom[HEADER_TITLE_START..HEADER_TITLE_END];
    slice.fill(0);

    for (destination, source) in slice.iter_mut().zip(title_bytes.iter().copied()) {
        *destination = source;
    }
}

pub(super) fn write_nintendo_logo(rom: &mut [u8]) {
    rom[0x0104..0x0134].copy_from_slice(&NINTENDO_LOGO);
}

pub(super) fn write_header_checksum(rom: &mut [u8]) {
    let checksum = rom[HEADER_TITLE_START..=HEADER_HEADER_CHECKSUM - 1]
        .iter()
        .fold(0u8, |sum, value| sum.wrapping_sub(*value).wrapping_sub(1));
    rom[HEADER_HEADER_CHECKSUM] = checksum;
}
