use chudtendo::emulator::{Emulator, SCREEN_WIDTH, SCREEN_HEIGHT};
use std::io::Write;
use std::time::{Duration, Instant};
use std::thread;

#[test]
fn dump_boot_frames() {
    let rom = std::fs::read("rom/sml.gb").expect("could not read rom/sml.gb");
    let mut emulator = Emulator::from_rom_bytes(rom).expect("failed to load ROM");
    emulator.start().expect("failed to start emulator");

    let start = Instant::now();
    let mut frame_count = 0;
    let mut last_frames = 0;

    // Run for 10 seconds (full boot + title screen)
    while start.elapsed() < Duration::from_secs(10) {
        let snap = emulator.snapshot();
        if snap.ppu_frames > last_frames {
            frame_count += 1;
            last_frames = snap.ppu_frames;

            // Dump frames including late boot and early gameplay
            if matches!(snap.ppu_frames, 1 | 60 | 150 | 200 | 250 | 300 | 350 | 400 | 500 | 600) {
                let path = format!("/tmp/chudtendo_frame_{:03}.ppm", snap.ppu_frames);
                write_ppm(&path, &snap.framebuffer);
                eprintln!(
                    "Frame {}: PC={:04x} LY={} steps={} -- saved to {}",
                    snap.ppu_frames, snap.cpu_pc, 0, snap.cpu_steps, path
                );
            }
        }
        thread::sleep(Duration::from_millis(5));
    }

    eprintln!("Total frames observed: {frame_count} (ppu_frames={})", last_frames);
    emulator.stop();
}

fn write_ppm(path: &str, framebuffer: &[u8]) {
    let mut file = std::fs::File::create(path).expect("could not create PPM file");
    write!(file, "P6\n{SCREEN_WIDTH} {SCREEN_HEIGHT}\n255\n").unwrap();
    // framebuffer is RGBA, PPM needs RGB
    for y in 0..SCREEN_HEIGHT {
        for x in 0..SCREEN_WIDTH {
            let i = (y * SCREEN_WIDTH + x) * 4;
            file.write_all(&framebuffer[i..i+3]).unwrap(); // R, G, B (skip A)
        }
    }
}
