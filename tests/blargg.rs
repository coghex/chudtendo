//! Blargg's Game Boy test ROM harness.
//!
//! These tests run Blargg's test ROMs headlessly and check serial output
//! for "Passed" / "Failed" strings. ROMs are not included in the repo.
//!
//! Setup:
//!   1. Download Blargg's test ROMs (cpu_instrs, etc.)
//!   2. Place them under `test_roms/blargg/` relative to the project root,
//!      or set the `BLARGG_TEST_ROMS` env var to the directory.
//!
//! Run:
//!   cargo test --test blargg
//!
//! Individual sub-tests:
//!   cargo test --test blargg -- cpu_instrs_01

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use chudtendo::emulator::Emulator;

const TEST_TIMEOUT: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

fn blargg_rom_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("BLARGG_TEST_ROMS") {
        return PathBuf::from(dir);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_roms").join("blargg")
}

fn find_rom(name: &str) -> Option<PathBuf> {
    let dir = blargg_rom_dir();
    // Try exact name, then common subdirectory layouts.
    let candidates = [
        dir.join(name),
        dir.join(format!("{name}/{name}")),
        dir.join(name.replace(".gb", "")).join(name),
    ];
    candidates.into_iter().find(|p| p.exists())
}

fn skip_if_missing(name: &str) -> PathBuf {
    match find_rom(name) {
        Some(path) => path,
        None => {
            eprintln!(
                "SKIPPED: {name} not found in {}. Set BLARGG_TEST_ROMS env var or place ROMs in test_roms/blargg/.",
                blargg_rom_dir().display()
            );
            // Return from the test without failing.
            std::process::exit(0);
        }
    }
}

struct TestResult {
    output: String,
    passed: bool,
    failed: bool,
    timed_out: bool,
}

fn run_blargg_test(rom_path: &Path) -> TestResult {
    run_blargg_test_mode(rom_path, false)
}

fn run_blargg_test_dmg(rom_path: &Path) -> TestResult {
    run_blargg_test_mode(rom_path, true)
}

fn run_blargg_test_mode(rom_path: &Path, dmg_mode: bool) -> TestResult {
    let mut emulator =
        Emulator::from_rom_file(rom_path).expect("failed to load ROM");
    if dmg_mode {
        emulator.set_dmg_mode();
    }
    let serial_rx = emulator
        .take_serial_receiver()
        .expect("serial receiver unavailable");
    emulator.start().expect("failed to start emulator");

    let result = collect_serial_output(&serial_rx, &mut emulator);

    emulator.stop();
    result
}

fn collect_serial_output(serial_rx: &Receiver<u8>, emulator: &mut Emulator) -> TestResult {
    let mut output = String::new();
    let start = Instant::now();

    loop {
        // Check serial port output.
        while let Ok(byte) = serial_rx.try_recv() {
            if byte.is_ascii() && byte != 0 {
                output.push(byte as char);
            }
        }

        // Check SRAM-based text output (used by sound/halt_bug tests).
        // Signature: $A001=$DE, $A002=$B0, $A003=$61.
        if let Some(sram_output) = read_sram_output(emulator) {
            output = sram_output;
        }

        let passed = output.contains("Passed");
        let failed = output.contains("Failed");

        if passed || failed {
            return TestResult {
                output,
                passed,
                failed,
                timed_out: false,
            };
        }

        // Check SRAM result byte: $A000 != $80 means test finished.
        // $FF is open-bus (disabled or absent SRAM) — ignore it.
        if !output.is_empty() {
            if let Some(result) = read_sram_result(emulator) {
                if result != 0x80 && result != 0xff {
                    // Re-read full output one last time.
                    if let Some(final_output) = read_sram_output(emulator) {
                        output = final_output;
                    }
                    let passed = result == 0;
                    return TestResult {
                        output,
                        passed,
                        failed: !passed,
                        timed_out: false,
                    };
                }
            }
        }

        if start.elapsed() >= TEST_TIMEOUT {
            return TestResult {
                output,
                passed: false,
                failed: false,
                timed_out: true,
            };
        }

        std::thread::sleep(POLL_INTERVAL);
    }
}

fn try_read(emulator: &Emulator, address: u16) -> Option<u8> {
    let mut pending = emulator.read(address);
    let deadline = Instant::now() + Duration::from_millis(50);
    while Instant::now() < deadline {
        if let Some(result) = pending.try_take() {
            return match result {
                chudtendo::emulator::ReadResult::Ready(v) => Some(v),
                chudtendo::emulator::ReadResult::NoData => None,
            };
        }
        std::thread::yield_now();
    }
    None
}

fn read_sram_output(emulator: &Emulator) -> Option<String> {
    // Check magic signature at $A001-$A003.
    if try_read(emulator, 0xa001)? != 0xde { return None; }
    if try_read(emulator, 0xa002)? != 0xb0 { return None; }
    if try_read(emulator, 0xa003)? != 0x61 { return None; }

    let mut output = String::new();
    for offset in 0..512u16 {
        let byte = try_read(emulator, 0xa004 + offset).unwrap_or(0);
        if byte == 0 { break; }
        if byte.is_ascii() {
            output.push(byte as char);
        }
    }
    if output.is_empty() { None } else { Some(output) }
}

fn read_sram_result(emulator: &Emulator) -> Option<u8> {
    try_read(emulator, 0xa000)
}

fn assert_blargg_pass_dmg(name: &str) {
    let path = skip_if_missing(name);
    let result = run_blargg_test_dmg(&path);

    if result.timed_out {
        panic!(
            "{name}: TIMED OUT after {TEST_TIMEOUT:?}\nSerial output:\n{}",
            result.output
        );
    }

    if result.failed {
        panic!("{name}: FAILED\nSerial output:\n{}", result.output);
    }

    if !result.passed {
        panic!(
            "{name}: No pass/fail detected\nSerial output:\n{}",
            result.output
        );
    }

    eprintln!("{name}: PASSED");
}

fn assert_blargg_pass(name: &str) {
    let path = skip_if_missing(name);
    let result = run_blargg_test(&path);

    if result.timed_out {
        panic!(
            "{name}: TIMED OUT after {TEST_TIMEOUT:?}\nSerial output:\n{}",
            result.output
        );
    }

    if result.failed {
        panic!("{name}: FAILED\nSerial output:\n{}", result.output);
    }

    if !result.passed {
        panic!(
            "{name}: No pass/fail detected\nSerial output:\n{}",
            result.output
        );
    }

    eprintln!("{name}: PASSED");
}

// --- Individual cpu_instrs sub-tests ---
// These are the 11 individual test ROMs from Blargg's cpu_instrs suite.
// Each can be run independently for faster iteration.

#[test]
fn cpu_instrs_01_special() {
    assert_blargg_pass("cpu_instrs/individual/01-special.gb");
}

#[test]
fn cpu_instrs_02_interrupts() {
    assert_blargg_pass("cpu_instrs/individual/02-interrupts.gb");
}

#[test]
fn cpu_instrs_03_op_sp_hl() {
    assert_blargg_pass("cpu_instrs/individual/03-op sp,hl.gb");
}

#[test]
fn cpu_instrs_04_op_r_imm() {
    assert_blargg_pass("cpu_instrs/individual/04-op r,imm.gb");
}

#[test]
fn cpu_instrs_05_op_rp() {
    assert_blargg_pass("cpu_instrs/individual/05-op rp.gb");
}

#[test]
fn cpu_instrs_06_ld_r_r() {
    assert_blargg_pass("cpu_instrs/individual/06-ld r,r.gb");
}

#[test]
fn cpu_instrs_07_jr_jp_call_ret_rst() {
    assert_blargg_pass("cpu_instrs/individual/07-jr,jp,call,ret,rst.gb");
}

#[test]
fn cpu_instrs_08_misc_instrs() {
    assert_blargg_pass("cpu_instrs/individual/08-misc instrs.gb");
}

#[test]
fn cpu_instrs_09_op_r_r() {
    assert_blargg_pass("cpu_instrs/individual/09-op r,r.gb");
}

#[test]
fn cpu_instrs_10_bit_ops() {
    assert_blargg_pass("cpu_instrs/individual/10-bit ops.gb");
}

#[test]
fn cpu_instrs_11_op_a_hl() {
    assert_blargg_pass("cpu_instrs/individual/11-op a,(hl).gb");
}

#[test]
fn cpu_instrs_combined() {
    assert_blargg_pass("cpu_instrs/cpu_instrs.gb");
}

// --- Instruction timing ---

#[test]
fn instr_timing() {
    assert_blargg_pass("instr_timing/instr_timing.gb");
}

// --- Memory timing ---

#[test]
fn mem_timing_01_read() {
    assert_blargg_pass("mem_timing/individual/01-read_timing.gb");
}

#[test]
fn mem_timing_02_write() {
    assert_blargg_pass("mem_timing/individual/02-write_timing.gb");
}

#[test]
fn mem_timing_03_modify() {
    assert_blargg_pass("mem_timing/individual/03-modify_timing.gb");
}

// --- Memory timing 2 ---
// Requires cycle-accurate memory access; async bus cannot pass these.

#[test]
fn mem_timing2_01_read() {
    assert_blargg_pass("mem_timing-2/rom_singles/01-read_timing.gb");
}

#[test]
fn mem_timing2_02_write() {
    assert_blargg_pass("mem_timing-2/rom_singles/02-write_timing.gb");
}

#[test]
fn mem_timing2_03_modify() {
    assert_blargg_pass("mem_timing-2/rom_singles/03-modify_timing.gb");
}

// --- Halt bug ---
// Requires cycle-accurate HALT timing; times out in async architecture.

#[test]
fn halt_bug() {
    assert_blargg_pass_dmg("halt_bug.gb");
}

// --- DMG sound ---
// Sound tests time out — Blargg's framework uses timer-dependent delays
// during initialization which hang in the async architecture.

#[test]
fn dmg_sound_01_registers() {
    assert_blargg_pass_dmg("dmg_sound/rom_singles/01-registers.gb");
}

#[test]
fn dmg_sound_02_len_ctr() {
    assert_blargg_pass_dmg("dmg_sound/rom_singles/02-len ctr.gb");
}

#[test]
fn dmg_sound_03_trigger() {
    assert_blargg_pass_dmg("dmg_sound/rom_singles/03-trigger.gb");
}

#[test]
fn dmg_sound_04_sweep() {
    assert_blargg_pass_dmg("dmg_sound/rom_singles/04-sweep.gb");
}

#[test]
fn dmg_sound_05_sweep_details() {
    assert_blargg_pass_dmg("dmg_sound/rom_singles/05-sweep details.gb");
}

#[test]
fn dmg_sound_06_overflow_on_trigger() {
    assert_blargg_pass_dmg("dmg_sound/rom_singles/06-overflow on trigger.gb");
}

#[test]
fn dmg_sound_07_len_sweep_period_sync() {
    assert_blargg_pass_dmg("dmg_sound/rom_singles/07-len sweep period sync.gb");
}

#[test]
fn dmg_sound_08_len_ctr_during_power() {
    assert_blargg_pass_dmg("dmg_sound/rom_singles/08-len ctr during power.gb");
}

#[test]
fn dmg_sound_09_wave_read_while_on() {
    assert_blargg_pass_dmg("dmg_sound/rom_singles/09-wave read while on.gb");
}

#[test]
fn dmg_sound_10_wave_trigger_while_on() {
    assert_blargg_pass_dmg("dmg_sound/rom_singles/10-wave trigger while on.gb");
}

#[test]
fn dmg_sound_11_regs_after_power() {
    assert_blargg_pass_dmg("dmg_sound/rom_singles/11-regs after power.gb");
}

#[test]
fn dmg_sound_12_wave_write_while_on() {
    assert_blargg_pass_dmg("dmg_sound/rom_singles/12-wave write while on.gb");
}

// --- CGB sound ---

#[test]
fn cgb_sound_01_registers() {
    assert_blargg_pass("cgb_sound/rom_singles/01-registers.gb");
}

#[test]
fn cgb_sound_02_len_ctr() {
    assert_blargg_pass("cgb_sound/rom_singles/02-len ctr.gb");
}

#[test]
fn cgb_sound_03_trigger() {
    assert_blargg_pass("cgb_sound/rom_singles/03-trigger.gb");
}

#[test]
fn cgb_sound_04_sweep() {
    assert_blargg_pass("cgb_sound/rom_singles/04-sweep.gb");
}

#[test]
fn cgb_sound_05_sweep_details() {
    assert_blargg_pass("cgb_sound/rom_singles/05-sweep details.gb");
}

#[test]
fn cgb_sound_06_overflow_on_trigger() {
    assert_blargg_pass("cgb_sound/rom_singles/06-overflow on trigger.gb");
}

#[test]
fn cgb_sound_07_len_sweep_period_sync() {
    assert_blargg_pass("cgb_sound/rom_singles/07-len sweep period sync.gb");
}

#[test]
fn cgb_sound_08_len_ctr_during_power() {
    assert_blargg_pass("cgb_sound/rom_singles/08-len ctr during power.gb");
}

#[test]
fn cgb_sound_09_wave_read_while_on() {
    assert_blargg_pass("cgb_sound/rom_singles/09-wave read while on.gb");
}

#[test]
fn cgb_sound_10_wave_trigger_while_on() {
    assert_blargg_pass("cgb_sound/rom_singles/10-wave trigger while on.gb");
}

#[test]
fn cgb_sound_11_regs_after_power() {
    assert_blargg_pass("cgb_sound/rom_singles/11-regs after power.gb");
}

#[test]
fn cgb_sound_12_wave() {
    assert_blargg_pass("cgb_sound/rom_singles/12-wave.gb");
}
