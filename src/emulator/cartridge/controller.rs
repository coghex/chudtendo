use std::time::Instant;

use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};

use crate::emulator::component::{ReadResult, WriteResult};

use super::image::CartridgeMetadata;
use super::{read_ram_bank, read_rom_bank, write_ram_bank};

#[derive(Debug)]
pub(super) enum CartridgeController {
    RomOnly,
    Mbc1(Mbc1State),
    Mbc2(Mbc2State),
    Mbc3(Mbc3State),
    Mbc5(Mbc5State),
    HuC1(HuC1State),
    HuC3(HuC3State),
    Mbc7(Mbc7State),
    Mmm01(Mmm01State),
    Mbc6(Mbc6State),
    PocketCamera(PocketCameraState),
    BandaiTama5(Tama5State),
}

impl CartridgeController {
    pub(super) fn read(
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
            Self::HuC1(state) => state.read(metadata, rom, ram, address),
            Self::HuC3(state) => state.read(metadata, rom, ram, address),
            Self::Mbc7(state) => state.read(metadata, rom, address),
            Self::Mmm01(state) => state.read(metadata, rom, ram, address),
            Self::Mbc6(state) => state.read(metadata, rom, ram, address),
            Self::PocketCamera(state) => state.read(metadata, rom, ram, address),
            Self::BandaiTama5(state) => state.read(metadata, rom, address),
        }
    }

    pub(super) fn write(
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
            Self::HuC1(state) => state.write(metadata, ram, address, value),
            Self::HuC3(state) => state.write(metadata, ram, address, value),
            Self::Mbc7(state) => state.write(metadata, address, value),
            Self::Mmm01(state) => state.write(metadata, ram, address, value),
            Self::Mbc6(state) => state.write(metadata, ram, address, value),
            Self::PocketCamera(state) => state.write(metadata, ram, address, value),
            Self::BandaiTama5(state) => state.write(metadata, address, value),
        }
    }

    pub(super) fn selected_rom_bank(&self, metadata: &CartridgeMetadata) -> usize {
        match self {
            Self::RomOnly => metadata.rom_bank_count.saturating_sub(1).min(1),
            Self::Mbc1(state) => state.selected_rom_bank(metadata),
            Self::Mbc2(state) => state.selected_rom_bank(metadata),
            Self::Mbc3(state) => state.selected_rom_bank(metadata),
            Self::Mbc5(state) => state.selected_rom_bank(metadata),
            Self::HuC1(state) => state.selected_rom_bank(metadata),
            Self::HuC3(state) => state.selected_rom_bank(metadata),
            Self::Mbc7(state) => state.selected_rom_bank(metadata),
            Self::Mmm01(state) => state.selected_rom_bank(metadata),
            Self::Mbc6(state) => state.selected_rom_bank(metadata),
            Self::PocketCamera(state) => state.selected_rom_bank(metadata),
            Self::BandaiTama5(state) => state.selected_rom_bank(metadata),
        }
    }

    pub(super) fn lower_rom_bank(&self, _metadata: &CartridgeMetadata) -> usize {
        match self {
            Self::RomOnly => 0,
            Self::Mbc1(state) => state.lower_rom_bank(_metadata),
            Self::Mbc2(_) | Self::Mbc3(_) | Self::Mbc5(_) | Self::Mbc6(_) | Self::Mbc7(_) | Self::Mmm01(_) | Self::PocketCamera(_) | Self::BandaiTama5(_) | Self::HuC1(_) | Self::HuC3(_) => 0,
        }
    }

    /// Whether RAM access at 0xA000-0xBFFF uses the standard read/write_ram_bank
    /// pattern (ram_enabled + bank select + flat array).  When false, the MBC has
    /// exotic RAM handling (RTC, EEPROM, IR, etc.) that requires the channel path.
    pub(super) fn uses_standard_ram(&self) -> bool {
        matches!(
            self,
            Self::RomOnly | Self::Mbc1(_) | Self::Mbc5(_) | Self::Mmm01(_)
        )
    }

    pub(super) fn ram_enabled(&self) -> bool {
        match self {
            Self::RomOnly => true,
            Self::Mbc1(state) => state.ram_enabled,
            Self::Mbc5(state) => state.ram_enabled,
            Self::Mmm01(state) => state.ram_enabled,
            // Exotic types: not applicable for direct access.
            _ => false,
        }
    }

    pub(super) fn selected_ram_bank(&self, metadata: &CartridgeMetadata) -> usize {
        match self {
            Self::RomOnly => 0.min(metadata.ram_bank_count.saturating_sub(1)),
            Self::Mbc1(state) => state.selected_ram_bank(metadata),
            Self::Mbc2(_) => 0,
            Self::Mbc3(state) => state.selected_ram_bank(metadata),
            Self::Mbc5(state) => state.selected_ram_bank(metadata),
            Self::HuC1(state) => state.selected_ram_bank(metadata),
            Self::HuC3(state) => state.selected_ram_bank(metadata),
            Self::Mbc7(_) => 0,
            Self::Mmm01(state) => state.selected_ram_bank(metadata),
            Self::Mbc6(_) => 0,
            Self::PocketCamera(state) => state.selected_ram_bank(),
            Self::BandaiTama5(_) => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Mbc1Mode {
    RomBanking,
    RamBanking,
}

#[derive(Debug)]
pub(super) struct Mbc1State {
    pub(super) ram_enabled: bool,
    pub(super) rom_bank_low5: u8,
    pub(super) bank_high2: u8,
    pub(super) mode: Mbc1Mode,
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

pub(super) const MBC2_RAM_SIZE: usize = 512;

#[derive(Debug)]
pub(super) struct Mbc2State {
    pub(super) ram_enabled: bool,
    pub(super) rom_bank: u8,
    pub(super) ram: [u8; MBC2_RAM_SIZE],
}

impl Mbc2State {
    pub(super) fn new(boot_seed: u64) -> Self {
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
pub(super) struct Mbc3State {
    pub(super) ram_enabled: bool,
    pub(super) rom_bank: u8,
    pub(super) bank_select: u8,
    pub(super) latch_prep: bool,
    pub(super) rtc: RtcState,
}

#[derive(Debug)]
pub(super) struct RtcState {
    pub(super) seconds: u8,
    pub(super) minutes: u8,
    pub(super) hours: u8,
    pub(super) day_low: u8,
    pub(super) day_high: u8,
    pub(super) latched_seconds: u8,
    pub(super) latched_minutes: u8,
    pub(super) latched_hours: u8,
    pub(super) latched_day_low: u8,
    pub(super) latched_day_high: u8,
    pub(super) base_instant: Instant,
}

impl Mbc3State {
    pub(super) fn new() -> Self {
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
                // Standard MBC3 uses a 7-bit bank register; bank 0 maps to 1.
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

    pub(super) fn write_register(&mut self, register: u8, value: u8) {
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
pub(super) struct Mbc5State {
    pub(super) ram_enabled: bool,
    pub(super) rom_bank_low: u8,
    pub(super) rom_bank_high: u8,
    pub(super) ram_bank: u8,
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

/// HuC1 — Hudson Soft MBC with infrared port.
///
/// Banking is essentially MBC1: 5-bit ROM bank register, 2-bit upper register
/// for RAM banking / upper ROM bits, plus a ROM/RAM mode select.  Writing
/// `0x0E` to `0x0000-0x1FFF` enters IR mode; any other value enables/disables
/// RAM the same way MBC1 does (low nibble == 0x0A → enabled).
///
/// In IR mode, reads from `0xA000-0xBFFF` return `0xC0` (no light detected)
/// in bit 0 = 0 and the remaining bits high.  Writes are accepted but the
/// emulated IR LED is a no-op.
#[derive(Debug)]
pub(super) struct HuC1State {
    pub(super) ram_enabled: bool,
    pub(super) rom_bank_low5: u8,
    pub(super) bank_high2: u8,
    pub(super) mode: Mbc1Mode,
    pub(super) ir_mode: bool,
}

impl HuC1State {
    fn read(
        &self,
        metadata: &CartridgeMetadata,
        rom: &[u8],
        ram: &[u8],
        address: u16,
    ) -> ReadResult {
        match address {
            0x0000..=0x3fff => read_rom_bank(rom, metadata, 0, address),
            0x4000..=0x7fff => read_rom_bank(
                rom,
                metadata,
                self.selected_rom_bank(metadata),
                address - 0x4000,
            ),
            0xa000..=0xbfff if self.ir_mode => {
                // IR sensor: bit 0 = 1 when light detected. No light → 0xC0.
                ReadResult::Ready(0xc0)
            }
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
                if value == 0x0e {
                    self.ir_mode = true;
                    self.ram_enabled = false;
                } else {
                    self.ir_mode = false;
                    self.ram_enabled = value & 0x0f == 0x0a;
                }
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
            0xa000..=0xbfff if self.ir_mode => {
                // IR LED write — accepted but no-op.
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

impl Default for HuC1State {
    fn default() -> Self {
        Self {
            ram_enabled: false,
            rom_bank_low5: 1,
            bank_high2: 0,
            mode: Mbc1Mode::RomBanking,
            ir_mode: false,
        }
    }
}

/// HuC3 — Hudson Soft mapper with command-driven RTC, IR, and tone generator.
///
/// ROM banking is MBC3-style (7-bit register, bank 0 → 1).  RAM banking uses
/// a 4-bit register selecting one of up to 4 banks.  The mode register at
/// `0x0000-0x1FFF` switches between:
///   - `0x00`: RAM disabled
///   - `0x0A`: RAM enabled (standard read/write)
///   - `0x0B`: Register/command mode (reads/writes go through the HuC3 state machine)
///   - `0x0E`: IR mode (reads return `0xC1`; writes accepted but no-op)
///
/// In command mode (`0x0B`), writes to `0xA000-0xBFFF` drive a nibble-based
/// state machine.  The upper nibble of the written byte selects the command:
///   - `0x1_`: Shift-read — pops the low nibble of the 24-bit shift register
///             into `output` and shifts right by 4.
///   - `0x3_`: Shift-write — shifts the register left by 4 and inserts the
///             written low nibble.
///   - `0x4_`: Register command — the low nibble selects the operation:
///       - `0`: Load minutes into shift register.
///       - `1`: Load days into shift register.
///       - `2`: Load zero (used for semaphore/status reads).
///       - `3`: Write minutes from shift register.
///       - `4`: Write days from shift register.
///   - `0x6_`: Tone control (stub — accepted, no audio output).
///
/// Reads from `0xA000-0xBFFF` in command mode return `0x01 | (output << 1)`.
#[derive(Debug)]
pub(super) struct HuC3State {
    pub(super) mode: u8,
    pub(super) rom_bank: u8,
    pub(super) ram_bank: u8,
    /// 24-bit shift register used by the command protocol.
    pub(super) shift: u32,
    /// Last nibble read out by a shift-read command.
    pub(super) output: u8,
    pub(super) rtc: HuC3Rtc,
}

#[derive(Debug)]
pub(super) struct HuC3Rtc {
    /// Minutes of the current day (0-1439).
    pub(super) minutes: u16,
    /// Day counter.
    pub(super) days: u16,
    /// Base instant for tracking real-time elapsed minutes.
    pub(super) base_instant: Instant,
}

impl HuC3State {
    pub(super) fn new() -> Self {
        Self {
            mode: 0x00,
            rom_bank: 1,
            ram_bank: 0,
            shift: 0,
            output: 0,
            rtc: HuC3Rtc::new(),
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
            0xa000..=0xbfff => match self.mode {
                0x0a => read_ram_bank(ram, metadata, self.ram_bank as usize, address - 0xa000),
                0x0b => ReadResult::Ready(0x01 | (self.output << 1)),
                0x0e => ReadResult::Ready(0xc1), // IR: bit 0 = 1 (no light)
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
                self.mode = value;
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
                self.ram_bank = value & 0x0f;
                WriteResult::Accepted
            }
            0xa000..=0xbfff => match self.mode {
                0x0a => write_ram_bank(
                    ram,
                    metadata,
                    self.ram_bank as usize,
                    address - 0xa000,
                    value,
                ),
                0x0b => {
                    self.handle_command(value);
                    WriteResult::Accepted
                }
                0x0e => WriteResult::Accepted, // IR write — no-op
                _ => WriteResult::NoData,
            },
            _ => WriteResult::NoData,
        }
    }

    fn handle_command(&mut self, value: u8) {
        let cmd = value >> 4;
        let data = value & 0x0f;

        match cmd {
            0x1 => {
                // Shift-read: pop low nibble of shift register.
                self.output = (self.shift & 0x0f) as u8;
                self.shift >>= 4;
            }
            0x3 => {
                // Shift-write: push nibble into shift register.
                self.shift = ((self.shift << 4) | data as u32) & 0x00ff_ffff;
            }
            0x4 => {
                // Register command — subcmd selects action.
                self.rtc.advance();
                match data {
                    0x0 => {
                        // Read minutes into shift register.
                        self.shift = self.rtc.minutes as u32;
                    }
                    0x1 => {
                        // Read days into shift register.
                        self.shift = self.rtc.days as u32;
                    }
                    0x2 => {
                        // Status/semaphore — return 0.
                        self.shift = 0;
                    }
                    0x3 => {
                        // Write minutes from shift register.
                        self.rtc.minutes = (self.shift as u16) % 1440;
                    }
                    0x4 => {
                        // Write days from shift register.
                        self.rtc.days = self.shift as u16;
                    }
                    _ => {}
                }
            }
            0x6 => {
                // Tone control — accepted but no audio output.
            }
            _ => {}
        }
    }

    fn selected_rom_bank(&self, metadata: &CartridgeMetadata) -> usize {
        (self.rom_bank as usize) % metadata.rom_bank_count
    }

    fn selected_ram_bank(&self, metadata: &CartridgeMetadata) -> usize {
        if metadata.ram_bank_count == 0 {
            return 0;
        }
        (self.ram_bank as usize) % metadata.ram_bank_count
    }
}

impl HuC3Rtc {
    fn new() -> Self {
        Self {
            minutes: 0,
            days: 0,
            base_instant: Instant::now(),
        }
    }

    fn advance(&mut self) {
        let elapsed_secs = self.base_instant.elapsed().as_secs();
        self.base_instant = Instant::now();

        if elapsed_secs == 0 {
            return;
        }

        let elapsed_mins = elapsed_secs / 60;
        let total_mins = self.minutes as u64 + elapsed_mins;
        self.minutes = (total_mins % 1440) as u16;
        self.days = self.days.wrapping_add((total_mins / 1440) as u16);
    }
}

// ---------------------------------------------------------------------------
// MBC7 — Accelerometer + 93LC56 SPI EEPROM
// ---------------------------------------------------------------------------

/// MBC7 — ROM banking (8-bit) with an ADXL202 accelerometer and a 93LC56
/// SPI EEPROM (128 × 16-bit words = 256 bytes).
///
/// Memory map (when both RAM enables are active):
///   - `0xA000/0xA010`: Accelerometer latch control (write 0x55 then 0xAA)
///   - `0xA020-0xA030`: Accelerometer X (low/high bytes)
///   - `0xA040-0xA050`: Accelerometer Y (low/high bytes)
///   - `0xA060`: Always 0x00
///   - `0xA070`: Always 0xFF
///   - `0xA080`: EEPROM bit-banged I/O
///
/// Two-step RAM enable: write `0x0A` to `0x0000-0x1FFF`, then `0x40` to
/// `0x4000-0x5FFF`.  Both must be set for register access.
#[derive(Debug)]
pub(super) struct Mbc7State {
    pub(super) ram_enable_1: bool,
    pub(super) ram_enable_2: bool,
    pub(super) rom_bank: u8,
    pub(super) latch_ready: bool,
    pub(super) accel_x: u16,
    pub(super) accel_y: u16,
    pub(super) eeprom: Eeprom93LC56,
}

/// ADXL202 accelerometer center value (no tilt).
const ACCEL_CENTER: u16 = 0x8000;

pub(super) const EEPROM_WORD_COUNT: usize = 128;
const EEPROM_BYTE_COUNT: usize = EEPROM_WORD_COUNT * 2;

impl Mbc7State {
    pub(super) fn new() -> Self {
        Self {
            ram_enable_1: false,
            ram_enable_2: false,
            rom_bank: 1,
            latch_ready: false,
            accel_x: ACCEL_CENTER,
            accel_y: ACCEL_CENTER,
            eeprom: Eeprom93LC56::new(),
        }
    }

    fn registers_active(&self) -> bool {
        self.ram_enable_1 && self.ram_enable_2
    }

    fn read(
        &self,
        metadata: &CartridgeMetadata,
        rom: &[u8],
        address: u16,
    ) -> ReadResult {
        match address {
            0x0000..=0x3fff => read_rom_bank(rom, metadata, 0, address),
            0x4000..=0x7fff => {
                read_rom_bank(rom, metadata, self.selected_rom_bank(metadata), address - 0x4000)
            }
            0xa000..=0xafff if self.registers_active() => {
                let reg = (address >> 4) & 0x0f;
                let value = match reg {
                    0x0 | 0x1 => 0x00,
                    0x2 => (self.accel_x & 0x00ff) as u8,
                    0x3 => (self.accel_x >> 8) as u8,
                    0x4 => (self.accel_y & 0x00ff) as u8,
                    0x5 => (self.accel_y >> 8) as u8,
                    0x6 => 0x00,
                    0x7 => 0xff,
                    0x8 => self.eeprom.read_do(),
                    _ => 0xff,
                };
                ReadResult::Ready(value)
            }
            0xa000..=0xbfff => ReadResult::NoData,
            _ => ReadResult::NoData,
        }
    }

    fn write(
        &mut self,
        _metadata: &CartridgeMetadata,
        address: u16,
        value: u8,
    ) -> WriteResult {
        match address {
            0x0000..=0x1fff => {
                self.ram_enable_1 = value == 0x0a;
                if !self.ram_enable_1 {
                    self.ram_enable_2 = false;
                }
                WriteResult::Accepted
            }
            0x2000..=0x3fff => {
                self.rom_bank = if value == 0 { 1 } else { value };
                WriteResult::Accepted
            }
            0x4000..=0x5fff => {
                self.ram_enable_2 = self.ram_enable_1 && value == 0x40;
                WriteResult::Accepted
            }
            0xa000..=0xafff if self.registers_active() => {
                let reg = (address >> 4) & 0x0f;
                match reg {
                    0x0 => {
                        if value == 0x55 {
                            self.latch_ready = true;
                        } else {
                            self.latch_ready = false;
                        }
                    }
                    0x1 => {
                        if value == 0xaa && self.latch_ready {
                            // Latch accelerometer — stubbed to center (no tilt).
                            self.accel_x = ACCEL_CENTER;
                            self.accel_y = ACCEL_CENTER;
                        }
                        self.latch_ready = false;
                    }
                    0x8 => {
                        self.eeprom.write(value);
                    }
                    _ => {}
                }
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

/// 93LC56 SPI EEPROM — 128 × 16-bit words, bit-banged via CS/CLK/DI/DO lines.
///
/// Protocol: assert CS, clock in a start bit (1), then 2 opcode bits + 7
/// address bits.  Depending on the opcode the EEPROM then reads or writes
/// 16 data bits, or executes a special command.
///
/// Opcodes (after start bit):
///   - `10` + addr: READ — clock out 16 bits from the addressed word.
///   - `01` + addr: WRITE — clock in 16 bits to the addressed word (if enabled).
///   - `11` + addr: ERASE — set addressed word to 0xFFFF (if enabled).
///   - `00` + `11xxxxx`: EWEN — enable writes/erases.
///   - `00` + `00xxxxx`: EWDS — disable writes/erases.
///   - `00` + `10xxxxx`: ERAL — erase all words (if enabled).
///   - `00` + `01xxxxx`: WRAL — write all words from next 16 bits (if enabled).
#[derive(Debug)]
pub(super) struct Eeprom93LC56 {
    pub(super) data: [u16; EEPROM_WORD_COUNT],
    write_enabled: bool,
    cs: bool,
    clk: bool,
    di: bool,
    do_bit: bool,
    /// Accumulated command bits (start + opcode + address = up to 10 bits).
    command_buffer: u16,
    bits_in: u8,
    state: EepromPhase,
    /// Data shift register for read/write transfers.
    data_buffer: u16,
    data_bits: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EepromPhase {
    /// Waiting for start bit, then collecting opcode+address (10 bits total).
    Command,
    /// Clocking out 16 data bits (MSB first) on the DO line.
    Reading,
    /// Clocking in 16 data bits (MSB first) from the DI line.
    Writing,
    /// Write/erase completed — DO = 1 (ready).
    Done,
}

impl Eeprom93LC56 {
    pub(super) fn new() -> Self {
        Self {
            data: [0xffff; EEPROM_WORD_COUNT],
            write_enabled: false,
            cs: false,
            clk: false,
            di: false,
            do_bit: true,
            command_buffer: 0,
            bits_in: 0,
            state: EepromPhase::Command,
            data_buffer: 0,
            data_bits: 0,
        }
    }

    fn read_do(&self) -> u8 {
        let mut value = 0u8;
        if self.do_bit {
            value |= 0x01;
        }
        if self.di {
            value |= 0x02;
        }
        if self.clk {
            value |= 0x40;
        }
        if self.cs {
            value |= 0x80;
        }
        value
    }

    fn write(&mut self, value: u8) {
        let new_cs = value & 0x80 != 0;
        let new_clk = value & 0x40 != 0;
        self.di = value & 0x02 != 0;

        // CS falling edge → reset.
        if self.cs && !new_cs {
            self.reset();
            self.cs = false;
            self.clk = false;
            return;
        }

        // CS rising edge → begin new command.
        if !self.cs && new_cs {
            self.reset();
            self.cs = true;
            self.clk = new_clk;
            return;
        }

        self.cs = new_cs;
        if !self.cs {
            self.clk = new_clk;
            return;
        }

        // Rising edge of CLK while CS is high → shift.
        if !self.clk && new_clk {
            self.clock_rising_edge();
        }
        self.clk = new_clk;
    }

    fn reset(&mut self) {
        self.command_buffer = 0;
        self.bits_in = 0;
        self.state = EepromPhase::Command;
        self.data_buffer = 0;
        self.data_bits = 0;
        self.do_bit = true;
    }

    fn clock_rising_edge(&mut self) {
        match self.state {
            EepromPhase::Command => {
                self.command_buffer = (self.command_buffer << 1) | (self.di as u16);
                self.bits_in += 1;

                // Skip leading zeros before start bit.
                if self.bits_in == 1 && self.command_buffer == 0 {
                    self.bits_in = 0;
                    return;
                }

                // Need 10 bits: 1 start + 2 opcode + 7 address.
                if self.bits_in < 10 {
                    return;
                }

                self.execute_command();
            }
            EepromPhase::Reading => {
                // Clock out MSB of data_buffer.
                self.do_bit = self.data_buffer & 0x8000 != 0;
                self.data_buffer <<= 1;
                self.data_bits += 1;
                if self.data_bits >= 16 {
                    self.state = EepromPhase::Done;
                }
            }
            EepromPhase::Writing => {
                self.data_buffer = (self.data_buffer << 1) | (self.di as u16);
                self.data_bits += 1;
                if self.data_bits >= 16 {
                    self.finish_write();
                }
            }
            EepromPhase::Done => {
                self.do_bit = true;
            }
        }
    }

    fn execute_command(&mut self) {
        // command_buffer format: 1_OO_AAAAAAA (10 bits, MSB is start bit)
        let opcode = (self.command_buffer >> 7) & 0x03;
        let address = (self.command_buffer & 0x7f) as usize;

        match opcode {
            0b10 => {
                // READ
                let addr = address % EEPROM_WORD_COUNT;
                self.data_buffer = self.data[addr];
                self.data_bits = 0;
                self.do_bit = false; // dummy bit before data
                self.state = EepromPhase::Reading;
            }
            0b01 => {
                // WRITE — collect 16 data bits
                if self.write_enabled {
                    self.data_buffer = 0;
                    self.data_bits = 0;
                    self.state = EepromPhase::Writing;
                } else {
                    self.state = EepromPhase::Done;
                }
            }
            0b11 => {
                // ERASE
                if self.write_enabled {
                    let addr = address % EEPROM_WORD_COUNT;
                    self.data[addr] = 0xffff;
                }
                self.do_bit = true;
                self.state = EepromPhase::Done;
            }
            0b00 => {
                // Special commands — top 2 bits of address field select sub-op.
                let sub = (address >> 5) & 0x03;
                match sub {
                    0b11 => self.write_enabled = true,  // EWEN
                    0b00 => self.write_enabled = false,  // EWDS
                    0b10 => {
                        // ERAL — erase all
                        if self.write_enabled {
                            self.data.fill(0xffff);
                        }
                    }
                    0b01 => {
                        // WRAL — write all (need 16 data bits)
                        if self.write_enabled {
                            self.data_buffer = 0;
                            self.data_bits = 0;
                            self.state = EepromPhase::Writing;
                            return;
                        }
                    }
                    _ => {}
                }
                self.do_bit = true;
                self.state = EepromPhase::Done;
            }
            _ => unreachable!(),
        }
    }

    fn finish_write(&mut self) {
        let opcode = (self.command_buffer >> 7) & 0x03;
        let address = (self.command_buffer & 0x7f) as usize;

        match opcode {
            0b01 => {
                // WRITE single word.
                let addr = address % EEPROM_WORD_COUNT;
                self.data[addr] = self.data_buffer;
            }
            0b00 if (address >> 5) & 0x03 == 0b01 => {
                // WRAL — write all words.
                self.data.fill(self.data_buffer);
            }
            _ => {}
        }

        self.do_bit = true;
        self.state = EepromPhase::Done;
    }

    pub(super) fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(EEPROM_BYTE_COUNT);
        for word in &self.data {
            bytes.push((word >> 8) as u8);   // big-endian
            bytes.push(*word as u8);
        }
        bytes
    }

    pub(super) fn load_bytes(&mut self, data: &[u8]) {
        for (i, word) in self.data.iter_mut().enumerate() {
            let offset = i * 2;
            if offset + 1 < data.len() {
                *word = ((data[offset] as u16) << 8) | data[offset + 1] as u16;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// MMM01 — Multi-game compilation cartridge mapper
// ---------------------------------------------------------------------------

/// MMM01 — Multicart mapper with two-stage boot.
///
/// On power-up the mapper is **unmapped**: ROM bank 0 reads from the
/// second-to-last physical bank and bank 1 from the last physical bank.  This
/// lets the game-selection menu live at the end of the ROM.
///
/// The menu configures a base ROM bank via `0x2000-0x3FFF` (and optionally
/// high bits via `0x4000-0x5FFF`), then writes to `0x0000-0x1FFF` to latch
/// the base and enter **mapped mode** permanently.
///
/// In mapped mode the mapper behaves like MBC1: the latched base is OR'd with
/// the 5-bit switchable bank register for ROM accesses.  RAM banking and mode
/// select work identically to MBC1.
#[derive(Debug)]
pub(super) struct Mmm01State {
    pub(super) mapped: bool,
    pub(super) ram_enabled: bool,
    pub(super) rom_bank_low5: u8,
    pub(super) bank_high2: u8,
    pub(super) mode: Mbc1Mode,
    /// Latched base ROM bank — set when mapping is enabled.
    pub(super) rom_bank_base: usize,
}

impl Mmm01State {
    pub(super) fn new() -> Self {
        Self {
            mapped: false,
            ram_enabled: false,
            rom_bank_low5: 1,
            bank_high2: 0,
            mode: Mbc1Mode::RomBanking,
            rom_bank_base: 0,
        }
    }

    fn read(
        &self,
        metadata: &CartridgeMetadata,
        rom: &[u8],
        ram: &[u8],
        address: u16,
    ) -> ReadResult {
        if !self.mapped {
            return self.read_unmapped(metadata, rom, address);
        }

        match address {
            0x0000..=0x3fff => {
                read_rom_bank(rom, metadata, self.rom_bank_base, address)
            }
            0x4000..=0x7fff => {
                let bank = self.selected_rom_bank(metadata);
                read_rom_bank(rom, metadata, bank, address - 0x4000)
            }
            0xa000..=0xbfff if self.ram_enabled => {
                read_ram_bank(ram, metadata, self.selected_ram_bank(metadata), address - 0xa000)
            }
            0xa000..=0xbfff => ReadResult::NoData,
            _ => ReadResult::NoData,
        }
    }

    fn read_unmapped(
        &self,
        metadata: &CartridgeMetadata,
        rom: &[u8],
        address: u16,
    ) -> ReadResult {
        match address {
            0x0000..=0x3fff => {
                let bank = metadata.rom_bank_count.saturating_sub(2);
                read_rom_bank(rom, metadata, bank, address)
            }
            0x4000..=0x7fff => {
                let bank = metadata.rom_bank_count.saturating_sub(1);
                read_rom_bank(rom, metadata, bank, address - 0x4000)
            }
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
        if !self.mapped {
            return self.write_unmapped(metadata, address, value);
        }

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
            0xa000..=0xbfff if self.ram_enabled => {
                write_ram_bank(
                    ram,
                    metadata,
                    self.selected_ram_bank(metadata),
                    address - 0xa000,
                    value,
                )
            }
            0xa000..=0xbfff => WriteResult::NoData,
            _ => WriteResult::NoData,
        }
    }

    fn write_unmapped(
        &mut self,
        metadata: &CartridgeMetadata,
        address: u16,
        value: u8,
    ) -> WriteResult {
        match address {
            0x0000..=0x1fff => {
                // Latch current register values as base and enter mapped mode.
                let base = ((self.bank_high2 as usize) << 5) | self.rom_bank_low5 as usize;
                self.rom_bank_base = base % metadata.rom_bank_count;
                self.ram_enabled = value & 0x0f == 0x0a;
                self.mapped = true;
                // Reset bank registers for the game's own banking.
                self.rom_bank_low5 = 1;
                self.bank_high2 = 0;
                self.mode = Mbc1Mode::RomBanking;
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
            _ => WriteResult::NoData,
        }
    }

    fn selected_rom_bank(&self, metadata: &CartridgeMetadata) -> usize {
        let low = match self.rom_bank_low5 {
            0 => 1,
            v => v as usize,
        };
        (self.rom_bank_base | low) % metadata.rom_bank_count
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

// ---------------------------------------------------------------------------
// MBC6 — Dual ROM banks + Flash ROM + split RAM
// ---------------------------------------------------------------------------

const MBC6_ROM_BANK_SIZE: usize = 0x2000; // 8 KiB
const MBC6_RAM_BANK_SIZE: usize = 0x1000; // 4 KiB

/// MBC6 — Dual 8 KiB ROM/Flash bank windows and dual 4 KiB RAM bank windows.
///
/// Used only by *Net de Get: Minigame @ 100*.  The cartridge contains flash
/// ROM that can be reprogrammed at runtime (for storing downloaded minigames).
///
/// Memory map:
///   - `0x0000-0x3FFF`: Fixed ROM (first 16 KiB)
///   - `0x4000-0x5FFF`: ROM/Flash bank A (8 KiB, switchable)
///   - `0x6000-0x7FFF`: ROM/Flash bank B (8 KiB, switchable)
///   - `0xA000-0xAFFF`: RAM bank A (4 KiB, switchable)
///   - `0xB000-0xBFFF`: RAM bank B (4 KiB, switchable)
///
/// Flash programming follows standard command sequences (AA/55/cmd at offsets
/// `0x0AAA`/`0x0555` within the bank window).
#[derive(Debug)]
pub(super) struct Mbc6State {
    pub(super) ram_enabled: bool,
    pub(super) ram_bank_a: u8,
    pub(super) ram_bank_b: u8,
    pub(super) rom_bank_a: u8,
    pub(super) rom_bank_b: u8,
    pub(super) flash_select_a: bool,
    pub(super) flash_select_b: bool,
    pub(super) flash_write_enabled: bool,
    /// Flash data — full copy of ROM, modified by flash programming.
    pub(super) flash: Vec<u8>,
    flash_state: FlashPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlashPhase {
    Ready,
    AwaitCmd55,
    AwaitCmd,
    Programming,
    EraseAwaitAA,
    EraseAwait55,
    EraseAwaitSector,
}

impl Mbc6State {
    pub(super) fn new(rom: &[u8]) -> Self {
        Self {
            ram_enabled: false,
            ram_bank_a: 0,
            ram_bank_b: 0,
            rom_bank_a: 2,
            rom_bank_b: 3,
            flash_select_a: false,
            flash_select_b: false,
            flash_write_enabled: false,
            flash: rom.to_vec(),
            flash_state: FlashPhase::Ready,
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
            0x4000..=0x5fff => self.read_flash_bank(self.rom_bank_a, address - 0x4000),
            0x6000..=0x7fff => self.read_flash_bank(self.rom_bank_b, address - 0x6000),
            0xa000..=0xafff if self.ram_enabled => {
                let offset = self.ram_bank_a as usize * MBC6_RAM_BANK_SIZE
                    + (address as usize - 0xa000);
                if offset < ram.len() {
                    ReadResult::Ready(ram[offset])
                } else {
                    ReadResult::NoData
                }
            }
            0xb000..=0xbfff if self.ram_enabled => {
                let offset = self.ram_bank_b as usize * MBC6_RAM_BANK_SIZE
                    + (address as usize - 0xb000);
                if offset < ram.len() {
                    ReadResult::Ready(ram[offset])
                } else {
                    ReadResult::NoData
                }
            }
            0xa000..=0xbfff => ReadResult::NoData,
            _ => ReadResult::NoData,
        }
    }

    fn read_flash_bank(&self, bank: u8, offset: u16) -> ReadResult {
        let index = bank as usize * MBC6_ROM_BANK_SIZE + offset as usize;
        if index < self.flash.len() {
            ReadResult::Ready(self.flash[index])
        } else {
            ReadResult::NoData
        }
    }

    fn write(
        &mut self,
        _metadata: &CartridgeMetadata,
        ram: &mut [u8],
        address: u16,
        value: u8,
    ) -> WriteResult {
        match address {
            0x0000..=0x03ff => {
                self.ram_enabled = value & 0x0f == 0x0a;
                WriteResult::Accepted
            }
            0x0400..=0x07ff => {
                self.ram_bank_a = value & 0x07;
                WriteResult::Accepted
            }
            0x0800..=0x0bff => {
                self.ram_bank_b = value & 0x07;
                WriteResult::Accepted
            }
            0x0c00..=0x0fff => {
                self.flash_write_enabled = value == 0x01;
                if !self.flash_write_enabled {
                    self.flash_state = FlashPhase::Ready;
                }
                WriteResult::Accepted
            }
            0x2000..=0x27ff => {
                self.rom_bank_a = value & 0x7f;
                WriteResult::Accepted
            }
            0x2800..=0x2fff => {
                self.flash_select_a = value & 0x08 != 0;
                WriteResult::Accepted
            }
            0x3000..=0x37ff => {
                self.rom_bank_b = value & 0x7f;
                WriteResult::Accepted
            }
            0x3800..=0x3fff => {
                self.flash_select_b = value & 0x08 != 0;
                WriteResult::Accepted
            }
            0x4000..=0x5fff if self.flash_write_enabled && self.flash_select_a => {
                self.flash_write(self.rom_bank_a, address - 0x4000, value);
                WriteResult::Accepted
            }
            0x6000..=0x7fff if self.flash_write_enabled && self.flash_select_b => {
                self.flash_write(self.rom_bank_b, address - 0x6000, value);
                WriteResult::Accepted
            }
            0x4000..=0x7fff => WriteResult::Accepted,
            0xa000..=0xafff if self.ram_enabled => {
                let offset = self.ram_bank_a as usize * MBC6_RAM_BANK_SIZE
                    + (address as usize - 0xa000);
                if offset < ram.len() {
                    ram[offset] = value;
                }
                WriteResult::Accepted
            }
            0xb000..=0xbfff if self.ram_enabled => {
                let offset = self.ram_bank_b as usize * MBC6_RAM_BANK_SIZE
                    + (address as usize - 0xb000);
                if offset < ram.len() {
                    ram[offset] = value;
                }
                WriteResult::Accepted
            }
            0xa000..=0xbfff => WriteResult::NoData,
            _ => WriteResult::NoData,
        }
    }

    fn flash_write(&mut self, bank: u8, offset: u16, value: u8) {
        let abs_offset = offset as usize;

        match self.flash_state {
            FlashPhase::Ready => {
                if abs_offset == 0x0aaa && value == 0xaa {
                    self.flash_state = FlashPhase::AwaitCmd55;
                } else if value == 0xf0 {
                    self.flash_state = FlashPhase::Ready;
                }
            }
            FlashPhase::AwaitCmd55 => {
                if abs_offset == 0x0555 && value == 0x55 {
                    self.flash_state = FlashPhase::AwaitCmd;
                } else {
                    self.flash_state = FlashPhase::Ready;
                }
            }
            FlashPhase::AwaitCmd => {
                if abs_offset == 0x0aaa {
                    match value {
                        0xa0 => self.flash_state = FlashPhase::Programming,
                        0x80 => self.flash_state = FlashPhase::EraseAwaitAA,
                        0xf0 => self.flash_state = FlashPhase::Ready,
                        _ => self.flash_state = FlashPhase::Ready,
                    }
                } else {
                    self.flash_state = FlashPhase::Ready;
                }
            }
            FlashPhase::Programming => {
                // Flash can only clear bits (AND with existing data).
                let index = bank as usize * MBC6_ROM_BANK_SIZE + abs_offset;
                if index < self.flash.len() {
                    self.flash[index] &= value;
                }
                self.flash_state = FlashPhase::Ready;
            }
            FlashPhase::EraseAwaitAA => {
                if abs_offset == 0x0aaa && value == 0xaa {
                    self.flash_state = FlashPhase::EraseAwait55;
                } else {
                    self.flash_state = FlashPhase::Ready;
                }
            }
            FlashPhase::EraseAwait55 => {
                if abs_offset == 0x0555 && value == 0x55 {
                    self.flash_state = FlashPhase::EraseAwaitSector;
                } else {
                    self.flash_state = FlashPhase::Ready;
                }
            }
            FlashPhase::EraseAwaitSector => {
                if value == 0x30 {
                    let sector_start = bank as usize * MBC6_ROM_BANK_SIZE;
                    let sector_end = (sector_start + MBC6_ROM_BANK_SIZE).min(self.flash.len());
                    if sector_start < self.flash.len() {
                        self.flash[sector_start..sector_end].fill(0xff);
                    }
                }
                self.flash_state = FlashPhase::Ready;
            }
        }
    }

    fn selected_rom_bank(&self, _metadata: &CartridgeMetadata) -> usize {
        self.rom_bank_a as usize
    }
}

// ---------------------------------------------------------------------------
// Pocket Camera (Game Boy Camera) — MBC5-like + camera ASIC
// ---------------------------------------------------------------------------

const CAMERA_REG_COUNT: usize = 0x36;

/// Pocket Camera — MBC5-style banking with a M64282FP camera sensor ASIC.
///
/// ROM banking is identical to MBC5 (9-bit bank register).  RAM has 16 × 8 KiB
/// banks (128 KiB total, battery-backed) used for photo storage.  Setting the
/// RAM bank register to `0x10` switches the `0xA000-0xBFFF` window to camera
/// hardware registers instead of RAM.
///
/// Camera register map (`0xA000-0xA035`, 54 bytes):
///   - `0xA000`: Trigger / status — write bit 0 = 1 to start capture;
///     reads 0 when capture is complete.
///   - `0xA001-0xA005`: Exposure, edge mode, gain, voltage reference.
///   - `0xA006-0xA035`: 4×4 dithering matrix (3 threshold sets).
///
/// On capture trigger the sensor is stubbed: bit 0 clears immediately
/// (instant completion) and RAM bank 0 at `0x0100-0x0EFF` is filled with
/// blank (white) tiles.
#[derive(Debug)]
pub(super) struct PocketCameraState {
    pub(super) ram_enabled: bool,
    pub(super) rom_bank_low: u8,
    pub(super) rom_bank_high: u8,
    pub(super) ram_bank: u8,
    /// When true, `0xA000-0xBFFF` accesses camera registers instead of RAM.
    pub(super) camera_mode: bool,
    pub(super) camera_regs: [u8; CAMERA_REG_COUNT],
}

impl PocketCameraState {
    pub(super) fn new() -> Self {
        Self {
            ram_enabled: false,
            rom_bank_low: 1,
            rom_bank_high: 0,
            ram_bank: 0,
            camera_mode: false,
            camera_regs: [0; CAMERA_REG_COUNT],
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
            0xa000..=0xbfff if self.camera_mode => {
                let offset = (address - 0xa000) as usize;
                if offset < CAMERA_REG_COUNT {
                    // Bit 0 of register 0 = 0 means capture complete (always ready).
                    ReadResult::Ready(self.camera_regs[offset] & if offset == 0 { 0xfe } else { 0xff })
                } else {
                    ReadResult::Ready(0x00)
                }
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
                if value & 0x10 != 0 {
                    self.camera_mode = true;
                } else {
                    self.camera_mode = false;
                    self.ram_bank = value & 0x0f;
                }
                WriteResult::Accepted
            }
            0xa000..=0xbfff if self.camera_mode => {
                let offset = (address - 0xa000) as usize;
                if offset < CAMERA_REG_COUNT {
                    self.camera_regs[offset] = value;
                    if offset == 0 && value & 0x01 != 0 {
                        self.trigger_capture(ram);
                    }
                }
                WriteResult::Accepted
            }
            0xa000..=0xbfff if self.ram_enabled => {
                write_ram_bank(ram, metadata, self.ram_bank as usize, address - 0xa000, value)
            }
            0xa000..=0xbfff => WriteResult::NoData,
            _ => WriteResult::NoData,
        }
    }

    fn trigger_capture(&mut self, ram: &mut [u8]) {
        // Stub: fill the capture tile buffer (RAM bank 0, offset 0x0100-0x0EFF)
        // with blank tiles (all zeros = white in 2bpp).
        let start = 0x0100;
        let end = 0x0f00.min(ram.len());
        if start < ram.len() {
            ram[start..end].fill(0x00);
        }
        // Clear capture-in-progress flag (bit 0 of register 0).
        self.camera_regs[0] &= 0xfe;
    }

    fn selected_rom_bank(&self, metadata: &CartridgeMetadata) -> usize {
        let bank = ((self.rom_bank_high as usize) << 8) | self.rom_bank_low as usize;
        bank % metadata.rom_bank_count
    }

    fn selected_ram_bank(&self) -> usize {
        self.ram_bank as usize
    }
}

// ---------------------------------------------------------------------------
// Bandai TAMA5 — Nibble command interface with ROM banking, internal RAM, RTC
// ---------------------------------------------------------------------------

pub(super) const TAMA5_RAM_SIZE: usize = 32;

/// Bandai TAMA5 — Command-driven mapper for Tamagotchi games.
///
/// All I/O goes through two registers at `0xA000-0xA001`:
///   - `0xA000` (write): **data register** — holds the low/high nibble of the
///     value being transferred.
///   - `0xA001` (write): **command/address register** — the upper 3 bits
///     select the command and the low 5 bits provide the address/sub-command.
///   - `0xA000` (read): returns the response value assembled from previous
///     read-back commands.
///
/// Commands (bits 5-7 of the value written to `0xA001`):
///   - `0`: Write low nibble of `data_reg` to RAM\[addr\].
///   - `1`: Write high nibble of `data_reg` to RAM\[addr\].
///   - `2`: Read low nibble — loads RAM\[addr\] low nibble into output.
///   - `3`: Read high nibble — loads RAM\[addr\] high nibble into output.
///   - `4`: Set ROM bank (low nibble of `data_reg`).
///   - `5`: Write low nibble of `data_reg` to RTC register\[addr & 7\].
///   - `6`: Read low nibble of RTC register\[addr & 7\] into output.
///   - `7`: Read high nibble of RTC register\[addr & 7\] into output.
///
/// Internal RAM is 32 bytes.  The RTC has 8 nibble-wide registers (seconds
/// through year).
#[derive(Debug)]
pub(super) struct Tama5State {
    pub(super) rom_bank: u8,
    pub(super) data_reg: u8,
    pub(super) output: u8,
    pub(super) ram: [u8; TAMA5_RAM_SIZE],
    pub(super) rtc: Tama5Rtc,
}

#[derive(Debug)]
pub(super) struct Tama5Rtc {
    /// 8 nibble-wide registers: seconds low/high, minutes low/high,
    /// hours low/high, day-of-week, day-counter.
    pub(super) regs: [u8; 8],
    pub(super) base_instant: Instant,
}

impl Tama5State {
    pub(super) fn new() -> Self {
        Self {
            rom_bank: 1,
            data_reg: 0,
            output: 0,
            ram: [0; TAMA5_RAM_SIZE],
            rtc: Tama5Rtc::new(),
        }
    }

    fn read(
        &self,
        metadata: &CartridgeMetadata,
        rom: &[u8],
        address: u16,
    ) -> ReadResult {
        match address {
            0x0000..=0x3fff => read_rom_bank(rom, metadata, 0, address),
            0x4000..=0x7fff => {
                read_rom_bank(rom, metadata, self.selected_rom_bank(metadata), address - 0x4000)
            }
            0xa000 => ReadResult::Ready(self.output),
            0xa001..=0xbfff => ReadResult::Ready(0xff),
            _ => ReadResult::NoData,
        }
    }

    fn write(
        &mut self,
        _metadata: &CartridgeMetadata,
        address: u16,
        value: u8,
    ) -> WriteResult {
        match address {
            0xa000 => {
                self.data_reg = value;
                WriteResult::Accepted
            }
            0xa001 => {
                self.execute_command(value);
                WriteResult::Accepted
            }
            0x0000..=0x7fff => WriteResult::Accepted,
            _ => WriteResult::NoData,
        }
    }

    fn execute_command(&mut self, cmd_byte: u8) {
        let cmd = (cmd_byte >> 5) & 0x07;
        let addr = (cmd_byte & 0x1f) as usize;

        match cmd {
            0 => {
                // Write low nibble of data_reg to RAM[addr].
                if addr < TAMA5_RAM_SIZE {
                    self.ram[addr] = (self.ram[addr] & 0xf0) | (self.data_reg & 0x0f);
                }
            }
            1 => {
                // Write high nibble of data_reg to RAM[addr].
                if addr < TAMA5_RAM_SIZE {
                    self.ram[addr] = (self.ram[addr] & 0x0f) | ((self.data_reg & 0x0f) << 4);
                }
            }
            2 => {
                // Read low nibble of RAM[addr] into output.
                if addr < TAMA5_RAM_SIZE {
                    self.output = self.ram[addr] & 0x0f;
                } else {
                    self.output = 0x0f;
                }
            }
            3 => {
                // Read high nibble of RAM[addr] into output.
                if addr < TAMA5_RAM_SIZE {
                    self.output = (self.ram[addr] >> 4) & 0x0f;
                } else {
                    self.output = 0x0f;
                }
            }
            4 => {
                // Set ROM bank.
                let bank = self.data_reg & 0x0f;
                self.rom_bank = if bank == 0 { 1 } else { bank };
            }
            5 => {
                // Write low nibble to RTC register.
                self.rtc.advance();
                let reg = addr & 0x07;
                self.rtc.regs[reg] = (self.rtc.regs[reg] & 0xf0) | (self.data_reg & 0x0f);
            }
            6 => {
                // Read low nibble of RTC register.
                self.rtc.advance();
                let reg = addr & 0x07;
                self.output = self.rtc.regs[reg] & 0x0f;
            }
            7 => {
                // Read high nibble of RTC register.
                self.rtc.advance();
                let reg = addr & 0x07;
                self.output = (self.rtc.regs[reg] >> 4) & 0x0f;
            }
            _ => {}
        }
    }

    fn selected_rom_bank(&self, metadata: &CartridgeMetadata) -> usize {
        (self.rom_bank as usize) % metadata.rom_bank_count
    }
}

impl Tama5Rtc {
    fn new() -> Self {
        Self {
            regs: [0; 8],
            base_instant: Instant::now(),
        }
    }

    fn advance(&mut self) {
        let elapsed_secs = self.base_instant.elapsed().as_secs();
        self.base_instant = Instant::now();

        if elapsed_secs == 0 {
            return;
        }

        // Decode current time from BCD-ish nibble registers.
        let secs = self.decode_bcd(0) + elapsed_secs;
        let total_secs = secs % 60;
        let carry_mins = secs / 60;

        let mins = self.decode_bcd(2) + carry_mins;
        let total_mins = mins % 60;
        let carry_hrs = mins / 60;

        let hrs = self.decode_bcd(4) + carry_hrs;
        let total_hrs = hrs % 24;
        let carry_days = hrs / 24;

        self.encode_bcd(0, total_secs);
        self.encode_bcd(2, total_mins);
        self.encode_bcd(4, total_hrs);

        if carry_days > 0 {
            // Register 6: day of week (0-6), register 7: day counter.
            let dow = (self.regs[6] as u64 + carry_days) % 7;
            self.regs[6] = dow as u8;
            self.regs[7] = self.regs[7].wrapping_add(carry_days as u8);
        }
    }

    /// Decode a 2-register BCD pair (low nibble + high nibble) starting at `base`.
    fn decode_bcd(&self, base: usize) -> u64 {
        let lo = (self.regs[base] & 0x0f) as u64;
        let hi = (self.regs[base + 1] & 0x0f) as u64;
        hi * 10 + lo
    }

    /// Encode a value as 2-register BCD pair starting at `base`.
    fn encode_bcd(&mut self, base: usize, value: u64) {
        self.regs[base] = (value % 10) as u8;
        self.regs[base + 1] = ((value / 10) % 10) as u8;
    }
}
