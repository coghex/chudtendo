//! Acid2 PPU accuracy tests.
//!
//! These tests run the dmg-acid2 and cgb-acid2 ROMs headlessly, capture the
//! framebuffer after the test stabilizes, and compare it pixel-by-pixel against
//! reference PNG images.
//!
//! ROMs and references are expected under test_roms/game-boy-test-roms-v7.0/.

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use chudtendo::emulator::{Emulator, SCREEN_HEIGHT, SCREEN_WIDTH};

const BYTES_PER_PIXEL: usize = 4; // RGBA

fn acid2_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("test_roms")
        .join("game-boy-test-roms-v7.0")
}

fn run_and_capture(rom_path: &Path, settle_secs: u64) -> Vec<u8> {
    let rom = std::fs::read(rom_path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", rom_path.display()));
    let mut emulator = Emulator::from_rom_bytes(rom).expect("failed to load ROM");
    emulator.start().expect("failed to start emulator");

    // Let the test ROM run and settle.
    let start = Instant::now();
    let mut last_frames = 0u64;
    while start.elapsed() < Duration::from_secs(settle_secs) {
        let snap = emulator.snapshot();
        last_frames = snap.ppu_frames;
        thread::sleep(Duration::from_millis(16));
    }

    // Capture a final snapshot.
    let snap = emulator.snapshot();
    eprintln!(
        "Captured at frame {} (PC={:04x})",
        snap.ppu_frames, snap.cpu_pc
    );
    let _ = last_frames;
    emulator.stop();
    snap.framebuffer
}

fn load_reference_png(path: &Path) -> Vec<u8> {
    let file = std::fs::File::open(path)
        .unwrap_or_else(|e| panic!("could not open {}: {e}", path.display()));
    let mut decoder = png::Decoder::new(file);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().expect("failed to read PNG header");
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("failed to decode PNG");
    let bytes = &buf[..info.buffer_size()];

    // After EXPAND, indexed/grayscale becomes RGB or RGBA.
    let mut rgba = Vec::with_capacity(SCREEN_WIDTH * SCREEN_HEIGHT * BYTES_PER_PIXEL);
    match info.color_type {
        png::ColorType::Rgba => {
            rgba.extend_from_slice(bytes);
        }
        png::ColorType::Rgb => {
            for chunk in bytes.chunks_exact(3) {
                rgba.extend_from_slice(chunk);
                rgba.push(0xff);
            }
        }
        other => {
            panic!("unexpected PNG color type after expansion: {other:?}");
        }
    }
    rgba
}

fn compare_framebuffers(actual: &[u8], reference: &[u8], name: &str) {
    assert_eq!(
        actual.len(),
        SCREEN_WIDTH * SCREEN_HEIGHT * BYTES_PER_PIXEL,
        "{name}: unexpected framebuffer size"
    );
    assert_eq!(
        reference.len(),
        actual.len(),
        "{name}: reference size mismatch (got {} expected {})",
        reference.len(),
        actual.len()
    );

    let mut mismatches = 0;
    let mut first_mismatch = None;
    for y in 0..SCREEN_HEIGHT {
        for x in 0..SCREEN_WIDTH {
            let i = (y * SCREEN_WIDTH + x) * BYTES_PER_PIXEL;
            // Compare RGB only (ignore alpha).
            if actual[i..i + 3] != reference[i..i + 3] {
                mismatches += 1;
                if first_mismatch.is_none() {
                    first_mismatch = Some((
                        x,
                        y,
                        [actual[i], actual[i + 1], actual[i + 2]],
                        [reference[i], reference[i + 1], reference[i + 2]],
                    ));
                }
            }
        }
    }

    if mismatches > 0 {
        // Dump actual framebuffer and a diff image for inspection.
        let dump_path = format!("/tmp/{name}_actual.ppm");
        write_ppm(&dump_path, actual);

        let mut diff = vec![0u8; SCREEN_WIDTH * SCREEN_HEIGHT * BYTES_PER_PIXEL];
        for i in 0..SCREEN_WIDTH * SCREEN_HEIGHT {
            let base = i * BYTES_PER_PIXEL;
            if actual[base..base + 3] != reference[base..base + 3] {
                diff[base] = 255;     // Red pixel for mismatch
                diff[base + 1] = 0;
                diff[base + 2] = 0;
                diff[base + 3] = 255;
            } else {
                // Dim version of actual for context
                diff[base] = actual[base] / 3;
                diff[base + 1] = actual[base + 1] / 3;
                diff[base + 2] = actual[base + 2] / 3;
                diff[base + 3] = 255;
            }
        }
        let diff_path = format!("/tmp/{name}_diff.ppm");
        write_ppm(&diff_path, &diff);

        // Print first 20 mismatches for debugging.
        let mut shown = 0;
        for y in 0..SCREEN_HEIGHT {
            for x in 0..SCREEN_WIDTH {
                let i = (y * SCREEN_WIDTH + x) * BYTES_PER_PIXEL;
                if actual[i..i + 3] != reference[i..i + 3] && shown < 20 {
                    eprintln!(
                        "  ({x:3},{y:3}): got ({:3},{:3},{:3}) exp ({:3},{:3},{:3})",
                        actual[i], actual[i+1], actual[i+2],
                        reference[i], reference[i+1], reference[i+2]
                    );
                    shown += 1;
                }
            }
        }
        panic!("{name}: {mismatches} pixel mismatches. Actual: {dump_path}  Diff: {diff_path}");
    }
}

fn write_ppm(path: &str, framebuffer: &[u8]) {
    use std::io::Write;
    let mut file = std::fs::File::create(path).expect("could not create PPM");
    write!(file, "P6\n{SCREEN_WIDTH} {SCREEN_HEIGHT}\n255\n").unwrap();
    for y in 0..SCREEN_HEIGHT {
        for x in 0..SCREEN_WIDTH {
            let i = (y * SCREEN_WIDTH + x) * BYTES_PER_PIXEL;
            file.write_all(&framebuffer[i..i + 3]).unwrap();
        }
    }
}

#[test]
fn cgb_acid2() {
    let dir = acid2_dir();
    let rom = dir.join("cgb-acid2").join("cgb-acid2.gbc");
    let reference = dir.join("cgb-acid2").join("cgb-acid2.png");
    if !rom.exists() {
        eprintln!("skipping cgb-acid2: ROM not found at {}", rom.display());
        return;
    }

    let fb = run_and_capture(&rom, 5);
    let ref_fb = load_reference_png(&reference);
    compare_framebuffers(&fb, &ref_fb, "cgb-acid2");
}

#[test]
fn dmg_acid2() {
    let dir = acid2_dir();
    let rom = dir.join("dmg-acid2").join("dmg-acid2.gb");
    // Our emulator runs DMG games in CGB compatibility mode, so use the CGB reference.
    let reference = dir.join("dmg-acid2").join("dmg-acid2-cgb.png");
    if !rom.exists() {
        eprintln!("skipping dmg-acid2: ROM not found at {}", rom.display());
        return;
    }

    let fb = run_and_capture(&rom, 5);
    let ref_fb = load_reference_png(&reference);
    compare_framebuffers(&fb, &ref_fb, "dmg-acid2");
}
