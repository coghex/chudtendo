# Chudtendo

A Game Boy / Game Boy Color emulator built in Rust with a multi-threaded, mailbox-driven architecture. Each hardware component (CPU, PPU, APU, Timer, WRAM, Cartridge) runs in its own thread and communicates via message passing.

## Build & Run

Requires Rust and SDL2:

```bash
brew install sdl2          # macOS
cargo build                # build
cargo run -- path/to/game.gb   # run a ROM
cargo run -- --dmg path/to/game.gb  # force DMG mode
```

Place boot ROMs at `data/cgb_boot.bin` (2304 bytes) and `data/dmg_boot.bin` (256 bytes), or update paths in `config.yaml`.

## Controls

Configured in `keybindings.yaml`. Defaults:

| Button | Key |
|--------|-----|
| D-pad | Arrow keys |
| A | Z |
| B | X |
| Start | Return |
| Select | Backspace |

**Save states:** F1-F8 saves, Shift+F1-F8 loads.

## Configuration

`config.yaml` — boot ROM paths and shader selection.

`shaders/` — software color correction shaders (YAML). Default `cgb_lcd.yaml` models the CGB LCD response. Set to `shaders/raw.yaml` for unmodified output.

## Headless Agent API

The emulator can run without SDL for automated testing or AI-driven gameplay:

```rust
use chudtendo::emulator::Emulator;
use chudtendo::input::JoypadButton;

let mut emu = Emulator::from_rom_file("game.gb").unwrap();
emu.mute_audio();
emu.start().unwrap();

// Run 60 frames
let snap = emu.run_frames(60);

// Send input
emu.joypad().press(JoypadButton::Start);
emu.run_frames(5);
emu.joypad().release(JoypadButton::Start);

// Read structured PPU data
let sprites = emu.oam_entries();       // 40 OAM entries
let bg_map = emu.tile_map(false);      // 32x32 tile indices
let lcd = emu.lcd_registers();         // LCDC, STAT, SCX, SCY, LY, ...
let vram = emu.read_range(0x8000, 0x2000);  // bulk memory read

// Framebuffer is RGBA32, 160x144
let pixels = snap.framebuffer;

emu.stop();
```

## Cartridge Support

ROM-only, MBC1, MBC2, MBC3 (with RTC), MBC5. Battery-backed saves (`.sav` files) are auto-detected from the ROM path.

## Test Results

```
Blargg:        44/44  (cpu_instrs, instr_timing, mem_timing, cgb_sound, dmg_sound, halt_bug)
Acid2:          2/2   (cgb-acid2, dmg-acid2)
Mooneye:       20/62  (bits, instr, timer, interrupts, halt — timing tests require cycle-exact arch)
SameSuite:      1/35  (channel_3_wave_ram_locked_write)
```
