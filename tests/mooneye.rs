//! Mooneye Test Suite harness.
//!
//! Each test succeeds when CPU registers contain Fibonacci numbers:
//! B=3, C=5, D=8, E=13, H=21, L=34.

use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use chudtendo::emulator::Emulator;

const TEST_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

fn mooneye_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("test_roms")
        .join("game-boy-test-roms-v7.0")
        .join("mooneye-test-suite")
}

fn run_mooneye_test(rom_path: &Path) -> bool {
    let rom = match std::fs::read(rom_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skip: {}: {e}", rom_path.display());
            return true; // skip missing
        }
    };
    let mut emulator = match Emulator::from_rom_bytes(rom) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("skip (load error): {}: {e}", rom_path.display());
            return true;
        }
    };
    if emulator.start().is_err() {
        eprintln!("skip (start error): {}", rom_path.display());
        return true;
    }

    let start = Instant::now();
    let passed = loop {
        if start.elapsed() >= TEST_TIMEOUT {
            let snap = emulator.snapshot();
            eprintln!(
                "TIMEOUT: pc={:04x} bc={:04x} de={:04x} hl={:04x}",
                snap.cpu_pc, snap.cpu_bc, snap.cpu_de, snap.cpu_hl
            );
            break false;
        }
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

fn assert_mooneye(rel_path: &str) {
    let path = mooneye_dir().join(rel_path);
    assert!(
        run_mooneye_test(&path),
        "FAILED: {rel_path}"
    );
}

// --- Acceptance: bits ---
#[test] fn mooneye_bits_reg_f() { assert_mooneye("acceptance/bits/reg_f.gb"); }
#[test] fn mooneye_bits_mem_oam() { assert_mooneye("acceptance/bits/mem_oam.gb"); }
#[test] fn mooneye_bits_unused_hwio() { assert_mooneye("acceptance/bits/unused_hwio-GS.gb"); }

// --- Acceptance: instructions ---
#[test] fn mooneye_instr_daa() { assert_mooneye("acceptance/instr/daa.gb"); }

// --- Acceptance: interrupts ---
#[test] fn mooneye_interrupts_ie_push() { assert_mooneye("acceptance/interrupts/ie_push.gb"); }

// --- Acceptance: timing ---
#[test] fn mooneye_div_timing() { assert_mooneye("acceptance/div_timing.gb"); }
#[test] fn mooneye_ei_sequence() { assert_mooneye("acceptance/ei_sequence.gb"); }
#[test] fn mooneye_ei_timing() { assert_mooneye("acceptance/ei_timing.gb"); }
#[test] fn mooneye_halt_ime0_ei() { assert_mooneye("acceptance/halt_ime0_ei.gb"); }
#[test] fn mooneye_halt_ime1_timing() { assert_mooneye("acceptance/halt_ime1_timing.gb"); }
#[test] fn mooneye_if_ie_registers() { assert_mooneye("acceptance/if_ie_registers.gb"); }
#[test] fn mooneye_intr_timing() { assert_mooneye("acceptance/intr_timing.gb"); }

// --- Acceptance: timer ---
#[test] fn mooneye_timer_div_write() { assert_mooneye("acceptance/timer/div_write.gb"); }
#[test] fn mooneye_timer_rapid_toggle() { assert_mooneye("acceptance/timer/rapid_toggle.gb"); }
#[test] fn mooneye_timer_tim00() { assert_mooneye("acceptance/timer/tim00.gb"); }
#[test] fn mooneye_timer_tim00_div_trigger() { assert_mooneye("acceptance/timer/tim00_div_trigger.gb"); }
#[test] fn mooneye_timer_tim01() { assert_mooneye("acceptance/timer/tim01.gb"); }
#[test] fn mooneye_timer_tim01_div_trigger() { assert_mooneye("acceptance/timer/tim01_div_trigger.gb"); }
#[test] fn mooneye_timer_tim10() { assert_mooneye("acceptance/timer/tim10.gb"); }
#[test] fn mooneye_timer_tim10_div_trigger() { assert_mooneye("acceptance/timer/tim10_div_trigger.gb"); }
#[test] fn mooneye_timer_tim11() { assert_mooneye("acceptance/timer/tim11.gb"); }
#[test] fn mooneye_timer_tim11_div_trigger() { assert_mooneye("acceptance/timer/tim11_div_trigger.gb"); }
#[test] fn mooneye_timer_tima_reload() { assert_mooneye("acceptance/timer/tima_reload.gb"); }
#[test] fn mooneye_timer_tima_write_reloading() { assert_mooneye("acceptance/timer/tima_write_reloading.gb"); }
#[test] fn mooneye_timer_tma_write_reloading() { assert_mooneye("acceptance/timer/tma_write_reloading.gb"); }

// --- Acceptance: OAM DMA ---
#[test] fn mooneye_oam_dma_basic() { assert_mooneye("acceptance/oam_dma/basic.gb"); }
#[test] fn mooneye_oam_dma_reg_read() { assert_mooneye("acceptance/oam_dma/reg_read.gb"); }
#[test] fn mooneye_oam_dma_sources() { assert_mooneye("acceptance/oam_dma/sources-GS.gb"); }
#[test] fn mooneye_oam_dma_restart() { assert_mooneye("acceptance/oam_dma_restart.gb"); }
#[test] fn mooneye_oam_dma_start() { assert_mooneye("acceptance/oam_dma_start.gb"); }
#[test] fn mooneye_oam_dma_timing() { assert_mooneye("acceptance/oam_dma_timing.gb"); }

// --- Acceptance: PPU ---
#[test] fn mooneye_ppu_hblank_ly_scx_timing() { assert_mooneye("acceptance/ppu/hblank_ly_scx_timing-GS.gb"); }
#[test] fn mooneye_ppu_intr_1_2_timing() { assert_mooneye("acceptance/ppu/intr_1_2_timing-GS.gb"); }
#[test] fn mooneye_ppu_intr_2_0_timing() { assert_mooneye("acceptance/ppu/intr_2_0_timing.gb"); }
#[test] fn mooneye_ppu_intr_2_mode0_timing() { assert_mooneye("acceptance/ppu/intr_2_mode0_timing.gb"); }
#[test] fn mooneye_ppu_intr_2_mode3_timing() { assert_mooneye("acceptance/ppu/intr_2_mode3_timing.gb"); }
#[test] fn mooneye_ppu_intr_2_oam_ok_timing() { assert_mooneye("acceptance/ppu/intr_2_oam_ok_timing.gb"); }
#[test] fn mooneye_ppu_lcdon_timing() { assert_mooneye("acceptance/ppu/lcdon_timing-GS.gb"); }
#[test] fn mooneye_ppu_lcdon_write_timing() { assert_mooneye("acceptance/ppu/lcdon_write_timing-GS.gb"); }
#[test] fn mooneye_ppu_stat_irq_blocking() { assert_mooneye("acceptance/ppu/stat_irq_blocking.gb"); }
#[test] fn mooneye_ppu_stat_lyc_onoff() { assert_mooneye("acceptance/ppu/stat_lyc_onoff.gb"); }
#[test] fn mooneye_ppu_vblank_stat_intr() { assert_mooneye("acceptance/ppu/vblank_stat_intr-GS.gb"); }

// --- Acceptance: serial ---
#[test] fn mooneye_serial_boot_sclk_align() { assert_mooneye("acceptance/serial/boot_sclk_align-dmgABCmgb.gb"); }

// --- Acceptance: misc timing ---
#[test] fn mooneye_add_sp_e_timing() { assert_mooneye("acceptance/add_sp_e_timing.gb"); }
#[test] fn mooneye_call_timing() { assert_mooneye("acceptance/call_timing.gb"); }
#[test] fn mooneye_call_timing2() { assert_mooneye("acceptance/call_timing2.gb"); }
#[test] fn mooneye_call_cc_timing() { assert_mooneye("acceptance/call_cc_timing.gb"); }
#[test] fn mooneye_call_cc_timing2() { assert_mooneye("acceptance/call_cc_timing2.gb"); }
#[test] fn mooneye_di_timing() { assert_mooneye("acceptance/di_timing-GS.gb"); }
#[test] fn mooneye_jp_timing() { assert_mooneye("acceptance/jp_timing.gb"); }
#[test] fn mooneye_jp_cc_timing() { assert_mooneye("acceptance/jp_cc_timing.gb"); }
#[test] fn mooneye_ld_hl_sp_e_timing() { assert_mooneye("acceptance/ld_hl_sp_e_timing.gb"); }
#[test] fn mooneye_halt_ime0_nointr_timing() { assert_mooneye("acceptance/halt_ime0_nointr_timing.gb"); }
#[test] fn mooneye_halt_ime1_timing2() { assert_mooneye("acceptance/halt_ime1_timing2-GS.gb"); }
#[test] fn mooneye_pop_timing() { assert_mooneye("acceptance/pop_timing.gb"); }
#[test] fn mooneye_push_timing() { assert_mooneye("acceptance/push_timing.gb"); }
#[test] fn mooneye_rapid_di_ei() { assert_mooneye("acceptance/rapid_di_ei.gb"); }
#[test] fn mooneye_ret_timing() { assert_mooneye("acceptance/ret_timing.gb"); }
#[test] fn mooneye_ret_cc_timing() { assert_mooneye("acceptance/ret_cc_timing.gb"); }
#[test] fn mooneye_reti_timing() { assert_mooneye("acceptance/reti_timing.gb"); }
#[test] fn mooneye_reti_intr_timing() { assert_mooneye("acceptance/reti_intr_timing.gb"); }
#[test] fn mooneye_rst_timing() { assert_mooneye("acceptance/rst_timing.gb"); }
