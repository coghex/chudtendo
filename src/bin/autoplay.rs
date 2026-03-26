//! Autoplay: launches the emulator with SDL (visible window + audio) while a
//! background thread sends scripted joypad inputs.

use std::path::Path;
use std::thread;
use std::time::Duration;

use chudtendo::input::{JoypadButton, JoypadState};

fn main() {
    let rom_path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: cargo run --bin autoplay -- <rom>");
        std::process::exit(1);
    });

    if let Err(e) = chudtendo::run_with_agent(
        chudtendo::RunMode::Interactive,
        Some(Path::new(&rom_path)),
        false,
        play_sml,
    ) {
        eprintln!("Error: {e}");
    }
}

/// Scripted input sequence for Super Mario Land World 1-1.
fn play_sml(joypad: JoypadState) {
    let tap = |btn: JoypadButton, ms: u64| {
        joypad.press(btn);
        thread::sleep(Duration::from_millis(ms));
        joypad.release(btn);
        thread::sleep(Duration::from_millis(50));
    };

    // Wait for boot sequence
    thread::sleep(Duration::from_secs(8));

    // Press Start at title screen
    eprintln!("[agent] pressing Start");
    tap(JoypadButton::Start, 200);
    thread::sleep(Duration::from_secs(2));

    // Game starts — walk right and play!
    eprintln!("[agent] starting gameplay");

    for i in 0..20 {
        eprintln!("[agent] action {}/20", i + 1);

        // Hold right and run
        joypad.press(JoypadButton::Right);
        thread::sleep(Duration::from_millis(1500));

        // Jump while moving right
        joypad.press(JoypadButton::A);
        thread::sleep(Duration::from_millis(300));
        joypad.release(JoypadButton::A);
        thread::sleep(Duration::from_millis(500));

        // Another jump
        joypad.press(JoypadButton::A);
        thread::sleep(Duration::from_millis(200));
        joypad.release(JoypadButton::A);
        thread::sleep(Duration::from_millis(800));

        joypad.release(JoypadButton::Right);
        thread::sleep(Duration::from_millis(100));
    }

    eprintln!("[agent] done");
}
