# Chudtendo Agent Instructions

You are controlling a Game Boy Color emulator running Super Mario Land. Your goal is to play through the game, reporting any visual glitches, audio issues, or emulation bugs you encounter.

## Setup

The emulator agent server is already running. You interact with it by running shell commands.

## Commands

All commands use the `agent_ctl` binary:

```bash
# View the current screen (saves a PPM image)
./target/release/agent_ctl screenshot

# Get structured game state (JSON: scroll position, sprites, LCD registers)
./target/release/agent_ctl state

# Get CPU registers
./target/release/agent_ctl status

# Press a button briefly (default 100ms, or specify duration in ms)
./target/release/agent_ctl press <button> [ms]

# Hold a button for N seconds
./target/release/agent_ctl hold <button> [seconds]

# Release a held button
./target/release/agent_ctl release <button>

# Advance N frames without input
./target/release/agent_ctl frames <n>

# Save/load state
./target/release/agent_ctl save <1-8>
./target/release/agent_ctl load <1-8>
```

Buttons: `a`, `b`, `start`, `select`, `up`, `down`, `left`, `right`

## How to Play

1. **Start the game:** Press `start` twice (once to dismiss title, once to begin).
2. **Move:** `hold right <seconds>` to walk/run right.
3. **Jump:** `press a <ms>` — longer press = higher jump (200-400ms).
4. **Jump while moving:** Run `hold right` and `press a` together — you can do this by running hold in background: `./target/release/agent_ctl hold right 1 & ./target/release/agent_ctl press a 300`

## Strategy

- **Check state often.** Run `state` between actions to see sprite positions. Mario is usually around screen x=32-40. Sprites ahead of Mario at similar Y are enemies.
- **Save frequently.** Use `save 1` before tricky sections. If you die, `load 1` to retry.
- **Use screenshots** when you need to see the full picture — platforms, gaps, pipes, etc. The state command alone can't show level geometry.
- **Enemies:** Goombas walk toward you. Jump on them or jump over them.
- **Pits:** Gaps in the ground are instant death. Jump across them.
- **Pipes:** Tall obstacles. Jump on top or over them.
- **? Blocks:** Jump into them from below to get coins/powerups.
- **End of level:** Reach the door/tower on the right side.

## Reporting Bugs

As you play, watch for:
- Visual glitches (flickering sprites, wrong colors, garbled tiles)
- Audio issues (crackling, wrong pitch, missing sounds)
- Gameplay bugs (wrong collision, stuck sprites, broken scrolling)
- Crashes or freezes

Report any issues with: what you saw, what level/area, and the frame number from `status`.

## Game Structure

Super Mario Land has 4 worlds with 3 levels each (1-1 through 4-3). Worlds 2 and 4 have shoot-em-up levels. The game is relatively short but challenging.
