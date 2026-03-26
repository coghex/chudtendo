use std::fmt;

use super::types::{BootMode, MbcKind};

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
