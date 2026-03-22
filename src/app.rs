use std::path::Path;
use std::time::{Duration, Instant};

use sdl2::audio::{AudioCallback, AudioSpecDesired};
use sdl2::event::Event;
use sdl2::keyboard::{Keycode, Mod};
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::render::Texture;
use sdl2::video::Window;

use crate::emulator::{CartridgeMetadata, Emulator, SCREEN_HEIGHT, SCREEN_WIDTH, Snapshot};
use crate::input::Keybindings;

const WINDOW_SCALE: u32 = 4;
const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
const DEFAULT_PPU_DELAY: Duration = Duration::from_nanos(16_536_000);
const HALF_REFRESH_MATCH_TOLERANCE: Duration = Duration::from_micros(2_000);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunMode {
    Interactive,
    SmokeTest,
}

pub fn run(run_mode: RunMode, rom_path: Option<&Path>, dmg_mode: bool) -> Result<(), String> {
    sdl2::hint::set("SDL_RENDER_SCALE_QUALITY", "nearest");

    let sdl = sdl2::init().map_err(|error| format!("failed to initialize SDL: {error}"))?;
    let video = sdl
        .video()
        .map_err(|error| format!("failed to initialize SDL video: {error}"))?;

    let mut window_builder = video.window(
        "Chudtendo",
        SCREEN_WIDTH as u32 * WINDOW_SCALE,
        SCREEN_HEIGHT as u32 * WINDOW_SCALE,
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
        .set_scale(WINDOW_SCALE as f32, WINDOW_SCALE as f32)
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
            emu
        }
        None => Emulator::new(),
    };
    if let Some(ppu_delay) = interactive_ppu_delay {
        emulator.set_ppu_delay(ppu_delay)?;
    }
    let cartridge = emulator.cartridge_metadata().clone();

    if matches!(run_mode, RunMode::SmokeTest) {
        emulator.start()?;
        run_smoke_frames(&mut emulator, &mut texture, &mut canvas)?;
        emulator.stop();
        return Ok(());
    }

    // SDL video should stay on the main thread on macOS, so the emulator modules run in workers.
    emulator.start()?;

    let _audio_device = if matches!(run_mode, RunMode::Interactive) {
        init_audio(&sdl, &mut emulator)?
    } else {
        None
    };

    let keybindings = Keybindings::load_or_default(Path::new("keybindings.yaml"));
    let joypad = emulator.joypad().clone();
    let initial_snapshot = wait_for_live_snapshot(&mut emulator, Duration::from_millis(100));
    upload_framebuffer(&mut texture, &initial_snapshot.framebuffer)?;
    draw_frame(&mut canvas, &texture)?;

    let mut event_pump = sdl
        .event_pump()
        .map_err(|error| format!("failed to create SDL event pump: {error}"))?;
    let mut last_title_update = Instant::now();
    let mut last_uploaded_frame = initial_snapshot.ppu_frames;
    let mut last_presented_frame = initial_snapshot.ppu_frames;
    let mut last_snapshot_frame = initial_snapshot.ppu_frames;
    let mut last_snapshot_time = Instant::now();
    let mut last_present_time = Instant::now();

    'running: loop {
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
                    } else if let Some(button) = keybindings.lookup(&sdl_key_name(keycode)) {
                        joypad.press(button);
                    }
                }
                Event::KeyUp {
                    keycode: Some(keycode),
                    ..
                } => {
                    if let Some(button) = keybindings.lookup(&sdl_key_name(keycode)) {
                        joypad.release(button);
                    }
                }
                _ => {}
            }
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
            upload_framebuffer(&mut texture, &frame.framebuffer)?;
            trace_app_stage("upload", frame.frame, upload_started.elapsed());
            last_uploaded_frame = frame.frame;
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
            update_title(&mut canvas, &cartridge, &snapshot)?;
            trace_app_stage("update-title", snapshot.ppu_frames, title_started.elapsed());
            last_title_update = Instant::now();
        }
    }

    emulator.stop();

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
        upload_framebuffer(texture, &snapshot.framebuffer)?;
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

fn upload_framebuffer(texture: &mut Texture<'_>, framebuffer: &[u8]) -> Result<(), String> {
    texture
        .with_lock(None, |pixels, pitch| {
            let row_width = SCREEN_WIDTH * 4;

            for (row_index, row) in framebuffer.chunks_exact(row_width).enumerate() {
                let offset = row_index * pitch;
                pixels[offset..offset + row_width].copy_from_slice(row);
            }
        })
        .map_err(|error| format!("failed to upload framebuffer: {error}"))
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
) -> Result<(), String> {
    let title = format!(
        "Chudtendo | {} | {} | {} | cpu={} ppu={} timer={}",
        cartridge.title,
        cartridge.mbc.name(),
        cartridge.boot_mode.name(),
        snapshot.cpu_steps,
        snapshot.ppu_frames,
        snapshot.timer_ticks
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
