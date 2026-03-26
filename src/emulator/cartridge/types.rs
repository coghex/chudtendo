use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MbcKind {
    RomOnly,
    Mbc1,
    Mbc2,
    Mmm01,
    Mbc3,
    Mbc5,
    Mbc6,
    Mbc7,
    HuC1,
    HuC3,
    PocketCamera,
    BandaiTama5,
    UnknownStub(u8),
}

impl MbcKind {
    pub fn from_header(cartridge_type: u8) -> Self {
        match cartridge_type {
            0x00 | 0x08 | 0x09 => Self::RomOnly,
            0x01..=0x03 => Self::Mbc1,
            0x05 | 0x06 => Self::Mbc2,
            0x0b..=0x0d => Self::Mmm01,
            0x0f..=0x13 => Self::Mbc3,
            0x19..=0x1e => Self::Mbc5,
            0x20 => Self::Mbc6,
            0x22 => Self::Mbc7,
            0xfc => Self::PocketCamera,
            0xfd => Self::BandaiTama5,
            0xfe => Self::HuC3,
            0xff => Self::HuC1,
            other => Self::UnknownStub(other),
        }
    }

    pub fn is_supported(self) -> bool {
        matches!(self, Self::RomOnly | Self::Mbc1 | Self::Mbc2 | Self::Mmm01 | Self::Mbc3 | Self::Mbc5 | Self::Mbc6 | Self::Mbc7 | Self::PocketCamera | Self::BandaiTama5 | Self::HuC1 | Self::HuC3)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::RomOnly => "ROM ONLY",
            Self::Mbc1 => "MBC1",
            Self::Mbc2 => "MBC2",
            Self::Mmm01 => "MMM01",
            Self::Mbc3 => "MBC3",
            Self::Mbc5 => "MBC5",
            Self::Mbc6 => "MBC6",
            Self::Mbc7 => "MBC7",
            Self::HuC1 => "HuC1",
            Self::HuC3 => "HuC3",
            Self::PocketCamera => "Pocket Camera",
            Self::BandaiTama5 => "Bandai TAMA5",
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
