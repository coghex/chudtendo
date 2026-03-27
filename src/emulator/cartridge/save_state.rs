use std::fs;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use super::controller::{CartridgeController, Mbc1Mode, MBC2_RAM_SIZE, EEPROM_WORD_COUNT, TAMA5_RAM_SIZE};
use super::image::CartridgeMetadata;

#[derive(Serialize, Deserialize)]
pub struct CartridgeSaveState {
    pub ram: Vec<u8>,
    pub boot_rom_mapped: bool,
    pub controller: CartridgeControllerSave,
}

#[derive(Serialize, Deserialize)]
pub enum CartridgeControllerSave {
    RomOnly,
    Mbc1 {
        ram_enabled: bool,
        rom_bank_low5: u8,
        bank_high2: u8,
        mode_is_ram_banking: bool,
    },
    Mbc2 {
        ram_enabled: bool,
        rom_bank: u8,
        ram: Vec<u8>,
    },
    Mbc3 {
        ram_enabled: bool,
        rom_bank: u8,
        bank_select: u8,
        latch_prep: bool,
        rtc_seconds: u8,
        rtc_minutes: u8,
        rtc_hours: u8,
        rtc_day_low: u8,
        rtc_day_high: u8,
        rtc_latched_seconds: u8,
        rtc_latched_minutes: u8,
        rtc_latched_hours: u8,
        rtc_latched_day_low: u8,
        rtc_latched_day_high: u8,
        /// Elapsed milliseconds since base_instant (for RTC reconstruction).
        rtc_elapsed_ms: u64,
    },
    Mbc5 {
        ram_enabled: bool,
        rom_bank_low: u8,
        rom_bank_high: u8,
        ram_bank: u8,
    },
    HuC1 {
        ram_enabled: bool,
        rom_bank_low5: u8,
        bank_high2: u8,
        mode_is_ram_banking: bool,
        ir_mode: bool,
    },
    HuC3 {
        mode: u8,
        rom_bank: u8,
        ram_bank: u8,
        shift: u32,
        output: u8,
        rtc_minutes: u16,
        rtc_days: u16,
        rtc_elapsed_ms: u64,
    },
    Mbc7 {
        rom_bank: u8,
        eeprom_data: Vec<u8>,
    },
    Mmm01 {
        mapped: bool,
        ram_enabled: bool,
        rom_bank_low5: u8,
        bank_high2: u8,
        mode_is_ram_banking: bool,
        rom_bank_base: u32,
    },
    Mbc6 {
        rom_bank_a: u8,
        rom_bank_b: u8,
        ram_bank_a: u8,
        ram_bank_b: u8,
        flash_data: Vec<u8>,
    },
    PocketCamera {
        ram_enabled: bool,
        rom_bank_low: u8,
        rom_bank_high: u8,
        ram_bank: u8,
    },
    BandaiTama5 {
        rom_bank: u8,
        data_reg: u8,
        output: u8,
        ram: Vec<u8>,
        rtc_regs: Vec<u8>,
        rtc_elapsed_ms: u64,
    },
}

pub(super) fn create_save(controller: &CartridgeController, ram: &[u8], boot_rom_mapped: bool) -> CartridgeSaveState {
    let controller = match controller {
        CartridgeController::RomOnly => CartridgeControllerSave::RomOnly,
        CartridgeController::Mbc1(s) => CartridgeControllerSave::Mbc1 {
            ram_enabled: s.ram_enabled,
            rom_bank_low5: s.rom_bank_low5,
            bank_high2: s.bank_high2,
            mode_is_ram_banking: matches!(s.mode, Mbc1Mode::RamBanking),
        },
        CartridgeController::Mbc2(s) => CartridgeControllerSave::Mbc2 {
            ram_enabled: s.ram_enabled,
            rom_bank: s.rom_bank,
            ram: s.ram.to_vec(),
        },
        CartridgeController::Mbc3(s) => {
            // Advance the RTC before snapshotting so the elapsed duration is captured.
            let elapsed_ms = s.rtc.base_instant.elapsed().as_millis() as u64;
            CartridgeControllerSave::Mbc3 {
                ram_enabled: s.ram_enabled,
                rom_bank: s.rom_bank,
                bank_select: s.bank_select,
                latch_prep: s.latch_prep,
                rtc_seconds: s.rtc.seconds,
                rtc_minutes: s.rtc.minutes,
                rtc_hours: s.rtc.hours,
                rtc_day_low: s.rtc.day_low,
                rtc_day_high: s.rtc.day_high,
                rtc_latched_seconds: s.rtc.latched_seconds,
                rtc_latched_minutes: s.rtc.latched_minutes,
                rtc_latched_hours: s.rtc.latched_hours,
                rtc_latched_day_low: s.rtc.latched_day_low,
                rtc_latched_day_high: s.rtc.latched_day_high,
                rtc_elapsed_ms: elapsed_ms,
            }
        }
        CartridgeController::Mbc5(s) => CartridgeControllerSave::Mbc5 {
            ram_enabled: s.ram_enabled,
            rom_bank_low: s.rom_bank_low,
            rom_bank_high: s.rom_bank_high,
            ram_bank: s.ram_bank,
        },
        CartridgeController::HuC1(s) => CartridgeControllerSave::HuC1 {
            ram_enabled: s.ram_enabled,
            rom_bank_low5: s.rom_bank_low5,
            bank_high2: s.bank_high2,
            mode_is_ram_banking: matches!(s.mode, Mbc1Mode::RamBanking),
            ir_mode: s.ir_mode,
        },
        CartridgeController::Mbc7(s) => CartridgeControllerSave::Mbc7 {
            rom_bank: s.rom_bank,
            eeprom_data: s.eeprom.to_bytes(),
        },
        CartridgeController::Mbc6(s) => CartridgeControllerSave::Mbc6 {
            rom_bank_a: s.rom_bank_a,
            rom_bank_b: s.rom_bank_b,
            ram_bank_a: s.ram_bank_a,
            ram_bank_b: s.ram_bank_b,
            flash_data: s.flash.clone(),
        },
        CartridgeController::PocketCamera(s) => CartridgeControllerSave::PocketCamera {
            ram_enabled: s.ram_enabled,
            rom_bank_low: s.rom_bank_low,
            rom_bank_high: s.rom_bank_high,
            ram_bank: s.ram_bank,
        },
        CartridgeController::BandaiTama5(s) => {
            let elapsed_ms = s.rtc.base_instant.elapsed().as_millis() as u64;
            CartridgeControllerSave::BandaiTama5 {
                rom_bank: s.rom_bank,
                data_reg: s.data_reg,
                output: s.output,
                ram: s.ram.to_vec(),
                rtc_regs: s.rtc.regs.to_vec(),
                rtc_elapsed_ms: elapsed_ms,
            }
        }
        CartridgeController::Mmm01(s) => CartridgeControllerSave::Mmm01 {
            mapped: s.mapped,
            ram_enabled: s.ram_enabled,
            rom_bank_low5: s.rom_bank_low5,
            bank_high2: s.bank_high2,
            mode_is_ram_banking: matches!(s.mode, Mbc1Mode::RamBanking),
            rom_bank_base: s.rom_bank_base as u32,
        },
        CartridgeController::HuC3(s) => {
            let elapsed_ms = s.rtc.base_instant.elapsed().as_millis() as u64;
            CartridgeControllerSave::HuC3 {
                mode: s.mode,
                rom_bank: s.rom_bank,
                ram_bank: s.ram_bank,
                shift: s.shift,
                output: s.output,
                rtc_minutes: s.rtc.minutes,
                rtc_days: s.rtc.days,
                rtc_elapsed_ms: elapsed_ms,
            }
        }
    };

    CartridgeSaveState {
        ram: ram.to_vec(),
        boot_rom_mapped,
        controller,
    }
}

pub(super) fn apply_save(
    controller: &mut CartridgeController,
    ram: &mut [u8],
    boot_rom_mapped: &mut bool,
    state: CartridgeSaveState,
) {
    let len = state.ram.len().min(ram.len());
    ram[..len].copy_from_slice(&state.ram[..len]);
    *boot_rom_mapped = state.boot_rom_mapped;

    match (controller, state.controller) {
        (CartridgeController::RomOnly, CartridgeControllerSave::RomOnly) => {}
        (CartridgeController::Mbc1(s), CartridgeControllerSave::Mbc1 {
            ram_enabled, rom_bank_low5, bank_high2, mode_is_ram_banking,
        }) => {
            s.ram_enabled = ram_enabled;
            s.rom_bank_low5 = rom_bank_low5;
            s.bank_high2 = bank_high2;
            s.mode = if mode_is_ram_banking { Mbc1Mode::RamBanking } else { Mbc1Mode::RomBanking };
        }
        (CartridgeController::Mbc2(s), CartridgeControllerSave::Mbc2 {
            ram_enabled, rom_bank, ram,
        }) => {
            s.ram_enabled = ram_enabled;
            s.rom_bank = rom_bank;
            let len = ram.len().min(s.ram.len());
            s.ram[..len].copy_from_slice(&ram[..len]);
        }
        (CartridgeController::Mbc3(s), CartridgeControllerSave::Mbc3 {
            ram_enabled, rom_bank, bank_select, latch_prep,
            rtc_seconds, rtc_minutes, rtc_hours, rtc_day_low, rtc_day_high,
            rtc_latched_seconds, rtc_latched_minutes, rtc_latched_hours,
            rtc_latched_day_low, rtc_latched_day_high, rtc_elapsed_ms,
        }) => {
            s.ram_enabled = ram_enabled;
            s.rom_bank = rom_bank;
            s.bank_select = bank_select;
            s.latch_prep = latch_prep;
            s.rtc.seconds = rtc_seconds;
            s.rtc.minutes = rtc_minutes;
            s.rtc.hours = rtc_hours;
            s.rtc.day_low = rtc_day_low;
            s.rtc.day_high = rtc_day_high;
            s.rtc.latched_seconds = rtc_latched_seconds;
            s.rtc.latched_minutes = rtc_latched_minutes;
            s.rtc.latched_hours = rtc_latched_hours;
            s.rtc.latched_day_low = rtc_latched_day_low;
            s.rtc.latched_day_high = rtc_latched_day_high;
            // Reconstruct base_instant so elapsed time since save-state is counted correctly.
            // We set base_instant to (now - elapsed_ms) so the RTC continues from where
            // it was when the state was saved.
            let elapsed_dur = std::time::Duration::from_millis(rtc_elapsed_ms);
            s.rtc.base_instant = Instant::now()
                .checked_sub(elapsed_dur)
                .unwrap_or_else(Instant::now);
        }
        (CartridgeController::Mbc5(s), CartridgeControllerSave::Mbc5 {
            ram_enabled, rom_bank_low, rom_bank_high, ram_bank,
        }) => {
            s.ram_enabled = ram_enabled;
            s.rom_bank_low = rom_bank_low;
            s.rom_bank_high = rom_bank_high;
            s.ram_bank = ram_bank;
        }
        (CartridgeController::HuC1(s), CartridgeControllerSave::HuC1 {
            ram_enabled, rom_bank_low5, bank_high2, mode_is_ram_banking, ir_mode,
        }) => {
            s.ram_enabled = ram_enabled;
            s.rom_bank_low5 = rom_bank_low5;
            s.bank_high2 = bank_high2;
            s.mode = if mode_is_ram_banking { Mbc1Mode::RamBanking } else { Mbc1Mode::RomBanking };
            s.ir_mode = ir_mode;
        }
        (CartridgeController::Mbc7(s), CartridgeControllerSave::Mbc7 {
            rom_bank, eeprom_data,
        }) => {
            s.rom_bank = rom_bank;
            s.eeprom.load_bytes(&eeprom_data);
        }
        (CartridgeController::Mbc6(s), CartridgeControllerSave::Mbc6 {
            rom_bank_a, rom_bank_b, ram_bank_a, ram_bank_b, flash_data,
        }) => {
            s.rom_bank_a = rom_bank_a;
            s.rom_bank_b = rom_bank_b;
            s.ram_bank_a = ram_bank_a;
            s.ram_bank_b = ram_bank_b;
            let len = flash_data.len().min(s.flash.len());
            s.flash[..len].copy_from_slice(&flash_data[..len]);
        }
        (CartridgeController::PocketCamera(s), CartridgeControllerSave::PocketCamera {
            ram_enabled, rom_bank_low, rom_bank_high, ram_bank,
        }) => {
            s.ram_enabled = ram_enabled;
            s.rom_bank_low = rom_bank_low;
            s.rom_bank_high = rom_bank_high;
            s.ram_bank = ram_bank;
        }
        (CartridgeController::BandaiTama5(s), CartridgeControllerSave::BandaiTama5 {
            rom_bank, data_reg, output, ram, rtc_regs, rtc_elapsed_ms,
        }) => {
            s.rom_bank = rom_bank;
            s.data_reg = data_reg;
            s.output = output;
            let len = ram.len().min(s.ram.len());
            s.ram[..len].copy_from_slice(&ram[..len]);
            let rlen = rtc_regs.len().min(s.rtc.regs.len());
            s.rtc.regs[..rlen].copy_from_slice(&rtc_regs[..rlen]);
            let elapsed_dur = std::time::Duration::from_millis(rtc_elapsed_ms);
            s.rtc.base_instant = Instant::now()
                .checked_sub(elapsed_dur)
                .unwrap_or_else(Instant::now);
        }
        (CartridgeController::Mmm01(s), CartridgeControllerSave::Mmm01 {
            mapped, ram_enabled, rom_bank_low5, bank_high2, mode_is_ram_banking, rom_bank_base,
        }) => {
            s.mapped = mapped;
            s.ram_enabled = ram_enabled;
            s.rom_bank_low5 = rom_bank_low5;
            s.bank_high2 = bank_high2;
            s.mode = if mode_is_ram_banking { Mbc1Mode::RamBanking } else { Mbc1Mode::RomBanking };
            s.rom_bank_base = rom_bank_base as usize;
        }
        (CartridgeController::HuC3(s), CartridgeControllerSave::HuC3 {
            mode, rom_bank, ram_bank, shift, output,
            rtc_minutes, rtc_days, rtc_elapsed_ms,
        }) => {
            s.mode = mode;
            s.rom_bank = rom_bank;
            s.ram_bank = ram_bank;
            s.shift = shift;
            s.output = output;
            s.rtc.minutes = rtc_minutes;
            s.rtc.days = rtc_days;
            let elapsed_dur = std::time::Duration::from_millis(rtc_elapsed_ms);
            s.rtc.base_instant = Instant::now()
                .checked_sub(elapsed_dur)
                .unwrap_or_else(Instant::now);
        }
        _ => {
            // MBC type mismatch — silently ignore to avoid corrupting state.
        }
    }
}

const RTC_SAVE_SIZE: usize = 48;

pub(super) fn load_save_file(
    path: &std::path::Path,
    ram: &mut [u8],
    controller: &mut CartridgeController,
    metadata: &CartridgeMetadata,
) {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(_) => return,
    };

    match controller {
        CartridgeController::Mbc2(state) => {
            let len = data.len().min(MBC2_RAM_SIZE);
            state.ram[..len].copy_from_slice(&data[..len]);
            for byte in &mut state.ram {
                *byte &= 0x0f;
            }
        }
        CartridgeController::BandaiTama5(state) => {
            let len = data.len().min(TAMA5_RAM_SIZE);
            state.ram[..len].copy_from_slice(&data[..len]);
        }
        CartridgeController::Mbc6(state) => {
            // Save file: RAM followed by flash data.
            let ram_len = data.len().min(ram.len());
            if !ram.is_empty() {
                ram[..ram_len].copy_from_slice(&data[..ram_len]);
            }
            if data.len() > ram.len() {
                let flash_data = &data[ram.len()..];
                let len = flash_data.len().min(state.flash.len());
                state.flash[..len].copy_from_slice(&flash_data[..len]);
            }
        }
        CartridgeController::Mbc7(state) => {
            state.eeprom.load_bytes(&data);
        }
        CartridgeController::Mbc3(state) if metadata.has_rtc() => {
            let ram_len = data.len().min(ram.len());
            ram[..ram_len].copy_from_slice(&data[..ram_len]);
            if data.len() >= ram.len() + RTC_SAVE_SIZE {
                let rtc_offset = ram.len();
                state.rtc.seconds = data[rtc_offset];
                state.rtc.minutes = data[rtc_offset + 4];
                state.rtc.hours = data[rtc_offset + 8];
                state.rtc.day_low = data[rtc_offset + 12];
                state.rtc.day_high = data[rtc_offset + 16];
                state.rtc.latched_seconds = data[rtc_offset + 20];
                state.rtc.latched_minutes = data[rtc_offset + 24];
                state.rtc.latched_hours = data[rtc_offset + 28];
                state.rtc.latched_day_low = data[rtc_offset + 32];
                state.rtc.latched_day_high = data[rtc_offset + 36];
            }
        }
        _ => {
            let len = data.len().min(ram.len());
            if !ram.is_empty() {
                ram[..len].copy_from_slice(&data[..len]);
            }
        }
    }
}

pub(super) fn write_save_file(
    path: &std::path::Path,
    ram: &[u8],
    controller: &CartridgeController,
    metadata: &CartridgeMetadata,
) {
    let data = match controller {
        CartridgeController::Mbc2(state) => state.ram.to_vec(),
        CartridgeController::Mbc6(state) => {
            // Save RAM followed by flash data.
            let mut data = ram.to_vec();
            data.extend_from_slice(&state.flash);
            data
        }
        CartridgeController::BandaiTama5(state) => state.ram.to_vec(),
        CartridgeController::Mbc7(state) => state.eeprom.to_bytes(),
        CartridgeController::Mbc3(state) if metadata.has_rtc() => {
            let mut data = ram.to_vec();
            let mut rtc_block = [0u8; RTC_SAVE_SIZE];
            rtc_block[0] = state.rtc.seconds;
            rtc_block[4] = state.rtc.minutes;
            rtc_block[8] = state.rtc.hours;
            rtc_block[12] = state.rtc.day_low;
            rtc_block[16] = state.rtc.day_high;
            rtc_block[20] = state.rtc.latched_seconds;
            rtc_block[24] = state.rtc.latched_minutes;
            rtc_block[28] = state.rtc.latched_hours;
            rtc_block[32] = state.rtc.latched_day_low;
            rtc_block[36] = state.rtc.latched_day_high;
            // Bytes 40-47: timestamp (unused for now, reserved for compatibility).
            data.extend_from_slice(&rtc_block);
            data
        }
        _ => ram.to_vec(),
    };

    if data.is_empty() {
        return;
    }

    if let Err(error) = fs::write(path, &data) {
        eprintln!("warning: failed to write save file {}: {error}", path.display());
    }
}
