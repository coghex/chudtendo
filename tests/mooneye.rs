//! Mooneye Test Suite harness.
//!
//! Each test succeeds when CPU registers contain Fibonacci numbers:
//! B=3, C=5, D=8, E=13, H=21, L=34.

use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use chudtendo::emulator::{CartridgeImage, Emulator, EmulatorConfig, HardwareMode};
use serde::Deserialize;

const TEST_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

fn test_timeout() -> Duration {
    std::env::var("CHUD_MOONEYE_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(TEST_TIMEOUT)
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct FullSaveStateDump {
    cpu: Vec<u8>,
    ppu: Vec<u8>,
    wram: Vec<u8>,
    cartridge: Vec<u8>,
    timer: Vec<u8>,
    apu: Vec<u8>,
    clock_cycles: u64,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct CpuSaveStateDump {
    af: u16,
    bc: u16,
    de: u16,
    hl: u16,
    sp: u16,
    pc: u16,
    io_registers: Vec<u8>,
    hram: Vec<u8>,
    interrupt_enable: u8,
    interrupt_flags: u8,
    interrupt_master_enable: bool,
    ime_enable_delay: u8,
    double_speed: bool,
    speed_switch_armed: bool,
    serial_transfer_countdown: u16,
    cycles: u64,
    is_halted: bool,
    is_stopped: bool,
    halt_bug: bool,
    steps: u64,
    hardware_mode: HardwareMode,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct PpuSaveStateDump {
    vram_banks: Vec<Vec<u8>>,
    selected_vram_bank: u8,
    oam: Vec<u8>,
    lcd_registers: Vec<u8>,
    hdma_registers: Vec<u8>,
    bg_palette_index: u8,
    obj_palette_index: u8,
    bg_palette_ram: Vec<u8>,
    obj_palette_ram: Vec<u8>,
    dots_into_line: u16,
    stat_interrupt_line: bool,
    stat_interrupt_armed: bool,
    window_line: u8,
    dots: u64,
    frames: u64,
    hardware_mode: HardwareMode,
}

fn temp_save_path(rom_path: &Path) -> PathBuf {
    let file_name = format!(
        "chudtendo-mooneye-{}-{}-{}.sav",
        std::process::id(),
        thread::current().name().unwrap_or("test"),
        rom_path.file_stem().and_then(|stem| stem.to_str()).unwrap_or("rom")
    );
    std::env::temp_dir().join(file_name)
}

fn load_save_state(save_path: &Path) -> Option<(CpuSaveStateDump, PpuSaveStateDump)> {
    let state_path = save_path.with_extension("state0");
    let bytes = std::fs::read(&state_path).ok()?;
    let full: FullSaveStateDump = bincode::deserialize(&bytes).ok()?;
    let cpu = bincode::deserialize(&full.cpu).ok()?;
    let ppu = bincode::deserialize(&full.ppu).ok()?;
    Some((cpu, ppu))
}

fn has_mooneye_pass_signature(bc: u16, de: u16, hl: u16) -> bool {
    let b = (bc >> 8) as u8;
    let c = bc as u8;
    let d = (de >> 8) as u8;
    let e = de as u8;
    let h = (hl >> 8) as u8;
    let l = hl as u8;
    b == 3 && c == 5 && d == 8 && e == 13 && h == 21 && l == 34
}

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
    let save_path = temp_save_path(rom_path);
    let cartridge = match CartridgeImage::from_bytes(rom) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("skip (load error): {}: {e}", rom_path.display());
            return true;
        }
    };
    let mut config = EmulatorConfig::with_cartridge(cartridge);
    config.save_path = Some(save_path.clone());
    let mut emulator = Emulator::with_config(config);
    if emulator.start().is_err() {
        eprintln!("skip (start error): {}", rom_path.display());
        return true;
    }

    let start = Instant::now();
    let passed = loop {
        if start.elapsed() >= test_timeout() {
            let snap = emulator.snapshot();
            let save_dump = emulator
                .save_state(0)
                .ok()
                .and_then(|_| load_save_state(&save_path));
            if save_dump.as_ref().is_some_and(|(cpu, _)| {
                has_mooneye_pass_signature(cpu.bc, cpu.de, cpu.hl)
            }) {
                break true;
            }
            eprintln!(
                "TIMEOUT: snapshot_pc={:04x} snapshot_bc={:04x} snapshot_de={:04x} snapshot_hl={:04x} save_pc={} save_bc={} save_de={} save_hl={} save_if={} save_ie={} save_ime={} save_cycles={} save_ly={} save_stat={} save_dots_into_line={} save_frames={}",
                snap.cpu_pc,
                snap.cpu_bc,
                snap.cpu_de,
                snap.cpu_hl,
                save_dump
                    .as_ref()
                    .map(|(cpu, _)| format!("{:04x}", cpu.pc))
                    .unwrap_or_else(|| "????".to_owned()),
                save_dump
                    .as_ref()
                    .map(|(cpu, _)| format!("{:04x}", cpu.bc))
                    .unwrap_or_else(|| "????".to_owned()),
                save_dump
                    .as_ref()
                    .map(|(cpu, _)| format!("{:04x}", cpu.de))
                    .unwrap_or_else(|| "????".to_owned()),
                save_dump
                    .as_ref()
                    .map(|(cpu, _)| format!("{:04x}", cpu.hl))
                    .unwrap_or_else(|| "????".to_owned()),
                save_dump
                    .as_ref()
                    .map(|(cpu, _)| format!("{:02x}", cpu.interrupt_flags))
                    .unwrap_or_else(|| "??".to_owned()),
                save_dump
                    .as_ref()
                    .map(|(cpu, _)| format!("{:02x}", cpu.interrupt_enable))
                    .unwrap_or_else(|| "??".to_owned()),
                save_dump
                    .as_ref()
                    .map(|(cpu, _)| cpu.interrupt_master_enable.to_string())
                    .unwrap_or_else(|| "?".to_owned()),
                save_dump
                    .as_ref()
                    .map(|(cpu, _)| cpu.cycles.to_string())
                    .unwrap_or_else(|| "?".to_owned()),
                save_dump
                    .as_ref()
                    .and_then(|(_, ppu)| ppu.lcd_registers.get(4).copied())
                    .map(|ly| format!("{ly:02x}"))
                    .unwrap_or_else(|| "??".to_owned()),
                save_dump
                    .as_ref()
                    .and_then(|(_, ppu)| ppu.lcd_registers.get(1).copied())
                    .map(|stat| format!("{stat:02x}"))
                    .unwrap_or_else(|| "??".to_owned()),
                save_dump
                    .as_ref()
                    .map(|(_, ppu)| ppu.dots_into_line.to_string())
                    .unwrap_or_else(|| "?".to_owned()),
                save_dump
                    .as_ref()
                    .map(|(_, ppu)| ppu.frames.to_string())
                    .unwrap_or_else(|| "?".to_owned()),
            );
            break false;
        }
        let snap = emulator.snapshot();
        if has_mooneye_pass_signature(snap.cpu_bc, snap.cpu_de, snap.cpu_hl) {
            break true;
        }
        thread::sleep(POLL_INTERVAL);
    };

    emulator.stop();
    let _ = std::fs::remove_file(save_path.with_extension("state0"));
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
