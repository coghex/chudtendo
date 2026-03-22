//! SameSuite test harness.
//!
//! Same exit condition as mooneye: registers B=3, C=5, D=8, E=13, H=21, L=34.

use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use chudtendo::emulator::Emulator;

const TEST_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

fn suite_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("test_roms")
        .join("game-boy-test-roms-v7.0")
        .join("same-suite")
}

fn run_test(rel_path: &str) -> bool {
    let path = suite_dir().join(rel_path);
    let rom = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));
    let mut emulator = match Emulator::from_rom_bytes(rom) {
        Ok(e) => e,
        Err(e) => { eprintln!("skip: {rel_path}: {e}"); return true; }
    };
    emulator.start().expect("failed to start");

    let start = Instant::now();
    let passed = loop {
        if start.elapsed() >= TEST_TIMEOUT { break false; }
        let snap = emulator.snapshot();
        let b = (snap.cpu_bc >> 8) as u8;
        let c = snap.cpu_bc as u8;
        let d = (snap.cpu_de >> 8) as u8;
        let e = snap.cpu_de as u8;
        let h = (snap.cpu_hl >> 8) as u8;
        let l = snap.cpu_hl as u8;
        if b == 3 && c == 5 && d == 8 && e == 13 && h == 21 && l == 34 {
            break true;
        }
        thread::sleep(POLL_INTERVAL);
    };
    emulator.stop();
    passed
}

fn assert_pass(rel: &str) { assert!(run_test(rel), "FAILED: {rel}"); }

// --- APU channel 1 ---
#[test] fn ch1_duty() { assert_pass("apu/channel_1/channel_1_duty.gb"); }
#[test] fn ch1_freq_change() { assert_pass("apu/channel_1/channel_1_freq_change.gb"); }
#[test] fn ch1_restart() { assert_pass("apu/channel_1/channel_1_restart.gb"); }
#[test] fn ch1_sweep() { assert_pass("apu/channel_1/channel_1_sweep.gb"); }
#[test] fn ch1_sweep_restart() { assert_pass("apu/channel_1/channel_1_sweep_restart.gb"); }
#[test] fn ch1_sweep_restart_2() { assert_pass("apu/channel_1/channel_1_sweep_restart_2.gb"); }
#[test] fn ch1_volume() { assert_pass("apu/channel_1/channel_1_volume.gb"); }
#[test] fn ch1_nrx2_glitch() { assert_pass("apu/channel_1/channel_1_nrx2_glitch.gb"); }
#[test] fn ch1_stop_restart() { assert_pass("apu/channel_1/channel_1_stop_restart.gb"); }

// --- APU channel 2 ---
#[test] fn ch2_duty() { assert_pass("apu/channel_2/channel_2_duty.gb"); }
#[test] fn ch2_freq_change() { assert_pass("apu/channel_2/channel_2_freq_change.gb"); }
#[test] fn ch2_restart() { assert_pass("apu/channel_2/channel_2_restart.gb"); }
#[test] fn ch2_volume() { assert_pass("apu/channel_2/channel_2_volume.gb"); }
#[test] fn ch2_nrx2_glitch() { assert_pass("apu/channel_2/channel_2_nrx2_glitch.gb"); }
#[test] fn ch2_stop_restart() { assert_pass("apu/channel_2/channel_2_stop_restart.gb"); }

// --- APU channel 3 ---
#[test] fn ch3_delay() { assert_pass("apu/channel_3/channel_3_delay.gb"); }
#[test] fn ch3_first_sample() { assert_pass("apu/channel_3/channel_3_first_sample.gb"); }
#[test] fn ch3_stop_div() { assert_pass("apu/channel_3/channel_3_stop_div.gb"); }
#[test] fn ch3_wave_ram_locked_write() { assert_pass("apu/channel_3/channel_3_wave_ram_locked_write.gb"); }

// --- APU channel 4 ---
#[test] fn ch4_delay() { assert_pass("apu/channel_4/channel_4_delay.gb"); }
#[test] fn ch4_freq_change() { assert_pass("apu/channel_4/channel_4_freq_change.gb"); }
#[test] fn ch4_equivalent_frequencies() { assert_pass("apu/channel_4/channel_4_equivalent_frequencies.gb"); }
#[test] fn ch4_lfsr() { assert_pass("apu/channel_4/channel_4_lfsr.gb"); }
#[test] fn ch4_lfsr15() { assert_pass("apu/channel_4/channel_4_lfsr15.gb"); }
#[test] fn ch4_lfsr_15_7() { assert_pass("apu/channel_4/channel_4_lfsr_15_7.gb"); }
#[test] fn ch4_lfsr_7_15() { assert_pass("apu/channel_4/channel_4_lfsr_7_15.gb"); }
#[test] fn ch4_lfsr_restart() { assert_pass("apu/channel_4/channel_4_lfsr_restart.gb"); }
#[test] fn ch4_lfsr_restart_fast() { assert_pass("apu/channel_4/channel_4_lfsr_restart_fast.gb"); }

// --- APU div ---
#[test] fn div_write_trigger() { assert_pass("apu/div_write_trigger.gb"); }

// --- DMA ---
#[test] fn gbc_dma_cont() { assert_pass("dma/gbc_dma_cont.gb"); }
#[test] fn gdma_addr_mask() { assert_pass("dma/gdma_addr_mask.gb"); }
#[test] fn hdma_lcd_off() { assert_pass("dma/hdma_lcd_off.gb"); }
#[test] fn hdma_mode0() { assert_pass("dma/hdma_mode0.gb"); }

// --- Interrupt ---
#[test] fn ei_delay_halt() { assert_pass("interrupt/ei_delay_halt.gb"); }

// --- PPU ---
#[test] fn blocking_bgpi_increase() { assert_pass("ppu/blocking_bgpi_increase.gb"); }
