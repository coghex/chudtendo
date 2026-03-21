use std::sync::mpsc::{Receiver, Sender, SyncSender, TryRecvError};
use std::thread;
use std::time::Instant;

use super::bus::Bus;
use super::component::{
    Command, ComponentReport, CpuInitState, CpuRegisters, CpuReport, HardwareMode, InterruptFlags,
    MasterClock, MemoryCommand, ReadResult, SharedCartridgeReadState, WriteResult,
};
use crate::input::JoypadState;

const FLAG_Z: u8 = 0x80;
const FLAG_N: u8 = 0x40;
const FLAG_H: u8 = 0x20;
const FLAG_C: u8 = 0x10;
const SERIAL_DATA_INDEX: usize = 0x01;
const SERIAL_CONTROL_INDEX: usize = 0x02;
const SERIAL_INTERRUPT_MASK: u8 = 0x08;
const SERIAL_TRANSFER_CYCLES: u16 = 4096;
const JOYPAD_INTERRUPT_MASK: u8 = 0x10;
const INSTRUCTIONS_PER_QUANTUM: usize = 4;
const CPU_REPORT_INTERVAL: u64 = 1024;
const ILLEGAL_OPCODES: [u8; 11] = [
    0xd3, 0xdb, 0xdd, 0xe3, 0xe4, 0xeb, 0xec, 0xed, 0xf4, 0xfc, 0xfd,
];

// Total T-cycle cost per opcode (taken path for conditional branches).
// Used to ensure the CPU cycle counter advances by at least this amount.
// Source: Pan Docs / Game Boy opcode timing tables.
#[rustfmt::skip]
const OPCODE_CYCLES: [u8; 256] = [
// Uses NOT-TAKEN cost for conditional branches. Taken-path corrections added inline.
//  0   1   2   3   4   5   6   7   8   9   A   B   C   D   E   F
    4, 12,  8,  8,  4,  4,  8,  4, 20,  8,  8,  8,  4,  4,  8,  4, // 0x
    4, 12,  8,  8,  4,  4,  8,  4, 12,  8,  8,  8,  4,  4,  8,  4, // 1x
    8, 12,  8,  8,  4,  4,  8,  4,  8,  8,  8,  8,  4,  4,  8,  4, // 2x: JR cc not-taken=8
    8, 12,  8,  8, 12, 12, 12,  4,  8,  8,  8,  8,  4,  4,  8,  4, // 3x: JR cc not-taken=8
    4,  4,  4,  4,  4,  4,  8,  4,  4,  4,  4,  4,  4,  4,  8,  4, // 4x
    4,  4,  4,  4,  4,  4,  8,  4,  4,  4,  4,  4,  4,  4,  8,  4, // 5x
    4,  4,  4,  4,  4,  4,  8,  4,  4,  4,  4,  4,  4,  4,  8,  4, // 6x
    8,  8,  8,  8,  8,  8,  4,  8,  4,  4,  4,  4,  4,  4,  8,  4, // 7x
    4,  4,  4,  4,  4,  4,  8,  4,  4,  4,  4,  4,  4,  4,  8,  4, // 8x
    4,  4,  4,  4,  4,  4,  8,  4,  4,  4,  4,  4,  4,  4,  8,  4, // 9x
    4,  4,  4,  4,  4,  4,  8,  4,  4,  4,  4,  4,  4,  4,  8,  4, // Ax
    4,  4,  4,  4,  4,  4,  8,  4,  4,  4,  4,  4,  4,  4,  8,  4, // Bx
    8, 12, 12, 16, 12, 16,  8, 16,  8, 16, 12,  4, 12, 24,  8, 16, // Cx: RET/JP/CALL cc not-taken
    8, 12, 12,  0, 12, 16,  8, 16,  8, 16, 12,  0, 12,  0,  8, 16, // Dx
   12, 12,  8,  0,  0, 16,  8, 16, 16,  4, 16,  0,  0,  0,  8, 16, // Ex
   12, 12,  8,  4,  0, 16,  8, 16, 12,  8, 16,  4,  0,  0,  8, 16, // Fx
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CoreState {
    Running,
    Halted,
    Stopped,
    Faulted { opcode: u8, prefixed: bool },
}

#[derive(Debug)]
pub struct CpuThread {
    bus: Bus,
    hardware_mode: HardwareMode,
    boot_target_mode: HardwareMode,
    registers: CpuRegisters,
    io_registers: [u8; super::component::IO_REGISTERS_LEN],
    hram: [u8; super::component::HRAM_LEN],
    clock: MasterClock,
    interrupt_flags: InterruptFlags,
    interrupt_enable: u8,
    interrupt_master_enable: bool,
    ime_enable_delay: u8,
    boot_rom_unmapped: bool,
    shared_cartridge: Option<SharedCartridgeReadState>,
    joypad: JoypadState,
    last_joypad: u8,
    double_speed: bool,
    speed_switch_armed: bool,
    serial_transfer_countdown: u16,
    serial_output: Option<SyncSender<u8>>,
    cycles: u64,
    last_step_pc: u16,
    state: CoreState,
    halt_bug: bool,
    steps: u64,
    shutdown: bool,
}

impl CpuThread {
    pub fn spawn(
        init_state: CpuInitState,
        bus: Bus,
        inbox: Receiver<Command>,
        reports: Sender<ComponentReport>,
        clock: MasterClock,
    ) -> thread::JoinHandle<()> {
        thread::Builder::new()
            .name("cpu".to_owned())
            .spawn(move || Self::from_init_state(init_state, bus).run(inbox, reports, clock))
            .expect("failed to spawn cpu thread")
    }

    fn run(
        mut self,
        inbox: Receiver<Command>,
        reports: Sender<ComponentReport>,
        clock: MasterClock,
    ) {
        self.clock = clock.clone();
        let start = Instant::now();

        while !self.shutdown {
            self.service_inbox(&inbox);
            if self.shutdown {
                break;
            }

            let elapsed_nanos = start.elapsed().as_nanos() as u64;
            let wall_target = elapsed_nanos
                .checked_mul(super::component::CPU_CLOCK_HZ)
                .map(|n| n / 1_000_000_000)
                .unwrap_or(u64::MAX);

            if self.cycles < wall_target {
                let cycles_before = self.cycles;
                for _ in 0..INSTRUCTIONS_PER_QUANTUM {
                    self.service_inbox(&inbox);
                    if self.shutdown || !self.step(&inbox) {
                        break;
                    }
                }
                // If halted/stopped, still advance the clock so other components progress.
                if self.cycles == cycles_before {
                    self.cycles += INSTRUCTIONS_PER_QUANTUM as u64 * 4;
                }
                clock.advance(self.cycles);
            } else {
                thread::yield_now();
            }

            if self.steps == INSTRUCTIONS_PER_QUANTUM as u64
                || self.steps % CPU_REPORT_INTERVAL == 0
            {
                let _ = reports.send(ComponentReport::Cpu(CpuReport {
                    steps: self.steps,
                    registers: self.registers,
                }));
            }
        }
    }

    fn step(&mut self, inbox: &Receiver<Command>) -> bool {
        let current_pc = self.registers.pc;
        let pending_interrupts = self.pending_interrupts();

        match self.state {
            CoreState::Faulted { .. } => return false,
            CoreState::Halted | CoreState::Stopped => {
                if pending_interrupts != 0 {
                    self.state = CoreState::Running;
                    if self.interrupt_master_enable {
                        self.service_interrupt(inbox, pending_interrupts);
                        self.steps = self.steps.wrapping_add(1);
                        return true;
                    }
                }
                return false;
            }
            CoreState::Running => {}
        }

        if self.interrupt_master_enable && pending_interrupts != 0 {
            self.service_interrupt(inbox, pending_interrupts);
            self.steps = self.steps.wrapping_add(1);
            return true;
        }

        let cycles_before = self.cycles;
        let opcode = self.fetch_byte(inbox);
        if opcode == 0xcb {
            let prefixed = self.fetch_byte(inbox);
            self.execute_cb_opcode(inbox, prefixed);
            // CB ops: 2 M-cycles for reg, 4 for (HL). Bus ops already correct.
        } else {
            self.execute_opcode(inbox, opcode);
        }
        // Ensure at least the correct total M-cycle cost. If bus ops
        // already counted more (e.g., due to blocking), keep the higher value.
        let minimum = cycles_before + OPCODE_CYCLES[opcode as usize] as u64;
        if self.cycles < minimum {
            self.cycles = minimum;
            self.clock.advance(self.cycles);
        }

        self.steps = self.steps.wrapping_add(1);
        self.advance_ime_delay();
        self.advance_serial_transfer();
        self.poll_joypad_interrupt();
        self.last_step_pc = current_pc;
        true
    }

    fn execute_opcode(&mut self, inbox: &Receiver<Command>, opcode: u8) {
        if ILLEGAL_OPCODES.contains(&opcode) {
            self.state = CoreState::Faulted {
                opcode,
                prefixed: false,
            };
            return;
        }

        match opcode {
            0x00 => {}
            0x01 | 0x11 | 0x21 | 0x31 => {
                let value = self.fetch_word(inbox);
                self.set_reg16((opcode >> 4) & 0x03, value);
            }
            0x02 => {
                self.write_byte(inbox, self.registers.bc, self.a());
            }
            0x03 | 0x13 | 0x23 | 0x33 => {
                let register = (opcode >> 4) & 0x03;
                let value = self.reg16(register).wrapping_add(1);
                self.set_reg16(register, value);
            }
            0x04 | 0x0c | 0x14 | 0x1c | 0x24 | 0x2c | 0x34 | 0x3c => {
                let register = (opcode >> 3) & 0x07;
                let value = self.read_r8(inbox, register);
                let result = value.wrapping_add(1);
                self.write_r8(inbox, register, result);
                self.set_inc_flags(result, value);
            }
            0x05 | 0x0d | 0x15 | 0x1d | 0x25 | 0x2d | 0x35 | 0x3d => {
                let register = (opcode >> 3) & 0x07;
                let value = self.read_r8(inbox, register);
                let result = value.wrapping_sub(1);
                self.write_r8(inbox, register, result);
                self.set_dec_flags(result, value);
            }
            0x06 | 0x0e | 0x16 | 0x1e | 0x26 | 0x2e | 0x36 | 0x3e => {
                let value = self.fetch_byte(inbox);
                self.write_r8(inbox, (opcode >> 3) & 0x07, value);
            }
            0x07 => self.rotate_a_left(false),
            0x08 => {
                let address = self.fetch_word(inbox);
                self.write_word(inbox, address, self.registers.sp);
            }
            0x09 | 0x19 | 0x29 | 0x39 => {
                let value = self.reg16((opcode >> 4) & 0x03);
                let hl = self.registers.hl;
                let result = hl.wrapping_add(value);
                self.set_flag(FLAG_N, false);
                self.set_flag(FLAG_H, ((hl & 0x0fff) + (value & 0x0fff)) > 0x0fff);
                self.set_flag(FLAG_C, (hl as u32 + value as u32) > 0xffff);
                self.registers.hl = result;
            }
            0x0a => {
                let value = self.read_byte(inbox, self.registers.bc);
                self.set_a(value);
            }
            0x0b | 0x1b | 0x2b | 0x3b => {
                let register = (opcode >> 4) & 0x03;
                let value = self.reg16(register).wrapping_sub(1);
                self.set_reg16(register, value);
            }
            0x0f => self.rotate_a_right(false),
            0x10 => {
                let _ = self.fetch_byte(inbox);
                if self.speed_switch_armed
                    && !matches!(self.hardware_mode, HardwareMode::DmgCompatibility)
                {
                    self.double_speed = !self.double_speed;
                    self.speed_switch_armed = false;
                } else {
                    self.state = CoreState::Stopped;
                }
            }
            0x12 => self.write_byte(inbox, self.registers.de, self.a()),
            0x17 => self.rotate_a_left(true),
            0x18 => {
                let offset = self.fetch_byte(inbox) as i8;
                self.registers.pc = self.registers.pc.wrapping_add_signed(offset as i16);
            }
            0x1a => {
                let value = self.read_byte(inbox, self.registers.de);
                self.set_a(value);
            }
            0x1f => self.rotate_a_right(true),
            0x20 | 0x28 | 0x30 | 0x38 => {
                let offset = self.fetch_byte(inbox) as i8;
                if self.condition((opcode >> 3) & 0x03) {
                    self.registers.pc = self.registers.pc.wrapping_add_signed(offset as i16);
                    self.cycles += 4;
                }
            }
            0x22 => {
                let address = self.registers.hl;
                self.write_byte(inbox, address, self.a());
                self.registers.hl = self.registers.hl.wrapping_add(1);
            }
            0x27 => self.decimal_adjust_accumulator(),
            0x2a => {
                let address = self.registers.hl;
                let value = self.read_byte(inbox, address);
                self.registers.hl = self.registers.hl.wrapping_add(1);
                self.set_a(value);
            }
            0x2f => {
                self.set_a(!self.a());
                self.set_flag(FLAG_N, true);
                self.set_flag(FLAG_H, true);
            }
            0x32 => {
                let address = self.registers.hl;
                self.write_byte(inbox, address, self.a());
                self.registers.hl = self.registers.hl.wrapping_sub(1);
            }
            0x37 => {
                let zero = self.flag(FLAG_Z);
                self.set_flags(zero, false, false, true);
            }
            0x3a => {
                let address = self.registers.hl;
                let value = self.read_byte(inbox, address);
                self.registers.hl = self.registers.hl.wrapping_sub(1);
                self.set_a(value);
            }
            0x3f => {
                let zero = self.flag(FLAG_Z);
                let carry = !self.flag(FLAG_C);
                self.set_flags(zero, false, false, carry);
            }
            0x40..=0x7f => {
                if opcode == 0x76 {
                    if !self.interrupt_master_enable && self.pending_interrupts() != 0 {
                        self.halt_bug = true;
                    } else {
                        self.state = CoreState::Halted;
                    }
                } else {
                    let source = opcode & 0x07;
                    let target = (opcode >> 3) & 0x07;
                    let value = self.read_r8(inbox, source);
                    self.write_r8(inbox, target, value);
                }
            }
            0x80..=0x87 => {
                let value = self.read_r8(inbox, opcode & 0x07);
                self.add_to_a(value, false);
            }
            0x88..=0x8f => {
                let value = self.read_r8(inbox, opcode & 0x07);
                self.add_to_a(value, true);
            }
            0x90..=0x97 => {
                let value = self.read_r8(inbox, opcode & 0x07);
                self.sub_from_a(value, false);
            }
            0x98..=0x9f => {
                let value = self.read_r8(inbox, opcode & 0x07);
                self.sub_from_a(value, true);
            }
            0xa0..=0xa7 => {
                let value = self.read_r8(inbox, opcode & 0x07);
                self.logic_and(value);
            }
            0xa8..=0xaf => {
                let value = self.read_r8(inbox, opcode & 0x07);
                self.logic_xor(value);
            }
            0xb0..=0xb7 => {
                let value = self.read_r8(inbox, opcode & 0x07);
                self.logic_or(value);
            }
            0xb8..=0xbf => {
                let value = self.read_r8(inbox, opcode & 0x07);
                self.compare_a(value);
            }
            0xc0 | 0xc8 | 0xd0 | 0xd8 => {
                if self.condition((opcode >> 3) & 0x03) {
                    self.registers.pc = self.pop_word(inbox);
                    self.cycles += 8; // taken: 2 internal M-cycles
                }
            }
            0xc1 | 0xd1 | 0xe1 | 0xf1 => {
                let value = self.pop_word(inbox);
                self.set_stack_reg((opcode >> 4) & 0x03, value);
            }
            0xc2 | 0xca | 0xd2 | 0xda => {
                let address = self.fetch_word(inbox);
                if self.condition((opcode >> 3) & 0x03) {
                    self.registers.pc = address;
                    self.cycles += 4; // taken: internal jump cycle
                }
            }
            0xc3 => self.registers.pc = self.fetch_word(inbox),
            0xc4 | 0xcc | 0xd4 | 0xdc => {
                let address = self.fetch_word(inbox);
                if self.condition((opcode >> 3) & 0x03) {
                    self.push_word(inbox, self.registers.pc);
                    self.registers.pc = address;
                    self.cycles += 4; // taken: internal cycle before push
                }
            }
            0xc5 | 0xd5 | 0xe5 | 0xf5 => {
                let value = self.stack_reg((opcode >> 4) & 0x03);
                self.push_word(inbox, value);
            }
            0xc6 | 0xce | 0xd6 | 0xde | 0xe6 | 0xee | 0xf6 | 0xfe => {
                let value = self.fetch_byte(inbox);
                match (opcode >> 3) & 0x07 {
                    0x00 => self.add_to_a(value, false),
                    0x01 => self.add_to_a(value, true),
                    0x02 => self.sub_from_a(value, false),
                    0x03 => self.sub_from_a(value, true),
                    0x04 => self.logic_and(value),
                    0x05 => self.logic_xor(value),
                    0x06 => self.logic_or(value),
                    0x07 => self.compare_a(value),
                    _ => unreachable!(),
                }
            }
            0xc7 | 0xcf | 0xd7 | 0xdf | 0xe7 | 0xef | 0xf7 | 0xff => {
                self.push_word(inbox, self.registers.pc);
                self.registers.pc = (opcode as u16) & 0x0038;
            }
            0xc9 => self.registers.pc = self.pop_word(inbox),
            0xcd => {
                let address = self.fetch_word(inbox);
                self.push_word(inbox, self.registers.pc);
                self.registers.pc = address;
            }
            0xd9 => {
                self.registers.pc = self.pop_word(inbox);
                self.interrupt_master_enable = true;
                self.ime_enable_delay = 0;
            }
            0xe0 => {
                let offset = self.fetch_byte(inbox) as u16;
                self.write_byte(inbox, 0xff00 | offset, self.a());
            }
            0xe2 => {
                let address = 0xff00 | u16::from(self.c());
                self.write_byte(inbox, address, self.a());
            }
            0xe8 => {
                let offset = self.fetch_byte(inbox) as i8;
                let (result, half_carry, carry) = add_signed_offset(self.registers.sp, offset);
                self.registers.sp = result;
                self.set_flags(false, false, half_carry, carry);
            }
            0xe9 => self.registers.pc = self.registers.hl,
            0xea => {
                let address = self.fetch_word(inbox);
                self.write_byte(inbox, address, self.a());
            }
            0xf0 => {
                let offset = self.fetch_byte(inbox) as u16;
                let value = self.read_byte(inbox, 0xff00 | offset);
                self.set_a(value);
            }
            0xf2 => {
                let address = 0xff00 | u16::from(self.c());
                let value = self.read_byte(inbox, address);
                self.set_a(value);
            }
            0xf3 => {
                self.interrupt_master_enable = false;
                self.ime_enable_delay = 0;
            }
            0xf8 => {
                let offset = self.fetch_byte(inbox) as i8;
                let (result, half_carry, carry) = add_signed_offset(self.registers.sp, offset);
                self.registers.hl = result;
                self.set_flags(false, false, half_carry, carry);
            }
            0xf9 => self.registers.sp = self.registers.hl,
            0xfa => {
                let address = self.fetch_word(inbox);
                let value = self.read_byte(inbox, address);
                self.set_a(value);
            }
            0xfb => self.ime_enable_delay = 2,
            _ => {
                self.state = CoreState::Faulted {
                    opcode,
                    prefixed: false,
                };
            }
        }
    }

    fn execute_cb_opcode(&mut self, inbox: &Receiver<Command>, opcode: u8) {
        let register = opcode & 0x07;
        let value = self.read_r8(inbox, register);

        match opcode >> 6 {
            0 => {
                let (result, carry) = match (opcode >> 3) & 0x07 {
                    0 => (value.rotate_left(1), value & 0x80 != 0),
                    1 => (value.rotate_right(1), value & 0x01 != 0),
                    2 => {
                        let carry_in = u8::from(self.flag(FLAG_C));
                        ((value << 1) | carry_in, value & 0x80 != 0)
                    }
                    3 => {
                        let carry_in = if self.flag(FLAG_C) { 0x80 } else { 0x00 };
                        ((value >> 1) | carry_in, value & 0x01 != 0)
                    }
                    4 => (value << 1, value & 0x80 != 0),
                    5 => (((value >> 1) | (value & 0x80)), value & 0x01 != 0),
                    6 => ((value << 4) | (value >> 4), false),
                    7 => (value >> 1, value & 0x01 != 0),
                    _ => unreachable!(),
                };

                self.write_r8(inbox, register, result);
                self.set_flags(result == 0, false, false, carry);
            }
            1 => {
                let bit = (opcode >> 3) & 0x07;
                self.set_flag(FLAG_Z, value & (1 << bit) == 0);
                self.set_flag(FLAG_N, false);
                self.set_flag(FLAG_H, true);
            }
            2 => {
                let bit = (opcode >> 3) & 0x07;
                self.write_r8(inbox, register, value & !(1 << bit));
            }
            3 => {
                let bit = (opcode >> 3) & 0x07;
                self.write_r8(inbox, register, value | (1 << bit));
            }
            _ => unreachable!(),
        }
    }

    fn service_interrupt(&mut self, inbox: &Receiver<Command>, pending_interrupts: u8) {
        let interrupt_bit = pending_interrupts.trailing_zeros() as u8;
        let interrupt_mask = 1u8 << interrupt_bit;
        let handler = match interrupt_bit {
            0 => 0x0040,
            1 => 0x0048,
            2 => 0x0050,
            3 => 0x0058,
            4 => 0x0060,
            _ => return,
        };

        self.interrupt_master_enable = false;
        self.ime_enable_delay = 0;
        self.state = CoreState::Running;
        self.interrupt_flags.clear(interrupt_mask);
        self.push_word(inbox, self.registers.pc);
        self.registers.pc = handler;
    }

    fn handle_memory(&mut self, command: MemoryCommand) {
        match command {
            MemoryCommand::Read {
                address,
                respond_to,
            } => {
                let result = self
                    .read_cpu_owned(address)
                    .map(ReadResult::Ready)
                    .unwrap_or(ReadResult::NoData);
                let _ = respond_to.send(result);
            }
            MemoryCommand::Write {
                address,
                value,
                respond_to,
            } => {
                let result = if self.write_cpu_owned(address, value) {
                    WriteResult::Accepted
                } else {
                    WriteResult::NoData
                };
                if let Some(respond_to) = respond_to {
                    let _ = respond_to.send(result);
                }
            }
        }
    }

    fn service_inbox(&mut self, inbox: &Receiver<Command>) {
        loop {
            match inbox.try_recv() {
                Ok(Command::Memory(command)) => self.handle_memory(command),
                Ok(Command::SetHardwareMode(hardware_mode)) => self.hardware_mode = hardware_mode,
                Ok(Command::Stop) => {
                    self.shutdown = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.shutdown = true;
                    break;
                }
            }
        }
    }

    fn read_byte(&mut self, inbox: &Receiver<Command>, address: u16) -> u8 {
        if let Some(value) = self.read_cpu_owned(address) {
            self.cycles += 4;
            self.clock.advance(self.cycles);
            return value;
        }

        if let Some(shared_cartridge) = &self.shared_cartridge {
            if let Some(value) = shared_cartridge.read(address) {
                self.cycles += 4;
                self.clock.advance(self.cycles);
                return value;
            }
        }

        let mut pending = self.bus.read(address);
        loop {
            self.service_inbox(inbox);
            if self.shutdown {
                self.cycles += 4;
                return 0xff;
            }
            if let Some(result) = pending.try_take() {
                self.cycles += 4;
                self.clock.advance(self.cycles);
                return match result {
                    ReadResult::Ready(value) => value,
                    ReadResult::NoData => 0xff,
                };
            }
            thread::yield_now();
        }
    }

    fn write_byte(&mut self, inbox: &Receiver<Command>, address: u16, value: u8) {
        if self.write_cpu_owned(address, value) {
            return;
        }

        tracing::trace!(pc = format_args!("{:04x}", self.registers.pc), addr = format_args!("{:04x}", address), value = format_args!("{:02x}", value), "cpu write");

        self.write_byte_sync(inbox, address, value);
    }

    fn write_byte_sync(&mut self, inbox: &Receiver<Command>, address: u16, value: u8) {
        if self.write_cpu_owned(address, value) {
            self.cycles += 4;
            self.clock.advance(self.cycles);
            return;
        }

        let mut pending = self.bus.write(address, value);

        loop {
            self.service_inbox(inbox);
            if self.shutdown {
                self.cycles += 4;
                return;
            }
            if let Some(result) = pending.try_take() {
                self.cycles += 4;
                self.clock.advance(self.cycles);
                if address == 0xff50 && matches!(result, WriteResult::Accepted) && value != 0x00 {
                    self.finish_boot_transition();
                }
                return;
            }
            thread::yield_now();
        }
    }

    fn write_word(&mut self, inbox: &Receiver<Command>, address: u16, value: u16) {
        let [low, high] = value.to_le_bytes();
        self.write_byte(inbox, address, low);
        self.write_byte(inbox, address.wrapping_add(1), high);
    }

    fn fetch_byte(&mut self, inbox: &Receiver<Command>) -> u8 {
        let value = self.read_byte(inbox, self.registers.pc);
        if self.halt_bug {
            self.halt_bug = false;
        } else {
            self.registers.pc = self.registers.pc.wrapping_add(1);
        }
        value
    }

    fn fetch_word(&mut self, inbox: &Receiver<Command>) -> u16 {
        let low = self.fetch_byte(inbox);
        let high = self.fetch_byte(inbox);
        u16::from_le_bytes([low, high])
    }

    fn push_word(&mut self, inbox: &Receiver<Command>, value: u16) {
        let [low, high] = value.to_le_bytes();
        self.registers.sp = self.registers.sp.wrapping_sub(1);
        self.write_byte_sync(inbox, self.registers.sp, high);
        self.registers.sp = self.registers.sp.wrapping_sub(1);
        self.write_byte_sync(inbox, self.registers.sp, low);
    }

    fn pop_word(&mut self, inbox: &Receiver<Command>) -> u16 {
        let low = self.read_byte(inbox, self.registers.sp);
        self.registers.sp = self.registers.sp.wrapping_add(1);
        let high = self.read_byte(inbox, self.registers.sp);
        self.registers.sp = self.registers.sp.wrapping_add(1);
        u16::from_le_bytes([low, high])
    }

    fn read_r8(&mut self, inbox: &Receiver<Command>, register: u8) -> u8 {
        match register {
            0 => self.b(),
            1 => self.c(),
            2 => self.d(),
            3 => self.e(),
            4 => self.h(),
            5 => self.l(),
            6 => self.read_byte(inbox, self.registers.hl),
            7 => self.a(),
            _ => unreachable!(),
        }
    }

    fn write_r8(&mut self, inbox: &Receiver<Command>, register: u8, value: u8) {
        match register {
            0 => self.set_b(value),
            1 => self.set_c(value),
            2 => self.set_d(value),
            3 => self.set_e(value),
            4 => self.set_h(value),
            5 => self.set_l(value),
            6 => self.write_byte(inbox, self.registers.hl, value),
            7 => self.set_a(value),
            _ => unreachable!(),
        }
    }

    fn reg16(&self, register: u8) -> u16 {
        match register {
            0 => self.registers.bc,
            1 => self.registers.de,
            2 => self.registers.hl,
            3 => self.registers.sp,
            _ => unreachable!(),
        }
    }

    fn set_reg16(&mut self, register: u8, value: u16) {
        match register {
            0 => self.registers.bc = value,
            1 => self.registers.de = value,
            2 => self.registers.hl = value,
            3 => self.registers.sp = value,
            _ => unreachable!(),
        }
    }

    fn stack_reg(&self, register: u8) -> u16 {
        match register {
            0 => self.registers.bc,
            1 => self.registers.de,
            2 => self.registers.hl,
            3 => self.registers.af & 0xfff0,
            _ => unreachable!(),
        }
    }

    fn set_stack_reg(&mut self, register: u8, value: u16) {
        match register {
            0 => self.registers.bc = value,
            1 => self.registers.de = value,
            2 => self.registers.hl = value,
            3 => self.registers.af = value & 0xfff0,
            _ => unreachable!(),
        }
    }

    fn add_to_a(&mut self, value: u8, include_carry: bool) {
        let a = self.a();
        let carry = u8::from(include_carry && self.flag(FLAG_C));
        let result = a.wrapping_add(value).wrapping_add(carry);
        self.set_a(result);
        self.set_flags(
            result == 0,
            false,
            ((a & 0x0f) + (value & 0x0f) + carry) > 0x0f,
            u16::from(a) + u16::from(value) + u16::from(carry) > 0x00ff,
        );
    }

    fn sub_from_a(&mut self, value: u8, include_carry: bool) {
        let a = self.a();
        let carry = u8::from(include_carry && self.flag(FLAG_C));
        let result = a.wrapping_sub(value).wrapping_sub(carry);
        self.set_a(result);
        self.set_flags(
            result == 0,
            true,
            (a & 0x0f) < ((value & 0x0f) + carry),
            u16::from(a) < (u16::from(value) + u16::from(carry)),
        );
    }

    fn logic_and(&mut self, value: u8) {
        let result = self.a() & value;
        self.set_a(result);
        self.set_flags(result == 0, false, true, false);
    }

    fn logic_xor(&mut self, value: u8) {
        let result = self.a() ^ value;
        self.set_a(result);
        self.set_flags(result == 0, false, false, false);
    }

    fn logic_or(&mut self, value: u8) {
        let result = self.a() | value;
        self.set_a(result);
        self.set_flags(result == 0, false, false, false);
    }

    fn compare_a(&mut self, value: u8) {
        let a = self.a();
        let result = a.wrapping_sub(value);
        self.set_flags(result == 0, true, (a & 0x0f) < (value & 0x0f), a < value);
    }

    fn rotate_a_left(&mut self, through_carry: bool) {
        let value = self.a();
        let carry_in = u8::from(through_carry && self.flag(FLAG_C));
        let carry_out = value & 0x80 != 0;
        let result = if through_carry {
            (value << 1) | carry_in
        } else {
            value.rotate_left(1)
        };
        self.set_a(result);
        self.set_flags(false, false, false, carry_out);
    }

    fn rotate_a_right(&mut self, through_carry: bool) {
        let value = self.a();
        let carry_in = if through_carry && self.flag(FLAG_C) {
            0x80
        } else {
            0x00
        };
        let carry_out = value & 0x01 != 0;
        let result = if through_carry {
            (value >> 1) | carry_in
        } else {
            value.rotate_right(1)
        };
        self.set_a(result);
        self.set_flags(false, false, false, carry_out);
    }

    fn decimal_adjust_accumulator(&mut self) {
        let mut a = self.a();
        let mut adjust = 0;
        let mut carry = self.flag(FLAG_C);

        if self.flag(FLAG_H) || (!self.flag(FLAG_N) && (a & 0x0f) > 0x09) {
            adjust |= 0x06;
        }
        if carry || (!self.flag(FLAG_N) && a > 0x99) {
            adjust |= 0x60;
            carry = true;
        }

        a = if self.flag(FLAG_N) {
            a.wrapping_sub(adjust)
        } else {
            a.wrapping_add(adjust)
        };

        self.set_a(a);
        self.set_flag(FLAG_Z, a == 0);
        self.set_flag(FLAG_H, false);
        self.set_flag(FLAG_C, carry);
    }

    fn condition(&self, condition: u8) -> bool {
        match condition {
            0 => !self.flag(FLAG_Z),
            1 => self.flag(FLAG_Z),
            2 => !self.flag(FLAG_C),
            3 => self.flag(FLAG_C),
            _ => unreachable!(),
        }
    }

    fn pending_interrupts(&self) -> u8 {
        self.interrupt_enable & self.interrupt_flags.load() & 0x1f
    }

    fn advance_ime_delay(&mut self) {
        if self.ime_enable_delay == 0 {
            return;
        }

        self.ime_enable_delay -= 1;
        if self.ime_enable_delay == 0 {
            self.interrupt_master_enable = true;
        }
    }

    fn advance_serial_transfer(&mut self) {
        if self.serial_transfer_countdown == 0 {
            return;
        }

        self.serial_transfer_countdown -= 1;
        if self.serial_transfer_countdown == 0 {
            // Transfer complete: no cable connected, so 0xFF shifted in.
            self.io_registers[SERIAL_DATA_INDEX] = 0xff;
            self.io_registers[SERIAL_CONTROL_INDEX] &= 0x7f;
            self.interrupt_flags.set(SERIAL_INTERRUPT_MASK);
        }
    }

    fn poll_joypad_interrupt(&mut self) {
        let select = self.io_registers[0x00];
        let current = self.joypad.read(select) & 0x0f;
        let previous = self.last_joypad;
        self.last_joypad = current;

        // Interrupt fires on any bit going from 1 (unpressed) to 0 (pressed).
        let falling_edges = previous & !current;
        if falling_edges != 0 {
            self.interrupt_flags.set(JOYPAD_INTERRUPT_MASK);
        }
    }

    fn finish_boot_transition(&mut self) {
        if self.boot_rom_unmapped {
            return;
        }

        self.boot_rom_unmapped = true;
        if let Some(shared_cartridge) = &self.shared_cartridge {
            shared_cartridge.set_boot_rom_mapped(false);
        }
        self.hardware_mode = self.boot_target_mode;
        self.bus.propagate_hardware_mode(self.boot_target_mode);
    }

    fn read_cpu_owned(&self, address: u16) -> Option<u8> {
        match address {
            0xff4c | 0xff4d | 0xff56 | 0xff6c
                if matches!(self.hardware_mode, HardwareMode::DmgCompatibility) =>
            {
                Some(0xff)
            }
            0xff00 => Some(if self.boot_rom_unmapped {
                self.joypad.read(self.io_registers[0x00])
            } else {
                self.io_registers[0x00]
            }),
            0xff4d => {
                let speed_bit = if self.double_speed { 0x80 } else { 0x00 };
                let armed_bit = if self.speed_switch_armed { 0x01 } else { 0x00 };
                Some(0x7e | speed_bit | armed_bit)
            }
            0xff01 | 0xff02 => {
                Some(self.io_registers[(address - 0xff00) as usize])
            }
            0xff0f => Some(0xe0 | self.interrupt_flags.load()),
            0xff03 | 0xff08..=0xff0e | 0xff27..=0xff2f | 0xff4e
            | 0xff57..=0xff67 | 0xff6c..=0xff6f | 0xff71..=0xff75
            | 0xff78..=0xff7f => Some(0xff),
            0xff4c | 0xff56 | 0xff76 | 0xff77 => {
                Some(self.io_registers[(address - 0xff00) as usize])
            }
            0xff80..=0xfffe => Some(self.hram[(address - 0xff80) as usize]),
            0xffff => Some(self.interrupt_enable),
            _ => None,
        }
    }

    fn write_cpu_owned(&mut self, address: u16, value: u8) -> bool {
        match address {
            0xff4c | 0xff4d | 0xff56 | 0xff6c
                if matches!(self.hardware_mode, HardwareMode::DmgCompatibility) =>
            {
                true
            }
            0xff00 => {
                self.io_registers[0x00] = if self.boot_rom_unmapped {
                    joypad_select(value)
                } else {
                    value
                };
                true
            }
            0xff02 => {
                self.io_registers[SERIAL_CONTROL_INDEX] = value;
                if value & 0x80 != 0 {
                    // Capture the transmitted byte before the transfer overwrites SB.
                    if let Some(sender) = &self.serial_output {
                        let _ = sender.try_send(self.io_registers[SERIAL_DATA_INDEX]);
                    }
                    // Transfer requested. With no link cable connected, both
                    // internal and external clock complete with 0xFF received.
                    // External clock uses a longer delay (simulates timeout).
                    self.serial_transfer_countdown = if value & 0x01 != 0 {
                        SERIAL_TRANSFER_CYCLES
                    } else {
                        SERIAL_TRANSFER_CYCLES * 8
                    };
                }
                true
            }
            0xff4d => {
                self.speed_switch_armed = value & 0x01 != 0;
                true
            }
            0xff0f => {
                self.interrupt_flags.store(value);
                true
            }
            0xff01 | 0xff03
            | 0xff08..=0xff0e
            | 0xff27..=0xff2f
            | 0xff4c | 0xff4e
            | 0xff56..=0xff67
            | 0xff6c..=0xff6f
            | 0xff71..=0xff7f => {
                self.io_registers[(address - 0xff00) as usize] = value;
                true
            }
            0xff80..=0xfffe => {
                self.hram[(address - 0xff80) as usize] = value;
                true
            }
            0xffff => {
                self.interrupt_enable = value;
                true
            }
            _ => false,
        }
    }

    fn a(&self) -> u8 {
        (self.registers.af >> 8) as u8
    }

    fn b(&self) -> u8 {
        (self.registers.bc >> 8) as u8
    }

    fn c(&self) -> u8 {
        self.registers.bc as u8
    }

    fn d(&self) -> u8 {
        (self.registers.de >> 8) as u8
    }

    fn e(&self) -> u8 {
        self.registers.de as u8
    }

    fn h(&self) -> u8 {
        (self.registers.hl >> 8) as u8
    }

    fn l(&self) -> u8 {
        self.registers.hl as u8
    }

    fn flags(&self) -> u8 {
        self.registers.af as u8 & 0xf0
    }

    fn flag(&self, mask: u8) -> bool {
        self.flags() & mask != 0
    }

    fn set_a(&mut self, value: u8) {
        self.registers.af = (u16::from(value) << 8) | u16::from(self.flags());
    }

    fn set_b(&mut self, value: u8) {
        self.registers.bc = (u16::from(value) << 8) | u16::from(self.c());
    }

    fn set_c(&mut self, value: u8) {
        self.registers.bc = (u16::from(self.b()) << 8) | u16::from(value);
    }

    fn set_d(&mut self, value: u8) {
        self.registers.de = (u16::from(value) << 8) | u16::from(self.e());
    }

    fn set_e(&mut self, value: u8) {
        self.registers.de = (u16::from(self.d()) << 8) | u16::from(value);
    }

    fn set_h(&mut self, value: u8) {
        self.registers.hl = (u16::from(value) << 8) | u16::from(self.l());
    }

    fn set_l(&mut self, value: u8) {
        self.registers.hl = (u16::from(self.h()) << 8) | u16::from(value);
    }

    fn set_flags(&mut self, zero: bool, subtract: bool, half_carry: bool, carry: bool) {
        let mut flags = 0u8;
        if zero {
            flags |= FLAG_Z;
        }
        if subtract {
            flags |= FLAG_N;
        }
        if half_carry {
            flags |= FLAG_H;
        }
        if carry {
            flags |= FLAG_C;
        }
        self.registers.af = (u16::from(self.a()) << 8) | u16::from(flags);
    }

    fn set_flag(&mut self, mask: u8, enabled: bool) {
        let mut flags = self.flags();
        if enabled {
            flags |= mask;
        } else {
            flags &= !mask;
        }
        self.registers.af = (u16::from(self.a()) << 8) | u16::from(flags);
    }

    fn set_inc_flags(&mut self, result: u8, original: u8) {
        self.set_flag(FLAG_Z, result == 0);
        self.set_flag(FLAG_N, false);
        self.set_flag(FLAG_H, (original & 0x0f) == 0x0f);
    }

    fn set_dec_flags(&mut self, result: u8, original: u8) {
        self.set_flag(FLAG_Z, result == 0);
        self.set_flag(FLAG_N, true);
        self.set_flag(FLAG_H, (original & 0x0f) == 0x00);
    }

    fn from_init_state(init_state: CpuInitState, bus: Bus) -> Self {
        Self {
            bus,
            hardware_mode: init_state.hardware_mode,
            boot_target_mode: init_state.boot_target_mode,
            registers: init_state.registers,
            io_registers: init_state.io_registers,
            hram: init_state.hram,
            clock: MasterClock::new(),
            interrupt_flags: init_state.interrupt_flags,
            interrupt_enable: init_state.interrupt_enable,
            interrupt_master_enable: false,
            ime_enable_delay: 0,
            boot_rom_unmapped: false,
            shared_cartridge: init_state.shared_cartridge,
            joypad: init_state.joypad,
            last_joypad: 0x0f,
            double_speed: false,
            speed_switch_armed: false,
            serial_transfer_countdown: 0,
            serial_output: init_state.serial_output,
            cycles: 0,
            last_step_pc: init_state.registers.pc,
            state: CoreState::Running,
            halt_bug: false,
            steps: 0,
            shutdown: false,
        }
    }

}

impl Default for CpuThread {
    fn default() -> Self {
        let (cpu_sender, _cpu_receiver) = std::sync::mpsc::channel();
        let (ppu_sender, _ppu_receiver) = std::sync::mpsc::channel();
        let (wram_sender, _wram_receiver) = std::sync::mpsc::channel();
        let (cartridge_sender, _cartridge_receiver) = std::sync::mpsc::channel();
        let (timer_sender, _timer_receiver) = std::sync::mpsc::channel();
        let (apu_sender, _apu_receiver) = std::sync::mpsc::channel();
        Self::from_init_state(
            CpuInitState::default(),
            Bus::new(
                cpu_sender,
                ppu_sender,
                wram_sender,
                cartridge_sender,
                timer_sender,
                apu_sender,
            ),
        )
    }
}

fn add_signed_offset(base: u16, offset: i8) -> (u16, bool, bool) {
    let offset = offset as i16 as u16;
    let result = base.wrapping_add(offset);
    let half_carry = ((base & 0x000f) + (offset & 0x000f)) > 0x000f;
    let carry = ((base & 0x00ff) + (offset & 0x00ff)) > 0x00ff;
    (result, half_carry, carry)
}

fn joypad_select(value: u8) -> u8 {
    value & 0x30
}



