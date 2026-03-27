#[cfg(target_os = "macos")]
#[macro_use]
extern crate objc;

pub mod app;
pub mod emulator;
pub mod input;
pub mod menu;
pub mod preferences;
pub mod settings;
pub mod shader;

pub use app::{RunMode, run, run_with_agent};
pub use input::{JoypadButton, JoypadState, Keybindings};
