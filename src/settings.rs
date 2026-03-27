use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

fn config_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.yaml")
}

fn load_raw() -> HashMap<String, serde_yaml::Value> {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|contents| serde_yaml::from_str(&contents).ok())
        .unwrap_or_default()
}

fn save_raw(config: &HashMap<String, serde_yaml::Value>) {
    if let Ok(yaml) = serde_yaml::to_string(config) {
        let _ = std::fs::write(config_path(), yaml);
    }
}

// ---------------------------------------------------------------------------
// Emulation
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EmulationSettings {
    /// Fast-forward speed multiplier. 0.0 = uncapped.
    pub ff_speed: f32,
    /// Rewind buffer duration in seconds.
    pub rewind_buffer_seconds: u32,
    /// Boot ROM variant: "cgb" or "dmg".
    pub boot_rom: String,
}

impl Default for EmulationSettings {
    fn default() -> Self {
        Self {
            ff_speed: 0.0,
            rewind_buffer_seconds: 30,
            boot_rom: "cgb".to_owned(),
        }
    }
}

impl EmulationSettings {
    pub fn load() -> Self {
        let raw = load_raw();
        Self {
            ff_speed: raw
                .get("ff_speed")
                .and_then(|v| v.as_f64())
                .map(|v| v as f32)
                .unwrap_or_else(|| Self::default().ff_speed),
            rewind_buffer_seconds: raw
                .get("rewind_buffer_seconds")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32)
                .unwrap_or_else(|| Self::default().rewind_buffer_seconds),
            boot_rom: raw
                .get("boot_rom")
                .and_then(|v| v.as_str())
                .map(|s| s.to_owned())
                .unwrap_or_else(|| Self::default().boot_rom),
        }
    }

    pub fn save(&self) {
        let mut raw = load_raw();
        raw.insert(
            "ff_speed".to_owned(),
            serde_yaml::Value::Number(serde_yaml::Number::from(self.ff_speed as f64)),
        );
        raw.insert(
            "rewind_buffer_seconds".to_owned(),
            serde_yaml::Value::Number(serde_yaml::Number::from(self.rewind_buffer_seconds as u64)),
        );
        raw.insert(
            "boot_rom".to_owned(),
            serde_yaml::Value::String(self.boot_rom.clone()),
        );
        save_raw(&raw);
    }
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WindowMode {
    Windowed,
    Fullscreen,
    Borderless,
}

impl WindowMode {
    pub const ALL: [WindowMode; 3] = [
        WindowMode::Windowed,
        WindowMode::Fullscreen,
        WindowMode::Borderless,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Windowed => "Windowed",
            Self::Fullscreen => "Fullscreen",
            Self::Borderless => "Borderless",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "windowed" => Some(Self::Windowed),
            "fullscreen" => Some(Self::Fullscreen),
            "borderless" => Some(Self::Borderless),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Windowed => "windowed",
            Self::Fullscreen => "fullscreen",
            Self::Borderless => "borderless",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DisplaySettings {
    pub window_scale: u32,
    pub window_mode: WindowMode,
    pub vsync: bool,
    pub frame_limit: u32,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            window_scale: 4,
            window_mode: WindowMode::Windowed,
            vsync: true,
            frame_limit: 0,
        }
    }
}

impl DisplaySettings {
    pub fn load() -> Self {
        let raw = load_raw();
        let defaults = Self::default();
        Self {
            window_scale: raw
                .get("window_scale")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32)
                .filter(|&s| s >= 1 && s <= 8)
                .unwrap_or(defaults.window_scale),
            window_mode: raw
                .get("window_mode")
                .and_then(|v| v.as_str())
                .and_then(WindowMode::from_str)
                .unwrap_or(defaults.window_mode),
            vsync: raw
                .get("vsync")
                .and_then(|v| v.as_bool())
                .unwrap_or(defaults.vsync),
            frame_limit: raw
                .get("frame_limit")
                .and_then(|v| v.as_u64())
                .map(|v| v as u32)
                .unwrap_or(defaults.frame_limit),
        }
    }

    pub fn save(&self) {
        let mut raw = load_raw();
        raw.insert(
            "window_scale".to_owned(),
            serde_yaml::Value::Number(serde_yaml::Number::from(self.window_scale as u64)),
        );
        raw.insert(
            "window_mode".to_owned(),
            serde_yaml::Value::String(self.window_mode.as_str().to_owned()),
        );
        raw.insert(
            "vsync".to_owned(),
            serde_yaml::Value::Bool(self.vsync),
        );
        raw.insert(
            "frame_limit".to_owned(),
            serde_yaml::Value::Number(serde_yaml::Number::from(self.frame_limit as u64)),
        );
        save_raw(&raw);
    }
}

// ---------------------------------------------------------------------------
// Controls
// ---------------------------------------------------------------------------

/// All bindable actions in the emulator.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Action {
    // Joypad
    Up,
    Down,
    Left,
    Right,
    A,
    B,
    Start,
    Select,
    // Emulator controls
    Pause,
    Reset,
    FastForwardToggle,
    FastForwardHold,
    Rewind,
}

impl Action {
    pub const ALL: [Action; 13] = [
        Action::Up,
        Action::Down,
        Action::Left,
        Action::Right,
        Action::A,
        Action::B,
        Action::Start,
        Action::Select,
        Action::Pause,
        Action::Reset,
        Action::FastForwardToggle,
        Action::FastForwardHold,
        Action::Rewind,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::A => "A",
            Self::B => "B",
            Self::Start => "Start",
            Self::Select => "Select",
            Self::Pause => "Pause",
            Self::Reset => "Reset",
            Self::FastForwardToggle => "Fast Forward (Toggle)",
            Self::FastForwardHold => "Fast Forward (Hold)",
            Self::Rewind => "Rewind",
        }
    }

    pub fn config_key(self) -> &'static str {
        match self {
            Self::Up => "key_up",
            Self::Down => "key_down",
            Self::Left => "key_left",
            Self::Right => "key_right",
            Self::A => "key_a",
            Self::B => "key_b",
            Self::Start => "key_start",
            Self::Select => "key_select",
            Self::Pause => "key_pause",
            Self::Reset => "key_reset",
            Self::FastForwardToggle => "key_ff_toggle",
            Self::FastForwardHold => "key_ff_hold",
            Self::Rewind => "key_rewind",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ControlsSettings {
    pub bindings: HashMap<Action, String>,
}

impl Default for ControlsSettings {
    fn default() -> Self {
        let bindings = HashMap::from([
            (Action::Up, "up".to_owned()),
            (Action::Down, "down".to_owned()),
            (Action::Left, "left".to_owned()),
            (Action::Right, "right".to_owned()),
            (Action::A, "z".to_owned()),
            (Action::B, "x".to_owned()),
            (Action::Start, "return".to_owned()),
            (Action::Select, "backspace".to_owned()),
            (Action::Pause, "f9".to_owned()),
            (Action::Reset, "f10".to_owned()),
            (Action::FastForwardToggle, "f11".to_owned()),
            (Action::FastForwardHold, "tab".to_owned()),
            (Action::Rewind, "`".to_owned()),
        ]);
        Self { bindings }
    }
}

impl ControlsSettings {
    pub fn load() -> Self {
        let raw = load_raw();
        let defaults = Self::default();
        let mut bindings = HashMap::new();
        for action in Action::ALL {
            let key = raw
                .get(action.config_key())
                .and_then(|v| v.as_str())
                .map(|s| s.to_owned())
                .unwrap_or_else(|| {
                    defaults
                        .bindings
                        .get(&action)
                        .cloned()
                        .unwrap_or_default()
                });
            bindings.insert(action, key);
        }
        Self { bindings }
    }

    pub fn save(&self) {
        let mut raw = load_raw();
        for action in Action::ALL {
            if let Some(key) = self.bindings.get(&action) {
                raw.insert(
                    action.config_key().to_owned(),
                    serde_yaml::Value::String(key.clone()),
                );
            }
        }
        save_raw(&raw);
    }

    pub fn key_for(&self, action: Action) -> &str {
        self.bindings
            .get(&action)
            .map(|s| s.as_str())
            .unwrap_or("")
    }
}

// ---------------------------------------------------------------------------
// InputMap — reverse lookup from key name → action
// ---------------------------------------------------------------------------

/// Maps SDL key names (lowercase) to actions for efficient lookup in the
/// event loop.  Built from `ControlsSettings`.
#[derive(Clone, Debug)]
pub struct InputMap {
    map: HashMap<String, Action>,
}

impl InputMap {
    pub fn from_controls(controls: &ControlsSettings) -> Self {
        let mut map = HashMap::new();
        for (&action, key) in &controls.bindings {
            if !key.is_empty() {
                map.insert(key.to_lowercase(), action);
            }
        }
        Self { map }
    }

    pub fn lookup(&self, key_name: &str) -> Option<Action> {
        self.map.get(&key_name.to_lowercase()).copied()
    }
}

// ---------------------------------------------------------------------------
// Shader helper (used by app.rs)
// ---------------------------------------------------------------------------

pub fn shader_path() -> Option<PathBuf> {
    let raw = load_raw();
    raw.get("shader")
        .and_then(|v| v.as_str())
        .map(|s| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(s))
}
