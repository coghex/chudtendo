//! Agent server: runs the emulator and accepts commands over TCP.
//!
//! Usage:
//!   cargo run --bin agent_server -- <rom>             # headless
//!   cargo run --bin agent_server -- --visible <rom>   # with SDL window

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use chudtendo::emulator::{Emulator, SCREEN_HEIGHT, SCREEN_WIDTH};
use chudtendo::input::JoypadButton;

const PORT: u16 = 31337;
const BYTES_PER_PIXEL: usize = 4;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let visible = args.iter().any(|a| a == "--visible");
    // --turbo (uncapped) or --turbo=2.0 (2x speed)
    let speed: f32 = args.iter()
        .find(|a| a.starts_with("--turbo"))
        .map(|a| {
            if let Some(val) = a.strip_prefix("--turbo=") {
                val.parse::<f32>().unwrap_or(0.0)
            } else {
                0.0 // bare --turbo = uncapped
            }
        })
        .unwrap_or(1.0); // no flag = normal speed
    let port = args.iter()
        .position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(PORT);
    let rom_path = args.iter()
        .find(|a| !a.starts_with("--") && a.parse::<u16>().is_err())
        .cloned()
        .unwrap_or_else(|| {
            eprintln!("usage: cargo run --bin agent_server -- [--visible] [--turbo[=N.N]] [--port N] <rom>");
            std::process::exit(1);
        });

    if visible {
        run_visible(&rom_path, speed, port);
    } else {
        run_headless(&rom_path, speed, port);
    }
}

fn run_headless(rom_path: &str, speed: f32, port: u16) {
    let mut emulator = load_emulator(rom_path);
    emulator.mute_audio();
    if speed != 1.0 { emulator.set_speed(speed); }
    emulator.start().unwrap_or_else(|e| { eprintln!("failed to start: {e}"); std::process::exit(1); });
    eprintln!("[agent-server] emulator started (headless{}) on port {port}, ROM: {rom_path}", if speed != 1.0 { &format!(", {speed}x") } else { "" });

    let emu = Arc::new(Mutex::new(emulator));
    run_tcp_server(emu, port);
}

fn run_visible(rom_path: &str, speed: f32, port: u16) {
    let emu: Arc<Mutex<Option<Emulator>>> = Arc::new(Mutex::new(None));
    let emu_for_tcp = emu.clone();

    // Spawn TCP listener in background — it waits until the emulator is ready.
    thread::spawn(move || {
        // Wait for the emulator to be set by the SDL thread.
        loop {
            {
                let guard = emu_for_tcp.lock().unwrap();
                if guard.is_some() { break; }
            }
            thread::sleep(Duration::from_millis(50));
        }
        run_tcp_server_shared(emu_for_tcp, port);
    });

    // Run SDL on the main thread (macOS requirement). The agent callback
    // stores the emulator reference for the TCP thread.
    // We can't access the Emulator from app::run, but we CAN use
    // run_with_agent to get the joypad. For the TCP server we need the
    // full Emulator though.
    //
    // Simplest approach: run the emulator + SDL via run_with_agent,
    // and the TCP thread controls it via the joypad only (no state reads
    // in visible mode — use screenshots from the window instead).
    //
    // Actually, let's do it properly: create the emulator here, share it,
    // and build a minimal SDL render loop ourselves.
    let mut emulator = load_emulator(rom_path);
    if speed != 1.0 { emulator.set_speed(speed); }
    emulator.start().unwrap_or_else(|e| { eprintln!("failed to start: {e}"); std::process::exit(1); });
    eprintln!("[agent-server] emulator started (visible{}) on port {port}, ROM: {rom_path}", if speed != 1.0 { &format!(", {speed}x") } else { "" });

    // Store emulator for TCP thread.
    {
        let mut guard = emu.lock().unwrap();
        *guard = Some(emulator);
    }

    // Run a minimal SDL render loop on the main thread.
    sdl_render_loop(emu, port);
}

fn sdl_render_loop(emu: Arc<Mutex<Option<Emulator>>>, _port: u16) {
    sdl2::hint::set("SDL_RENDER_SCALE_QUALITY", "nearest");
    let sdl = sdl2::init().expect("failed to init SDL");
    let video = sdl.video().expect("failed to init SDL video");

    let window = video
        .window("Chudtendo Agent", SCREEN_WIDTH as u32 * 4, SCREEN_HEIGHT as u32 * 4)
        .position_centered()
        .build()
        .expect("failed to create window");

    let mut canvas = window.into_canvas().build().expect("failed to create canvas");
    canvas.set_draw_color(sdl2::pixels::Color::RGB(0, 0, 0));
    canvas.clear();
    canvas.present();

    let texture_creator = canvas.texture_creator();
    let mut texture = texture_creator
        .create_texture_streaming(sdl2::pixels::PixelFormatEnum::ABGR8888, SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32)
        .expect("failed to create texture");

    let mut event_pump = sdl.event_pump().expect("failed to create event pump");

    'running: loop {
        for event in event_pump.poll_iter() {
            match event {
                sdl2::event::Event::Quit { .. } => break 'running,
                sdl2::event::Event::KeyDown { keycode: Some(sdl2::keyboard::Keycode::Escape), .. } => break 'running,
                _ => {}
            }
        }

        // Try to grab the latest framebuffer. If the mutex is held by the
        // TCP handler (processing a training command), skip this frame
        // instead of blocking. This prevents the SDL window from freezing.
        if let Ok(mut guard) = emu.try_lock() {
            if let Some(ref mut emulator) = *guard {
                let snap = emulator.snapshot();
                let fb = &snap.framebuffer;
                let _ = texture.with_lock(None, |pixels, pitch| {
                    let row_width = SCREEN_WIDTH * 4;
                    for (row_index, row) in fb.chunks_exact(row_width).enumerate() {
                        let offset = row_index * pitch;
                        pixels[offset..offset + row_width].copy_from_slice(row);
                    }
                });
            }
        }

        canvas.copy(&texture, None, None).ok();
        canvas.present();

        thread::sleep(Duration::from_millis(16));
    }

    // Stop emulator on window close.
    let mut guard = emu.lock().unwrap();
    if let Some(ref mut emulator) = *guard {
        emulator.stop();
    }
}

fn load_emulator(rom_path: &str) -> Emulator {
    Emulator::from_rom_file(Path::new(rom_path))
        .unwrap_or_else(|e| { eprintln!("failed to load ROM: {e}"); std::process::exit(1); })
}

fn run_tcp_server(emu: Arc<Mutex<Emulator>>, port: u16) {
    let listener = TcpListener::bind(format!("127.0.0.1:{port}"))
        .unwrap_or_else(|e| { eprintln!("failed to bind port {port}: {e}"); std::process::exit(1); });
    eprintln!("[agent-server] listening on 127.0.0.1:{port}");

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let emu = emu.clone();
        thread::spawn(move || handle_client(stream, &emu));
    }
}

fn run_tcp_server_shared(emu: Arc<Mutex<Option<Emulator>>>, port: u16) {
    let listener = TcpListener::bind(format!("127.0.0.1:{port}"))
        .unwrap_or_else(|e| { eprintln!("failed to bind port {port}: {e}"); std::process::exit(1); });
    eprintln!("[agent-server] listening on 127.0.0.1:{port}");

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let emu = emu.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let line = line.trim().to_string();
                if line.is_empty() { continue; }
                let response = handle_shared_command(&emu, &line);
                let _ = writer.write_all(response.as_bytes());
                let _ = writer.write_all(b"\n");
                let _ = writer.flush();
            }
        });
    }
}

/// Handle a command with the shared emulator, releasing the lock during sleeps
/// so the SDL render loop can continue updating.
fn handle_shared_command(emu: &Arc<Mutex<Option<Emulator>>>, cmd: &str) -> String {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() {
        return r#"{"error":"empty command"}"#.to_string();
    }

    // For press/hold: grab joypad (brief lock), then sleep WITHOUT the lock.
    match parts[0] {
        "press" => {
            let Some(btn) = parts.get(1).and_then(|s| parse_button(s)) else {
                return r#"{"error":"usage: press <button> [ms]"}"#.to_string();
            };
            let ms = parts.get(2).and_then(|s| s.parse::<u64>().ok()).unwrap_or(100);
            let joypad = {
                let guard = emu.lock().unwrap();
                guard.as_ref().map(|e| e.joypad().clone())
            };
            if let Some(joypad) = joypad {
                joypad.press(btn);
                thread::sleep(Duration::from_millis(ms));
                joypad.release(btn);
                r#"{"ok":true}"#.to_string()
            } else {
                r#"{"error":"emulator not ready"}"#.to_string()
            }
        }
        "hold" => {
            let Some(btn) = parts.get(1).and_then(|s| parse_button(s)) else {
                return r#"{"error":"usage: hold <button> [seconds]"}"#.to_string();
            };
            let secs = parts.get(2).and_then(|s| s.parse::<f64>().ok()).unwrap_or(1.0);
            let joypad = {
                let guard = emu.lock().unwrap();
                guard.as_ref().map(|e| e.joypad().clone())
            };
            if let Some(joypad) = joypad {
                joypad.press(btn);
                thread::sleep(Duration::from_secs_f64(secs));
                joypad.release(btn);
                r#"{"ok":true}"#.to_string()
            } else {
                r#"{"error":"emulator not ready"}"#.to_string()
            }
        }
        "down" => {
            let Some(btn) = parts.get(1).and_then(|s| parse_button(s)) else {
                return r#"{"error":"usage: down <button>"}"#.to_string();
            };
            let guard = emu.lock().unwrap();
            if let Some(ref emulator) = *guard {
                emulator.joypad().press(btn);
                r#"{"ok":true}"#.to_string()
            } else {
                r#"{"error":"emulator not ready"}"#.to_string()
            }
        }
        "release" | "up" => {
            let Some(btn) = parts.get(1).and_then(|s| parse_button(s)) else {
                return r#"{"error":"usage: release <button>"}"#.to_string();
            };
            let guard = emu.lock().unwrap();
            if let Some(ref emulator) = *guard {
                emulator.joypad().release(btn);
                r#"{"ok":true}"#.to_string()
            } else {
                r#"{"error":"emulator not ready"}"#.to_string()
            }
        }
        "frames" => {
            // Advance N frames WITHOUT holding the lock the whole time.
            let n = parts.get(1).and_then(|s| s.parse::<u64>().ok()).unwrap_or(4);
            let target = {
                let mut guard = emu.lock().unwrap();
                if let Some(ref mut emulator) = *guard {
                    let snap = emulator.snapshot();
                    snap.ppu_frames + n
                } else {
                    return r#"{"error":"emulator not ready"}"#.to_string();
                }
            };
            // Poll with lock released between checks.
            loop {
                {
                    let mut guard = emu.lock().unwrap();
                    if let Some(ref mut emulator) = *guard {
                        let snap = emulator.status_snapshot();
                        if snap.ppu_frames >= target {
                            return format!(r#"{{"ok":true,"frame":{}}}"#, snap.ppu_frames);
                        }
                    }
                }
                thread::sleep(Duration::from_micros(500));
            }
        }
        _ => {
            // Brief lock for all other commands. Keep the lock duration
            // short so the SDL render loop isn't starved.
            let mut guard = emu.lock().unwrap();
            if let Some(ref mut emulator) = *guard {
                handle_command_direct(emulator, cmd)
            } else {
                r#"{"error":"emulator not ready"}"#.to_string()
            }
        }
    }
}

fn handle_client(stream: std::net::TcpStream, emu: &Arc<Mutex<Emulator>>) {
    let reader = BufReader::new(stream.try_clone().unwrap());
    let mut writer = stream;
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let line = line.trim().to_string();
        if line.is_empty() { continue; }
        let response = {
            let mut emu = emu.lock().unwrap();
            handle_command_direct(&mut emu, &line)
        };
        let _ = writer.write_all(response.as_bytes());
        let _ = writer.write_all(b"\n");
        let _ = writer.flush();
    }
}

fn handle_command_direct(emu: &mut Emulator, cmd: &str) -> String {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() {
        return r#"{"error":"empty command"}"#.to_string();
    }

    match parts[0] {
        "screenshot" => {
            let snap = emu.snapshot();
            let path = "/tmp/chudtendo_agent.ppm";
            write_ppm(path, &snap.framebuffer);
            format!(r#"{{"ok":true,"path":"{path}","frame":{}}}"#, snap.ppu_frames)
        }
        "framebytes" => {
            // Return raw RGB framebuffer as base64 for fast Python ingestion.
            // No file I/O — much faster than screenshot for RL training.
            let snap = emu.snapshot();
            let fb = &snap.framebuffer;
            // Convert RGBA to RGB (drop alpha).
            let mut rgb = Vec::with_capacity(SCREEN_WIDTH * SCREEN_HEIGHT * 3);
            for pixel in fb.chunks_exact(BYTES_PER_PIXEL) {
                rgb.extend_from_slice(&pixel[..3]);
            }
            use std::io::Write as _;
            let mut b64 = String::new();
            base64_encode(&rgb, &mut b64);
            format!(r#"{{"ok":true,"frame":{},"width":{SCREEN_WIDTH},"height":{SCREEN_HEIGHT},"rgb_base64":"{b64}"}}"#, snap.ppu_frames)
        }
        "gameinfo" | "features" => {
            let snap = emu.snapshot();

            // Get PPU data in ONE command (no bus reads).
            let ppu = emu.ppu_features();
            let (scx, scy, sprites_str) = if let Some(ppu) = &ppu {
                let sprites: Vec<String> = ppu.oam.chunks_exact(4)
                    .filter(|e| e[0] > 0 && e[0] < 160 && e[1] > 0)
                    .map(|e| format!("[{},{},{},{}]",
                        e[1] as i16 - 8, e[0] as i16 - 16, e[2], e[3]))
                    .collect();
                (ppu.scx, ppu.scy, sprites.join(","))
            } else {
                (0, 0, String::new())
            };

            // Game RAM: only 6 bus reads (to WRAM/CPU, not PPU).
            let game_ram = emu.read_range(0xC0A0, 3);
            let lives = emu.read_range(0xDA15, 1).first().copied().unwrap_or(0);
            let coins = emu.read_range(0xFFB4, 1).first().copied().unwrap_or(0);
            let powerup = emu.read_range(0xC200, 1).first().copied().unwrap_or(0);

            let mut score: u32 = 0;
            for &b in &game_ram {
                score = score * 100 + (b as u32 >> 4) * 10 + (b as u32 & 0x0F);
            }
            score *= 10;

            format!(
                r#"{{"frame":{},"scx":{},"scy":{},"lives":{},"score":{},"coins":{},"powerup":{},"sprites":[{}],"pc":"0x{:04x}"}}"#,
                snap.ppu_frames, scx, scy, lives, score, coins, powerup,
                sprites_str, snap.cpu_pc,
            )
        }
        "state" => {
            let snap = emu.snapshot();
            let lcd = emu.lcd_registers();
            let oam = emu.oam_entries();
            let sprites: Vec<String> = oam.iter()
                .filter(|s| s.x > 0 && s.y > 0 && s.y < 160)
                .map(|s| format!(
                    r#"{{"x":{},"y":{},"tile":{},"flags":{}}}"#,
                    s.screen_x(), s.screen_y(), s.tile, s.flags
                ))
                .collect();
            format!(
                r#"{{"frame":{},"pc":"0x{:04x}","lcdc":{},"scx":{},"scy":{},"ly":{},"wx":{},"wy":{},"sprites":[{}]}}"#,
                snap.ppu_frames, snap.cpu_pc,
                lcd.lcdc, lcd.scx, lcd.scy, lcd.ly, lcd.wx, lcd.wy,
                sprites.join(","),
            )
        }
        "status" => {
            let snap = emu.snapshot();
            format!(
                r#"{{"frame":{},"pc":"0x{:04x}","af":"0x{:04x}","bc":"0x{:04x}","de":"0x{:04x}","hl":"0x{:04x}","sp":"0x{:04x}"}}"#,
                snap.ppu_frames, snap.cpu_pc,
                snap.cpu_af, snap.cpu_bc, snap.cpu_de, snap.cpu_hl, snap.cpu_sp,
            )
        }
        "read" => {
            // Read 1 or more bytes: read <hex_addr> [count]
            let Some(addr_str) = parts.get(1) else {
                return r#"{"error":"usage: read <hex_addr> [count]"}"#.to_string();
            };
            let addr = u16::from_str_radix(addr_str.trim_start_matches("0x"), 16).unwrap_or(0);
            let count = parts.get(2).and_then(|s| s.parse::<u16>().ok()).unwrap_or(1);
            let data = emu.read_range(addr, count);
            let vals: Vec<String> = data.iter().map(|b| format!("{b}")).collect();
            format!(r#"{{"ok":true,"addr":"0x{addr:04x}","values":[{}]}}"#, vals.join(","))
        }
        "press" => {
            let Some(btn) = parts.get(1).and_then(|s| parse_button(s)) else {
                return r#"{"error":"usage: press <button> [ms]"}"#.to_string();
            };
            let ms = parts.get(2).and_then(|s| s.parse::<u64>().ok()).unwrap_or(100);
            let joypad = emu.joypad().clone();
            joypad.press(btn);
            thread::sleep(Duration::from_millis(ms));
            joypad.release(btn);
            r#"{"ok":true}"#.to_string()
        }
        "hold" => {
            let Some(btn) = parts.get(1).and_then(|s| parse_button(s)) else {
                return r#"{"error":"usage: hold <button> [seconds]"}"#.to_string();
            };
            let secs = parts.get(2).and_then(|s| s.parse::<f64>().ok()).unwrap_or(1.0);
            let joypad = emu.joypad().clone();
            joypad.press(btn);
            thread::sleep(Duration::from_secs_f64(secs));
            joypad.release(btn);
            r#"{"ok":true}"#.to_string()
        }
        "release" => {
            let Some(btn) = parts.get(1).and_then(|s| parse_button(s)) else {
                return r#"{"error":"usage: release <button>"}"#.to_string();
            };
            emu.joypad().release(btn);
            r#"{"ok":true}"#.to_string()
        }
        "frames" => {
            let n = parts.get(1).and_then(|s| s.parse::<u64>().ok()).unwrap_or(60);
            let snap = emu.run_frames(n);
            format!(r#"{{"ok":true,"frame":{}}}"#, snap.ppu_frames)
        }
        "save" => {
            let slot = parts.get(1).and_then(|s| s.parse::<u8>().ok()).unwrap_or(1);
            match emu.save_state(slot) {
                Ok(()) => format!(r#"{{"ok":true,"slot":{slot}}}"#),
                Err(e) => format!(r#"{{"error":"{e}"}}"#),
            }
        }
        "load" => {
            let slot = parts.get(1).and_then(|s| s.parse::<u8>().ok()).unwrap_or(1);
            match emu.load_state(slot) {
                Ok(()) => format!(r#"{{"ok":true,"slot":{slot}}}"#),
                Err(e) => format!(r#"{{"error":"{e}"}}"#),
            }
        }
        "help" => {
            r#"{"commands":["screenshot","state","status","press <button> [ms]","hold <button> [secs]","down <button>","release/up <button>","frames <n>","save <slot>","load <slot>","help"]}"#.to_string()
        }
        _ => format!(r#"{{"error":"unknown command: {}"}}"#, parts[0]),
    }
}

fn parse_button(s: &str) -> Option<JoypadButton> {
    match s.to_lowercase().as_str() {
        "a" => Some(JoypadButton::A),
        "b" => Some(JoypadButton::B),
        "start" => Some(JoypadButton::Start),
        "select" => Some(JoypadButton::Select),
        "up" => Some(JoypadButton::Up),
        "down" => Some(JoypadButton::Down),
        "left" => Some(JoypadButton::Left),
        "right" => Some(JoypadButton::Right),
        _ => None,
    }
}

fn write_ppm(path: &str, framebuffer: &[u8]) {
    let mut file = std::fs::File::create(path).expect("could not create PPM");
    use std::io::Write;
    write!(file, "P6\n{SCREEN_WIDTH} {SCREEN_HEIGHT}\n255\n").unwrap();
    for y in 0..SCREEN_HEIGHT {
        for x in 0..SCREEN_WIDTH {
            let i = (y * SCREEN_WIDTH + x) * BYTES_PER_PIXEL;
            file.write_all(&framebuffer[i..i + 3]).unwrap();
        }
    }
}

fn base64_encode(data: &[u8], out: &mut String) {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut i = 0;
    while i + 2 < data.len() {
        let n = (data[i] as u32) << 16 | (data[i + 1] as u32) << 8 | data[i + 2] as u32;
        out.push(CHARS[(n >> 18 & 0x3F) as usize] as char);
        out.push(CHARS[(n >> 12 & 0x3F) as usize] as char);
        out.push(CHARS[(n >> 6 & 0x3F) as usize] as char);
        out.push(CHARS[(n & 0x3F) as usize] as char);
        i += 3;
    }
    let rem = data.len() - i;
    if rem == 1 {
        let n = (data[i] as u32) << 16;
        out.push(CHARS[(n >> 18 & 0x3F) as usize] as char);
        out.push(CHARS[(n >> 12 & 0x3F) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = (data[i] as u32) << 16 | (data[i + 1] as u32) << 8;
        out.push(CHARS[(n >> 18 & 0x3F) as usize] as char);
        out.push(CHARS[(n >> 12 & 0x3F) as usize] as char);
        out.push(CHARS[(n >> 6 & 0x3F) as usize] as char);
        out.push('=');
    }
}
