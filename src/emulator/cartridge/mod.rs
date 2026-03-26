mod controller;
mod error;
mod image;
mod save_state;
mod thread;
mod types;

pub use error::{BootRomLoadError, CartridgeLoadError};
pub use image::{BootRomImage, CartridgeImage, CartridgeMetadata};
// save_state types are used only within the cartridge module.
pub use thread::CartridgeThread;
pub use types::{BootMode, MbcKind};

use crate::emulator::component::{ReadResult, WriteResult};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emulator::component::MemoryCommand;
    use image::{write_header_checksum, write_nintendo_logo, write_title};

    fn bank_count_from_code(code: u8) -> usize {
        match code {
            0x00 => 2,
            0x01 => 4,
            0x02 => 8,
            0x03 => 16,
            0x04 => 32,
            0x05 => 64,
            0x06 => 128,
            0x07 => 256,
            0x08 => 512,
            0x52 => 72,
            0x53 => 80,
            0x54 => 96,
            _ => 2,
        }
    }

    fn build_test_rom(
        title: &str,
        cgb_flag: u8,
        sgb_flag: u8,
        cartridge_type: u8,
        rom_size_code: u8,
        ram_size_code: u8,
    ) -> Vec<u8> {
        let bank_count = match rom_size_code {
            0x00 => 2,
            0x01 => 4,
            0x02 => 8,
            0x03 => 16,
            0x04 => 32,
            0x05 => 64,
            0x06 => 128,
            0x07 => 256,
            0x08 => 512,
            0x52 => 72,
            0x53 => 80,
            0x54 => 96,
            _ => 2,
        };
        let mut rom = vec![0; bank_count * ROM_BANK_SIZE];

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
            CartridgeImage::from_bytes(build_test_rom("UNKTEST", 0xc0, 0x00, 0xbf, 0x01, 0x03))
                .unwrap();

        assert_eq!(
            image.ensure_runtime_supported(),
            Err(CartridgeLoadError::UnsupportedMbc(MbcKind::UnknownStub(0xbf)))
        );
    }
}
