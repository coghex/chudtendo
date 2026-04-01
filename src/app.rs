use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sdl2::audio::{AudioCallback, AudioSpecDesired};
use sdl2::event::Event;
use sdl2::keyboard::{Keycode, Mod};
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::render::Texture;
use sdl2::video::Window;

use crate::emulator::{CartridgeMetadata, Emulator, SCREEN_HEIGHT, SCREEN_WIDTH, Snapshot};
use crate::menu::{self, MenuAction};
use crate::shader::Shader;

const DEFAULT_WINDOW_SCALE: u32 = 4;
const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
const DEFAULT_PPU_DELAY: Duration = Duration::from_nanos(16_536_000);
const HALF_REFRESH_MATCH_TOLERANCE: Duration = Duration::from_micros(2_000);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunMode {
    Interactive,
    SmokeTest,
}

/// Run the emulator with an optional agent callback. The callback receives a
/// clone of the joypad and runs in a background thread while the SDL window
/// is active. The emulator exits when the agent thread finishes or the window
/// is closed, whichever comes first.
pub fn run_with_agent<F>(run_mode: RunMode, rom_path: Option<&Path>, dmg_mode: bool, agent: F) -> Result<(), String>
where
    F: FnOnce(crate::input::JoypadState) + Send + 'static,
{
    run_inner(run_mode, rom_path, dmg_mode, 1.0, Some(Box::new(agent)))
}

pub fn run(run_mode: RunMode, rom_path: Option<&Path>, dmg_mode: bool, speed: f32) -> Result<(), String> {
    run_inner(run_mode, rom_path, dmg_mode, speed, None)
}

fn run_inner(
    run_mode: RunMode,
    rom_path: Option<&Path>,
    dmg_mode: bool,
    speed: f32,
    agent: Option<Box<dyn FnOnce(crate::input::JoypadState) + Send>>,
) -> Result<(), String> {
    sdl2::hint::set("SDL_RENDER_SCALE_QUALITY", "nearest");

    let sdl = sdl2::init().map_err(|error| format!("failed to initialize SDL: {error}"))?;
    let video = sdl
        .video()
        .map_err(|error| format!("failed to initialize SDL video: {error}"))?;

    let mut current_scale = load_window_scale();

    let mut window_builder = video.window(
        "Chudtendo",
        SCREEN_WIDTH as u32 * current_scale,
        SCREEN_HEIGHT as u32 * current_scale,
    );
    window_builder.position_centered();

    if matches!(run_mode, RunMode::SmokeTest) {
        window_builder.hidden();
    }

    let window = window_builder
        .build()
        .map_err(|error| format!("failed to create SDL window: {error}"))?;

    let uses_vsync = matches!(run_mode, RunMode::Interactive);
    let mut canvas_builder = window.into_canvas();
    if uses_vsync {
        canvas_builder = canvas_builder.present_vsync();
    }
    let mut canvas = canvas_builder
        .build()
        .map_err(|error| format!("failed to create SDL canvas: {error}"))?;
    let interactive_ppu_delay = if uses_vsync {
        detect_interactive_ppu_delay(&video, canvas.window())?
    } else {
        None
    };

    canvas
        .set_scale(current_scale as f32, current_scale as f32)
        .map_err(|error| format!("failed to configure render scale: {error}"))?;

    let texture_creator = canvas.texture_creator();
    let mut texture = texture_creator
        .create_texture_streaming(
            PixelFormatEnum::RGBA32,
            SCREEN_WIDTH as u32,
            SCREEN_HEIGHT as u32,
        )
        .map_err(|error| format!("failed to create streaming texture: {error}"))?;

    let mut emulator = match rom_path {
        Some(path) => {
            let mut emu = Emulator::from_rom_file(path).map_err(|error| error.to_string())?;
            if dmg_mode {
                emu.set_dmg_mode();
            }
            if speed != 1.0 {
                emu.set_speed(speed);
            }
            emu
        }
        None => Emulator::new(),
    };
    if let Some(ppu_delay) = interactive_ppu_delay {
        emulator.set_ppu_delay(ppu_delay)?;
    }
    let mut cartridge = emulator.cartridge_metadata().clone();

    if matches!(run_mode, RunMode::SmokeTest) {
        emulator.start()?;
        run_smoke_frames(&mut emulator, &mut texture, &mut canvas)?;
        emulator.stop();
        return Ok(());
    }

    // Install native menu bar (macOS: File, Emulation menus).
    // Share the save path so the menu callback can query slot timestamps
    // from the filesystem when the submenu opens.
    let save_path = std::sync::Arc::new(std::sync::Mutex::new(
        emulator.config().save_path.clone(),
    ));
    let menu_save_path = save_path.clone();
    let slot_info_fn: menu::SlotInfoFn = Box::new(move || {
        let path = menu_save_path.lock().unwrap().clone();
        (1..=8u8)
            .map(|slot| {
                let ts = path.as_ref().and_then(|p| {
                    let state_path = p.with_extension(format!("state{slot}"));
                    let meta = std::fs::metadata(&state_path).ok()?;
                    let modified = meta.modified().ok()?;
                    let elapsed = modified.elapsed().ok()?;
                    Some(format_elapsed(elapsed))
                });
                menu::SlotInfo { slot, timestamp: ts }
            })
            .collect()
    });
    let shared_scale: menu::SharedScale = std::sync::Arc::new(
        std::sync::atomic::AtomicU32::new(current_scale),
    );
    let menu_receiver = menu::install_menu_bar(slot_info_fn, shared_scale.clone());
    let settings_receiver = crate::preferences::install_settings_channel();

    // SDL video should stay on the main thread on macOS, so the emulator modules run in workers.
    emulator.start()?;

    let mut _audio_device = if matches!(run_mode, RunMode::Interactive) {
        init_audio(&sdl, &mut emulator)?
    } else {
        None
    };

    // Spawn agent thread if provided.
    let _agent_thread = agent.map(|f| {
        let joypad = emulator.joypad().clone();
        std::thread::Builder::new()
            .name("agent".to_owned())
            .spawn(move || f(joypad))
            .expect("failed to spawn agent thread")
    });

    let shader = load_shader();
    let controls = crate::settings::ControlsSettings::load();
    let mut input_map = crate::settings::InputMap::from_controls(&controls);
    let mut joypad = emulator.joypad().clone();
    let initial_snapshot = wait_for_live_snapshot(&mut emulator, Duration::from_millis(100));
    upload_framebuffer(&mut texture, &initial_snapshot.framebuffer, &shader)?;
    draw_frame(&mut canvas, &texture)?;

    let mut event_pump = sdl
        .event_pump()
        .map_err(|error| format!("failed to create SDL event pump: {error}"))?;
    let mut fast_forward_toggled = false;
    let mut fast_forward_held = false;
    let mut ff_speed: f32 = crate::settings::EmulationSettings::load().ff_speed;

    let mut rewinding = false;
    let mut last_rewind_step = Instant::now();
    let mut last_rewind_capture_frame: u64 = 0;
    const REWIND_CAPTURE_INTERVAL: u64 = 4; // capture every 4 frames
    const REWIND_STEP_INTERVAL: Duration = Duration::from_millis(50); // ~20 steps/sec during rewind

    let mut last_title_update = Instant::now();
    let mut last_uploaded_frame = initial_snapshot.ppu_frames;
    let mut last_presented_frame = initial_snapshot.ppu_frames;
    let mut last_snapshot_frame = initial_snapshot.ppu_frames;
    let mut last_snapshot_time = Instant::now();
    let mut last_present_time = Instant::now();

    'running: loop {
        // Poll menu actions.
        while let Ok(action) = menu_receiver.try_recv() {
            match action {
                MenuAction::Quit => break 'running,
                MenuAction::Pause => {
                    let now_paused = emulator.toggle_pause();
                    eprintln!("{}", if now_paused { "Paused" } else { "Resumed" });
                }
                MenuAction::Reset => {
                    match emulator.reset() {
                        Ok(()) => {
                            _audio_device = init_audio(&sdl, &mut emulator)?;
                            joypad = emulator.joypad().clone();
                            emulator.rewind_clear();
                            last_rewind_capture_frame = 0;
                            eprintln!("Reset");
                        }
                        Err(e) => eprintln!("Reset failed: {e}"),
                    }
                }
                MenuAction::LoadRom => {
                    if let Some(path) = pick_rom_file() {
                        match load_new_rom(
                            &mut emulator,
                            &path,
                            dmg_mode,
                            speed,
                            interactive_ppu_delay,
                        ) {
                            Ok(()) => {
                                _audio_device = init_audio(&sdl, &mut emulator)?;
                                joypad = emulator.joypad().clone();
                                cartridge = emulator.cartridge_metadata().clone();
                                *save_path.lock().unwrap() = emulator.config().save_path.clone();
                                emulator.rewind_clear();
                                last_rewind_capture_frame = 0;
                                last_uploaded_frame = 0;
                                last_presented_frame = 0;
                                last_snapshot_frame = 0;
                                eprintln!("Loaded: {}", cartridge.title);
                            }
                            Err(e) => eprintln!("Load ROM failed: {e}"),
                        }
                    }
                }
                MenuAction::SaveState(slot) => {
                    match emulator.save_state(slot) {
                        Ok(()) => eprintln!("Saved state {slot}"),
                        Err(e) => eprintln!("Save failed: {e}"),
                    }
                }
                MenuAction::LoadState(slot) => {
                    match emulator.load_state(slot) {
                        Ok(()) => eprintln!("Loaded state {slot}"),
                        Err(e) => eprintln!("Load failed: {e}"),
                    }
                }
                MenuAction::ToggleFastForward => {
                    fast_forward_toggled = !fast_forward_toggled;
                    apply_ff_speed(&emulator, fast_forward_toggled || fast_forward_held, speed, ff_speed);
                    eprintln!("Fast forward {}", if fast_forward_toggled { "ON" } else { "OFF" });
                }
                MenuAction::ToggleRewind => {
                    rewinding = !rewinding;
                    if rewinding {
                        emulator.pause();
                        last_rewind_step = Instant::now();
                        eprintln!("Rewinding ({} snapshots)", emulator.rewind_len());
                    } else {
                        emulator.resume();
                        eprintln!("Rewind stopped");
                    }
                }
                MenuAction::OpenSettings => {
                    crate::preferences::open_preferences_window();
                }
                MenuAction::SetScale(new_scale) => {
                    if new_scale >= 1 && new_scale <= 8 {
                        current_scale = new_scale;
                        shared_scale.store(current_scale, std::sync::atomic::Ordering::Relaxed);
                        canvas
                            .window_mut()
                            .set_size(
                                SCREEN_WIDTH as u32 * current_scale,
                                SCREEN_HEIGHT as u32 * current_scale,
                            )
                            .map_err(|e| format!("failed to resize window: {e}"))?;
                        canvas
                            .set_scale(current_scale as f32, current_scale as f32)
                            .map_err(|e| format!("failed to set render scale: {e}"))?;
                        save_window_scale(current_scale);
                        eprintln!("Scale: {current_scale}x");
                    }
                }
            }
        }

        // Poll for settings changes from preferences window.
        while let Ok(changed) = settings_receiver.try_recv() {
            use crate::preferences::SettingsChanged;
            match changed {
                SettingsChanged::Emulation => {
                    let emu_settings = crate::settings::EmulationSettings::load();
                    ff_speed = emu_settings.ff_speed;
                    // Apply current FF state with new speed.
                    apply_ff_speed(&emulator, fast_forward_toggled || fast_forward_held, speed, ff_speed);
                    eprintln!("Emulation settings applied (ff_speed={ff_speed})");
                }
                SettingsChanged::Display => {
                    let display_settings = crate::settings::DisplaySettings::load();
                    if display_settings.window_scale != current_scale
                        && display_settings.window_scale >= 1
                        && display_settings.window_scale <= 8
                    {
                        current_scale = display_settings.window_scale;
                        shared_scale.store(current_scale, std::sync::atomic::Ordering::Relaxed);
                        let _ = canvas.window_mut().set_size(
                            SCREEN_WIDTH as u32 * current_scale,
                            SCREEN_HEIGHT as u32 * current_scale,
                        );
                        let _ = canvas.set_scale(current_scale as f32, current_scale as f32);
                        eprintln!("Display settings applied (scale={current_scale}x)");
                    }
                }
                SettingsChanged::Controls => {
                    let controls = crate::settings::ControlsSettings::load();
                    input_map = crate::settings::InputMap::from_controls(&controls);
                    eprintln!("Controls settings applied");
                }
            }
        }

        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => break 'running,
                Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                } => break 'running,
                Event::KeyDown {
                    keycode: Some(keycode),
                    keymod,
                    repeat: false,
                    ..
                } => {
                    // F1-F8: save state; Shift+F1-F8: load state.
                    let fkey_slot = match keycode {
                        Keycode::F1 => Some(1u8),
                        Keycode::F2 => Some(2),
                        Keycode::F3 => Some(3),
                        Keycode::F4 => Some(4),
                        Keycode::F5 => Some(5),
                        Keycode::F6 => Some(6),
                        Keycode::F7 => Some(7),
                        Keycode::F8 => Some(8),
                        _ => None,
                    };
                    if let Some(slot) = fkey_slot {
                        let shift =
                            keymod.contains(Mod::LSHIFTMOD) || keymod.contains(Mod::RSHIFTMOD);
                        if shift {
                            match emulator.load_state(slot) {
                                Ok(()) => eprintln!("Loaded state {slot}"),
                                Err(e) => eprintln!("Load failed: {e}"),
                            }
                        } else {
                            match emulator.save_state(slot) {
                                Ok(()) => eprintln!("Saved state {slot}"),
                                Err(e) => eprintln!("Save failed: {e}"),
                            }
                        }
                    } else if let Some(action) = input_map.lookup(&sdl_key_name(keycode)) {
                        use crate::settings::Action;
                        match action {
                            Action::Pause => {
                                let now_paused = emulator.toggle_pause();
                                eprintln!("{}", if now_paused { "Paused" } else { "Resumed" });
                            }
                            Action::Reset => {
                                match emulator.reset() {
                                    Ok(()) => {
                                        _audio_device = init_audio(&sdl, &mut emulator)?;
                                        joypad = emulator.joypad().clone();
                                        emulator.rewind_clear();
                                        last_rewind_capture_frame = 0;
                                        eprintln!("Reset");
                                    }
                                    Err(e) => eprintln!("Reset failed: {e}"),
                                }
                            }
                            Action::FastForwardToggle => {
                                fast_forward_toggled = !fast_forward_toggled;
                                apply_ff_speed(&emulator, fast_forward_toggled || fast_forward_held, speed, ff_speed);
                                eprintln!("Fast forward {}", if fast_forward_toggled { "ON" } else { "OFF" });
                            }
                            Action::FastForwardHold => {
                                fast_forward_held = true;
                                apply_ff_speed(&emulator, fast_forward_toggled || fast_forward_held, speed, ff_speed);
                            }
                            Action::Rewind => {
                                if !rewinding {
                                    rewinding = true;
                                    emulator.pause();
                                    last_rewind_step = Instant::now();
                                }
                            }
                            Action::Up => joypad.press(crate::input::JoypadButton::Up),
                            Action::Down => joypad.press(crate::input::JoypadButton::Down),
                            Action::Left => joypad.press(crate::input::JoypadButton::Left),
                            Action::Right => joypad.press(crate::input::JoypadButton::Right),
                            Action::A => joypad.press(crate::input::JoypadButton::A),
                            Action::B => joypad.press(crate::input::JoypadButton::B),
                            Action::Start => joypad.press(crate::input::JoypadButton::Start),
                            Action::Select => joypad.press(crate::input::JoypadButton::Select),
                        }
                    }
                }
                Event::KeyUp {
                    keycode: Some(keycode),
                    ..
                } => {
                    if let Some(action) = input_map.lookup(&sdl_key_name(keycode)) {
                        use crate::settings::Action;
                        match action {
                            Action::Rewind => {
                                if rewinding {
                                    rewinding = false;
                                    emulator.resume();
                                }
                            }
                            Action::FastForwardHold => {
                                fast_forward_held = false;
                                apply_ff_speed(&emulator, fast_forward_toggled || fast_forward_held, speed, ff_speed);
                            }
                            Action::Up => joypad.release(crate::input::JoypadButton::Up),
                            Action::Down => joypad.release(crate::input::JoypadButton::Down),
                            Action::Left => joypad.release(crate::input::JoypadButton::Left),
                            Action::Right => joypad.release(crate::input::JoypadButton::Right),
                            Action::A => joypad.release(crate::input::JoypadButton::A),
                            Action::B => joypad.release(crate::input::JoypadButton::B),
                            Action::Start => joypad.release(crate::input::JoypadButton::Start),
                            Action::Select => joypad.release(crate::input::JoypadButton::Select),
                            _ => {} // toggle actions don't need key-up handling
                        }
                    }
                }
                _ => {}
            }
        }

        // --- Rewind step: restore snapshots backwards at fixed rate ---
        if rewinding && last_rewind_step.elapsed() >= REWIND_STEP_INTERVAL {
            if let Some(framebuffer) = emulator.rewind_step() {
                // Display the framebuffer that was saved with this snapshot.
                upload_framebuffer(&mut texture, &framebuffer, &shader)?;
                draw_frame(&mut canvas, &texture)?;
            } else {
                eprintln!("Rewind buffer empty");
                rewinding = false;
                emulator.resume();
            }
            last_rewind_step = Instant::now();
        }

        if uses_vsync {
            let present_started = Instant::now();
            draw_frame(&mut canvas, &texture)?;
            trace_app_stage(
                "present-call",
                last_uploaded_frame,
                present_started.elapsed(),
            );
            trace_present_event("present", last_uploaded_frame, last_present_time.elapsed());
            last_present_time = Instant::now();
        } else {
            thread_sleep_for_frame_interval();
        }

        if let Some(frame) = emulator.take_latest_frame() {
            if frame.frame != last_snapshot_frame {
                trace_present_event("snapshot", frame.frame, last_snapshot_time.elapsed());
                last_snapshot_time = Instant::now();
                last_snapshot_frame = frame.frame;
            }
            let upload_started = Instant::now();
            upload_framebuffer(&mut texture, &frame.framebuffer, &shader)?;
            trace_app_stage("upload", frame.frame, upload_started.elapsed());
            last_uploaded_frame = frame.frame;

            // --- Auto-capture for rewind buffer (every N frames, not while rewinding) ---
            if !rewinding
                && frame.frame >= last_rewind_capture_frame + REWIND_CAPTURE_INTERVAL
            {
                if let Err(e) = emulator.rewind_capture(&frame.framebuffer) {
                    tracing::warn!("rewind capture failed: {e}");
                }
                last_rewind_capture_frame = frame.frame;
            }

            emulator.recycle_framebuffer(frame.framebuffer);
        }

        if !uses_vsync {
            if last_uploaded_frame != last_presented_frame {
                draw_frame(&mut canvas, &texture)?;
                trace_present_event("present", last_uploaded_frame, last_present_time.elapsed());
                last_present_time = Instant::now();
                last_presented_frame = last_uploaded_frame;
            } else {
                thread_sleep();
            }
        }

        if last_title_update.elapsed() >= Duration::from_millis(250) {
            let status_started = Instant::now();
            let snapshot = emulator.status_snapshot();
            trace_app_stage(
                "status-snapshot",
                snapshot.ppu_frames,
                status_started.elapsed(),
            );
            let title_started = Instant::now();
            let rewind_info = if rewinding {
                Some(emulator.rewind_len())
            } else {
                None
            };
            update_title(&mut canvas, &cartridge, &snapshot, emulator.is_paused(), rewind_info)?;
            trace_app_stage("update-title", snapshot.ppu_frames, title_started.elapsed());
            last_title_update = Instant::now();
        }
    }

    emulator.stop();

    Ok(())
}

fn format_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        "just now".to_owned()
    } else if secs < 3600 {
        let mins = secs / 60;
        format!("{}m ago", mins)
    } else if secs < 86400 {
        let hours = secs / 3600;
        format!("{}h ago", hours)
    } else {
        let days = secs / 86400;
        format!("{}d ago", days)
    }
}

fn apply_ff_speed(emulator: &Emulator, active: bool, normal_speed: f32, ff_speed: f32) {
    if active {
        emulator.set_speed(ff_speed);
    } else {
        emulator.set_speed(normal_speed);
    }
}

fn pick_rom_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Load ROM")
        .add_filter("Game Boy ROMs", &["gb", "gbc", "bin", "rom"])
        .add_filter("All files", &["*"])
        .pick_file()
}

fn load_new_rom(
    emulator: &mut Emulator,
    path: &Path,
    dmg_mode: bool,
    speed: f32,
    ppu_delay: Option<Duration>,
) -> Result<(), String> {
    emulator.stop();
    let mut new_emu = Emulator::from_rom_file(path).map_err(|e| e.to_string())?;
    if dmg_mode {
        new_emu.set_dmg_mode();
    }
    if speed != 1.0 {
        new_emu.set_speed(speed);
    }
    if let Some(delay) = ppu_delay {
        new_emu.set_ppu_delay(delay)?;
    }
    new_emu.start()?;
    *emulator = new_emu;
    Ok(())
}

fn run_smoke_frames(
    emulator: &mut Emulator,
    texture: &mut Texture<'_>,
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
) -> Result<(), String> {
    let start = Instant::now();
    let mut saw_activity = false;

    while start.elapsed() < Duration::from_millis(100) {
        let snapshot = emulator.snapshot();
        saw_activity |=
            snapshot.cpu_steps != 0 || snapshot.ppu_frames != 0 || snapshot.timer_ticks != 0;
        upload_framebuffer(texture, &snapshot.framebuffer, &Shader::identity())?;
        draw_frame(canvas, texture)?;

        if saw_activity {
            return Ok(());
        }

        thread_sleep();
    }

    Err("smoke test did not observe emulator activity".to_owned())
}

fn wait_for_live_snapshot(emulator: &mut Emulator, timeout: Duration) -> Snapshot {
    let start = Instant::now();
    let mut last_snapshot = emulator.snapshot();

    while start.elapsed() < timeout {
        last_snapshot = emulator.snapshot();
        if last_snapshot.cpu_steps != 0
            || last_snapshot.ppu_frames != 0
            || last_snapshot.timer_ticks != 0
        {
            break;
        }

        thread_sleep();
    }

    last_snapshot
}

fn thread_sleep() {
    std::thread::sleep(Duration::from_millis(1));
}

fn thread_sleep_for_frame_interval() {
    std::thread::sleep(FRAME_INTERVAL);
}

fn detect_interactive_ppu_delay(
    video: &sdl2::VideoSubsystem,
    window: &Window,
) -> Result<Option<Duration>, String> {
    let display_index = window
        .display_index()
        .map_err(|error| format!("failed to query window display index: {error}"))?;
    let display_mode = video
        .current_display_mode(display_index)
        .map_err(|error| format!("failed to query current display mode: {error}"))?;
    let ppu_delay = half_refresh_ppu_delay(display_mode.refresh_rate);

    trace_display_pacing(display_mode.refresh_rate, ppu_delay);
    Ok(ppu_delay)
}

fn half_refresh_ppu_delay(refresh_rate: i32) -> Option<Duration> {
    if refresh_rate <= 0 {
        return None;
    }

    let refresh_rate = u64::try_from(refresh_rate).ok()?;
    let half_refresh_nanos = (2_000_000_000_u64 + (refresh_rate / 2)) / refresh_rate;
    let half_refresh_delay = Duration::from_nanos(half_refresh_nanos);
    let deviation = duration_abs_diff(half_refresh_delay, DEFAULT_PPU_DELAY);
    if deviation > HALF_REFRESH_MATCH_TOLERANCE {
        return None;
    }

    Some(half_refresh_delay)
}

fn duration_abs_diff(left: Duration, right: Duration) -> Duration {
    left.abs_diff(right)
}

fn upload_framebuffer(texture: &mut Texture<'_>, framebuffer: &[u8], shader: &Shader) -> Result<(), String> {
    texture
        .with_lock(None, |pixels, pitch| {
            let row_width = SCREEN_WIDTH * 4;

            for (row_index, row) in framebuffer.chunks_exact(row_width).enumerate() {
                let offset = row_index * pitch;
                pixels[offset..offset + row_width].copy_from_slice(row);
                // Apply shader in-place on the texture row.
                shader.apply(&mut pixels[offset..offset + row_width]);
            }
        })
        .map_err(|error| format!("failed to upload framebuffer: {error}"))
}

fn load_window_scale() -> u32 {
    crate::settings::DisplaySettings::load().window_scale
}

fn save_window_scale(scale: u32) {
    let mut ds = crate::settings::DisplaySettings::load();
    ds.window_scale = scale;
    ds.save();
}

fn load_shader() -> Shader {
    match crate::settings::shader_path() {
        Some(path) => match Shader::load(&path) {
            Ok(shader) => {
                eprintln!("Shader loaded: {} ({})", shader.name, path.display());
                shader
            }
            Err(e) => {
                eprintln!("Warning: {e}, using identity shader");
                Shader::identity()
            }
        },
        None => Shader::identity(),
    }
}

fn draw_frame(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    texture: &Texture<'_>,
) -> Result<(), String> {
    canvas.set_draw_color(Color::RGB(18, 20, 30));
    canvas.clear();
    canvas
        .copy(texture, None, None)
        .map_err(|error| format!("failed to draw frame: {error}"))?;
    canvas.present();
    Ok(())
}

fn update_title(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    cartridge: &CartridgeMetadata,
    snapshot: &Snapshot,
    paused: bool,
    rewind_info: Option<usize>,
) -> Result<(), String> {
    let status = if let Some(remaining) = rewind_info {
        format!(" [REWIND: {}]", remaining)
    } else if paused {
        " [PAUSED]".to_owned()
    } else {
        String::new()
    };
    let title = format!(
        "Chudtendo | {} | {} | {} | cpu={} ppu={} timer={}{}",
        cartridge.title,
        cartridge.mbc.name(),
        cartridge.boot_mode.name(),
        snapshot.cpu_steps,
        snapshot.ppu_frames,
        snapshot.timer_ticks,
        status,
    );

    canvas
        .window_mut()
        .set_title(&title)
        .map_err(|error| format!("failed to update window title: {error}"))
}

fn trace_present_event(event: &str, ppu_frames: u64, delta: Duration) {
    tracing::debug!(event, frame = ppu_frames, delta_us = delta.as_micros() as u64, "present");
}

fn trace_app_stage(stage: &str, frame: u64, duration: Duration) {
    let threshold = match stage {
        "present-call" => Duration::from_millis(20),
        "upload" => Duration::from_micros(500),
        "status-snapshot" | "update-title" => Duration::from_millis(1),
        _ => Duration::from_micros(500),
    };
    if duration >= threshold {
        tracing::warn!(stage, frame, delta_us = duration.as_micros() as u64, "app stage slow");
    }
}

fn trace_display_pacing(refresh_rate: i32, ppu_delay: Option<Duration>) {
    tracing::debug!(refresh_rate, ppu_delay_ns = ppu_delay.map_or(0, |d| d.as_nanos() as u64), "display pacing");
}

struct AudioOutput {
    receiver: std::sync::mpsc::Receiver<[f32; 2]>,
}

impl AudioCallback for AudioOutput {
    type Channel = f32;

    fn callback(&mut self, out: &mut [f32]) {
        for chunk in out.chunks_exact_mut(2) {
            if let Ok(sample) = self.receiver.try_recv() {
                chunk[0] = sample[0];
                chunk[1] = sample[1];
            } else {
                chunk[0] = 0.0;
                chunk[1] = 0.0;
            }
        }
    }
}

fn init_audio(
    sdl: &sdl2::Sdl,
    emulator: &mut Emulator,
) -> Result<Option<sdl2::audio::AudioDevice<AudioOutput>>, String> {
    let audio = sdl
        .audio()
        .map_err(|error| format!("failed to initialize SDL audio: {error}"))?;

    let Some(receiver) = emulator.take_sample_receiver() else {
        return Ok(None);
    };

    let desired = AudioSpecDesired {
        freq: Some(48_000),
        channels: Some(2),
        samples: Some(1024),
    };

    let device = audio
        .open_playback(None, &desired, |_spec| AudioOutput { receiver })
        .map_err(|error| format!("failed to open audio device: {error}"))?;

    device.resume();
    Ok(Some(device))
}

/// Convert an SDL keycode to the backend-agnostic key name used by keybindings.
///
/// Returns the SDL key name in lowercase (e.g. "z", "return", "up", "space").
/// This is the only SDL-specific mapping in the input pipeline.
fn sdl_key_name(keycode: Keycode) -> String {
    keycode.name().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_PPU_DELAY, HALF_REFRESH_MATCH_TOLERANCE, half_refresh_ppu_delay};
    use std::time::Duration;

    #[test]
    fn half_refresh_matches_120hz_panel() {
        let delay = half_refresh_ppu_delay(120).expect("120 Hz should use exact half-refresh");
        assert_eq!(delay, Duration::from_nanos(16_666_667));
    }

    #[test]
    fn half_refresh_rejects_non_matching_60hz_panel() {
        assert_eq!(half_refresh_ppu_delay(60), None);
    }

    #[test]
    fn half_refresh_stays_close_to_nominal_target() {
        let delay = half_refresh_ppu_delay(120).unwrap();
        assert!(delay.abs_diff(DEFAULT_PPU_DELAY) <= HALF_REFRESH_MATCH_TOLERANCE);
    }
}
