use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use serde::Deserialize;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JoypadButton {
    Right,
    Left,
    Up,
    Down,
    A,
    B,
    Select,
    Start,
}

/// Shared joypad state readable by the CPU thread without message passing.
///
/// Layout of the packed byte:
///   bits 0-3: d-pad    (Right=0, Left=1, Up=2, Down=3)  — 0 = pressed
///   bits 4-7: buttons  (A=4, B=5, Select=6, Start=7)    — 0 = pressed
///
/// Initialized to 0xFF (all unpressed).
#[derive(Clone, Debug)]
pub struct JoypadState {
    packed: Arc<AtomicU8>,
}

impl JoypadState {
    pub fn new() -> Self {
        Self {
            packed: Arc::new(AtomicU8::new(0xff)),
        }
    }

    pub fn press(&self, button: JoypadButton) {
        let bit = button_bit(button);
        self.packed.fetch_and(!(1 << bit), Ordering::Release);
    }

    pub fn release(&self, button: JoypadButton) {
        let bit = button_bit(button);
        self.packed.fetch_or(1 << bit, Ordering::Release);
    }

    /// Read the joypad register value given the current select bits written
    /// to 0xFF00. Returns the byte the CPU should see.
    pub fn read(&self, select: u8) -> u8 {
        let packed = self.packed.load(Ordering::Acquire);
        let dpad = packed & 0x0f;
        let buttons = (packed >> 4) & 0x0f;

        let mut result = 0x0f;
        if select & 0x10 == 0 {
            result &= dpad;
        }
        if select & 0x20 == 0 {
            result &= buttons;
        }

        0xc0 | (select & 0x30) | result
    }

    /// Returns true if any selected button is pressed (useful for interrupt).
    pub fn any_pressed(&self, select: u8) -> bool {
        let value = self.read(select);
        value & 0x0f != 0x0f
    }
}

impl Default for JoypadState {
    fn default() -> Self {
        Self::new()
    }
}

fn button_bit(button: JoypadButton) -> u8 {
    match button {
        JoypadButton::Right => 0,
        JoypadButton::Left => 1,
        JoypadButton::Up => 2,
        JoypadButton::Down => 3,
        JoypadButton::A => 4,
        JoypadButton::B => 5,
        JoypadButton::Select => 6,
        JoypadButton::Start => 7,
    }
}

/// Backend-agnostic keybinding configuration.
///
/// Maps key name strings (e.g. "Z", "X", "Return", "Up") to joypad buttons.
/// Key names are case-insensitive on lookup.
#[derive(Clone, Debug)]
pub struct Keybindings {
    map: HashMap<String, JoypadButton>,
}

impl Keybindings {
    pub fn load_or_default(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(contents) => match Self::from_yaml(&contents) {
                Ok(kb) => return kb,
                Err(error) => {
                    eprintln!("warning: failed to parse {}: {error}", path.display());
                }
            },
            Err(_) => {}
        }
        Self::default()
    }

    pub fn from_yaml(yaml: &str) -> Result<Self, serde_yaml::Error> {
        let raw: HashMap<String, JoypadButton> = serde_yaml::from_str(yaml)?;
        let map = raw
            .into_iter()
            .map(|(key, button)| (key.to_lowercase(), button))
            .collect();
        Ok(Self { map })
    }

    /// Look up a joypad button for the given key name (case-insensitive).
    pub fn lookup(&self, key_name: &str) -> Option<JoypadButton> {
        self.map.get(&key_name.to_lowercase()).copied()
    }
}

impl Default for Keybindings {
    fn default() -> Self {
        let pairs = [
            ("right", JoypadButton::Right),
            ("left", JoypadButton::Left),
            ("up", JoypadButton::Up),
            ("down", JoypadButton::Down),
            ("z", JoypadButton::A),
            ("x", JoypadButton::B),
            ("backspace", JoypadButton::Select),
            ("return", JoypadButton::Start),
        ];
        let map = pairs
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect();
        Self { map }
    }
}
