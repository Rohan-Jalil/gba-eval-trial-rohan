//! ARM7TDMI core: register file, modes, CPSR/SPSR, exceptions, step()
//! (gbatek: ARM CPU Register Set, ARM CPU Flags & Condition Field, ARM CPU Exceptions).
//!
//! # PC convention (contract for arm.rs and thumb.rs)
//! - Between instructions, `registers[15]` holds the address of the next instruction to execute.
//! - While an instruction executes, `registers[15]` reads as its address + 8 (ARM) or + 4 (THUMB),
//!   the pipelined value gbatek describes. Cases needing more (ARM shift-by-register reads PC+12)
//!   add the extra offset themselves.
//! - Decoders never write `registers[15]` directly. They call `branch_to` (or `write_register(15, _)`),
//!   which aligns the target for the current state and flushes the pipeline. If neither is called,
//!   `step()` advances to the next sequential instruction.
//! - To switch state (BX), set or clear `CPSR_THUMB` with `set_thumb_state` before calling `branch_to`.

#[allow(dead_code)] // shifter and add/sub helpers are unused until arm.rs/thumb.rs call them; remove then
mod alu;
mod arm;
mod thumb;

use crate::bus::{Bus, BIOS_SIZE};
use crate::ppu::{CYCLES_PER_LINE, HBLANK_START_CYCLE};
pub use alu::{FLAG_CARRY, FLAG_NEGATIVE, FLAG_OVERFLOW, FLAG_ZERO};

/// CPSR mode bits 0-4 (gbatek: ARM CPU Register Set).
pub const MODE_USER: u32 = 0x10;
pub const MODE_FIQ: u32 = 0x11;
pub const MODE_IRQ: u32 = 0x12;
pub const MODE_SUPERVISOR: u32 = 0x13;
pub const MODE_ABORT: u32 = 0x17;
pub const MODE_UNDEFINED: u32 = 0x1B;
pub const MODE_SYSTEM: u32 = 0x1F;
pub const MODE_MASK: u32 = 0x1F; // M4-M0

pub const CPSR_THUMB: u32 = 1 << 5; // T: 0 = ARM, 1 = THUMB state
pub const CPSR_FIQ_DISABLE: u32 = 1 << 6; // F
pub const CPSR_IRQ_DISABLE: u32 = 1 << 7; // I

pub const PROGRAM_COUNTER: usize = 15; // R15
pub const LINK_REGISTER: usize = 14; // R14
pub const STACK_POINTER: usize = 13; // R13

/// Exceptions the GBA can raise (gbatek: ARM CPU Exceptions). Aborts and FIQ never occur on the GBA.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Exception {
    Reset,
    UndefinedInstruction,
    SoftwareInterrupt,
    Interrupt, // IRQ
}

/// Register bank selected by the mode bits: User/System share one, each exception mode has its own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Bank {
    User = 0,
    Fiq = 1,
    Supervisor = 2,
    Abort = 3,
    Irq = 4,
    Undefined = 5,
}

/// Map mode bits to their bank. Invalid mode values fall back to the User bank (gbatek says
/// they are "not allowed" and does not define them).
fn bank_of(mode: u32) -> Bank {
    match mode & MODE_MASK {
        MODE_FIQ => Bank::Fiq,
        MODE_IRQ => Bank::Irq,
        MODE_SUPERVISOR => Bank::Supervisor,
        MODE_ABORT => Bank::Abort,
        MODE_UNDEFINED => Bank::Undefined,
        _ => Bank::User,
    }
}

/// MSR field mask bits of the opcode (gbatek: ARM Opcodes: PSR Transfer (MRS, MSR)). Only the
/// flags and control fields exist on the ARM7TDMI; s (bit 18) and x (bit 17) select reserved
/// bits 8-23, which MSR never changes.
pub const MSR_FIELD_CONTROL: u32 = 1 << 16; // c: bits 0-7
pub const MSR_FIELD_EXTENSION: u32 = 1 << 17; // x: reserved bits 8-15, ignored
pub const MSR_FIELD_STATUS: u32 = 1 << 18; // s: reserved bits 16-23, ignored
pub const MSR_FIELD_FLAGS: u32 = 1 << 19; // f: bits 24-31

/// PSR bits an MSR may write for the given field mask (opcode bits 16-19). The unused bits
/// 24-27 of the flags field are writable; reserved bits 8-23 never are.
fn msr_writable_bits(field_mask: u32) -> u32 {
    let mut writable = 0;
    if field_mask & MSR_FIELD_FLAGS != 0 {
        writable |= 0xFF00_0000;
    }
    if field_mask & MSR_FIELD_CONTROL != 0 {
        writable |= 0x0000_00FF;
    }
    writable
}

pub struct Cpu {
    /// R0-R15 as seen by the current mode. See the PC convention at the top of this file.
    pub registers: [u32; 16],
    /// Current program status register. Change it with `set_cpsr` so banking follows the mode.
    pub cpsr: u32,
    /// SPSR_fiq/svc/abt/irq/und, indexed by `Bank` (the User slot is unused).
    saved_psrs: [u32; 6],
    /// R13-R14 of each bank not currently active, indexed by `Bank`.
    banked_r13_r14: [[u32; 2]; 6],
    /// R8-R12 of User/System (stored here while in FIQ mode).
    user_r8_r12: [u32; 5],
    /// R8_fiq-R12_fiq (stored here while not in FIQ mode).
    fiq_r8_r12: [u32; 5],
    /// Set by `branch_to` during an instruction, so `step()` does not advance the PC.
    pipeline_flushed: bool,
}

/// Longest idle step while halted. Timer overflows can wake the CPU at any cycle and the CPU
/// cannot see them, so a halted step never skips more than this.
/// ponytail: fixed chunk; ask the bus for "cycles until next event" if halt timing needs to be exact.
const MAX_HALT_STEP_CYCLES: u32 = 16;

/// Cycles a halted CPU idles in one step: up to the next PPU HBlank/line boundary, capped.
fn halted_cycles(bus: &Bus) -> u32 {
    let line_cycle = bus.ppu.line_cycle;
    let next_boundary = if line_cycle < HBLANK_START_CYCLE { HBLANK_START_CYCLE } else { CYCLES_PER_LINE };
    (next_boundary - line_cycle).clamp(1, MAX_HALT_STEP_CYCLES)
}

impl Cpu {
    /// Power-on state: the Reset exception, i.e. Supervisor mode, ARM state, IRQ and FIQ disabled,
    /// PC at vector 0 (gbatek: ARM CPU Exceptions).
    pub fn new() -> Self {
        let mut cpu = Cpu {
            registers: [0; 16],
            cpsr: MODE_SUPERVISOR | CPSR_IRQ_DISABLE | CPSR_FIQ_DISABLE,
            saved_psrs: [0; 6],
            banked_r13_r14: [[0; 2]; 6],
            user_r8_r12: [0; 5],
            fiq_r8_r12: [0; 5],
            pipeline_flushed: false,
        };
        cpu.enter_exception(Exception::Reset);
        cpu
    }

    /// Execute one instruction and return the cycles it used, including bus access cycles.
    pub fn step(&mut self, bus: &mut Bus) -> u32 {
        if let Some(cycles) = self.service_interrupts_and_halt(bus) {
            return cycles + bus.take_access_cycles();
        }

        let instruction_address = self.registers[PROGRAM_COUNTER];
        self.pipeline_flushed = false;
        let internal_cycles = if self.in_thumb_state() {
            let opcode = bus.read16(instruction_address & !1);
            self.registers[PROGRAM_COUNTER] = instruction_address.wrapping_add(4);
            let cycles = thumb::execute_thumb(self, bus, opcode);
            self.advance_if_not_branched(instruction_address, 2);
            cycles
        } else {
            let opcode = bus.read32(instruction_address & !3);
            self.registers[PROGRAM_COUNTER] = instruction_address.wrapping_add(8);
            // A failed condition costs only the fetch (gbatek: "If condition=false: 1S cycle").
            let cycles = if self.cond_passed(opcode >> 28) { arm::execute_arm(self, bus, opcode) } else { 0 };
            self.advance_if_not_branched(instruction_address, 4);
            cycles
        };
        internal_cycles + bus.take_access_cycles()
    }

    /// Between instructions: idle while halted, then take a pending IRQ (gbatek: GBA Interrupt
    /// Control, GBA System Control HALTCNT, ARM CPU Exceptions).
    ///
    /// Halt ends once (IE AND IF) != 0, regardless of IME and CPSR.I. Returns `Some(cycles)` when
    /// the step was spent halted, so the rest of the machine advances by that many cycles.
    /// Taking an IRQ returns `None`: the first handler instruction runs in the same step.
    fn service_interrupts_and_halt(&mut self, bus: &mut Bus) -> Option<u32> {
        if bus.irq.halt_requested {
            if !bus.irq.wake_condition() {
                return Some(halted_cycles(bus));
            }
            bus.irq.halt_requested = false;
        }
        if bus.irq.pending() && self.cpsr & CPSR_IRQ_DISABLE == 0 {
            self.enter_exception(Exception::Interrupt);
        }
        bus.irq.executing_bios = self.registers[PROGRAM_COUNTER] < BIOS_SIZE as u32;
        None
    }

    fn advance_if_not_branched(&mut self, instruction_address: u32, instruction_size: u32) {
        if !self.pipeline_flushed {
            self.registers[PROGRAM_COUNTER] = instruction_address.wrapping_add(instruction_size);
        }
    }

    // ---- PC and registers ----

    /// Jump to `target`, aligned to 4 (ARM) or 2 (THUMB) for the current state, and flush the
    /// pipeline. Instruction-cycle refill costs are not charged yet (Phase 5 timing).
    pub fn branch_to(&mut self, target: u32) {
        let alignment_mask = if self.in_thumb_state() { !1 } else { !3 };
        self.registers[PROGRAM_COUNTER] = target & alignment_mask;
        self.pipeline_flushed = true;
    }

    /// Write `value` to register `index`; R15 goes through `branch_to`.
    pub fn write_register(&mut self, index: usize, value: u32) {
        if index == PROGRAM_COUNTER {
            self.branch_to(value);
        } else {
            self.registers[index] = value;
        }
    }

    /// Address of the instruction now executing (only valid during execution).
    pub fn current_instruction_address(&self) -> u32 {
        self.registers[PROGRAM_COUNTER].wrapping_sub(2 * self.instruction_size())
    }

    /// 4 in ARM state, 2 in THUMB state.
    pub fn instruction_size(&self) -> u32 {
        if self.in_thumb_state() { 2 } else { 4 }
    }

    /// Read a User-mode register from any mode (LDM/STM with the S bit, "^").
    pub fn read_user_register(&self, index: usize) -> u32 {
        let current_bank = bank_of(self.cpsr);
        match index {
            8..=12 if current_bank == Bank::Fiq => self.user_r8_r12[index - 8],
            13 | 14 if current_bank != Bank::User => self.banked_r13_r14[Bank::User as usize][index - 13],
            _ => self.registers[index],
        }
    }

    /// Write a User-mode register from any mode (LDM with the S bit, "^", R15 not in the list).
    pub fn write_user_register(&mut self, index: usize, value: u32) {
        let current_bank = bank_of(self.cpsr);
        match index {
            8..=12 if current_bank == Bank::Fiq => self.user_r8_r12[index - 8] = value,
            13 | 14 if current_bank != Bank::User => self.banked_r13_r14[Bank::User as usize][index - 13] = value,
            _ => self.write_register(index, value),
        }
    }

    // ---- CPSR / SPSR ----

    pub fn mode(&self) -> u32 {
        self.cpsr & MODE_MASK
    }

    pub fn in_thumb_state(&self) -> bool {
        self.cpsr & CPSR_THUMB != 0
    }

    /// Set or clear the T bit (BX). Call `branch_to` afterwards so the target is aligned for the new state.
    pub fn set_thumb_state(&mut self, thumb: bool) {
        if thumb {
            self.cpsr |= CPSR_THUMB;
        } else {
            self.cpsr &= !CPSR_THUMB;
        }
    }

    /// Replace the whole CPSR, swapping banked registers if the mode changes.
    pub fn set_cpsr(&mut self, value: u32) {
        self.switch_register_bank(value & MODE_MASK);
        self.cpsr = value;
    }

    /// Switch R8-R14 to the bank of `new_mode` (gbatek: ARM CPU Register Set).
    fn switch_register_bank(&mut self, new_mode: u32) {
        let old_bank = bank_of(self.cpsr);
        let new_bank = bank_of(new_mode);
        if old_bank == new_bank {
            return;
        }
        if old_bank == Bank::Fiq || new_bank == Bank::Fiq {
            let (save_to, load_from) = if new_bank == Bank::Fiq {
                (&mut self.user_r8_r12, &self.fiq_r8_r12)
            } else {
                (&mut self.fiq_r8_r12, &self.user_r8_r12)
            };
            save_to.copy_from_slice(&self.registers[8..13]);
            self.registers[8..13].copy_from_slice(load_from);
        }
        self.banked_r13_r14[old_bank as usize] = [self.registers[13], self.registers[14]];
        [self.registers[13], self.registers[14]] = self.banked_r13_r14[new_bank as usize];
    }

    /// Whether the current mode has an SPSR (every mode except User and System).
    pub fn has_spsr(&self) -> bool {
        bank_of(self.cpsr) != Bank::User
    }

    /// SPSR of the current mode. User/System have none; reading there returns the CPSR (guess:
    /// gbatek leaves it undefined).
    pub fn spsr(&self) -> u32 {
        match bank_of(self.cpsr) {
            Bank::User => self.cpsr,
            bank => self.saved_psrs[bank as usize],
        }
    }

    /// Write the SPSR of the current mode; ignored in User/System.
    pub fn set_spsr(&mut self, value: u32) {
        let bank = bank_of(self.cpsr);
        if bank != Bank::User {
            self.saved_psrs[bank as usize] = value;
        }
    }

    /// MSR to CPSR: write the flags and/or control byte selected by `field_mask` (opcode bits
    /// 16-19). In User mode only the flags byte changes; T is never changed by MSR
    /// (gbatek: ARM Opcodes: PSR Transfer (MRS, MSR)).
    pub fn write_cpsr_fields(&mut self, value: u32, field_mask: u32) {
        let mut writable = msr_writable_bits(field_mask) & !CPSR_THUMB;
        if self.mode() == MODE_USER {
            writable &= 0xFF00_0000;
        }
        self.set_cpsr((self.cpsr & !writable) | (value & writable));
    }

    /// MSR to SPSR: write the flags and/or control byte selected by `field_mask` (opcode bits
    /// 16-19). Reserved bits 8-23 are kept; T is writable so a handler can pick the return state.
    pub fn write_spsr_fields(&mut self, value: u32, field_mask: u32) {
        let writable = msr_writable_bits(field_mask);
        self.set_spsr((self.spsr() & !writable) | (value & writable));
    }

    /// CPSR = SPSR of the current mode, as done by ALU ops with S=1 and Rd=R15 and LDM with "^"
    /// and R15. Call before `branch_to` so the target is aligned for the restored state.
    pub fn restore_cpsr_from_spsr(&mut self) {
        if self.has_spsr() {
            self.set_cpsr(self.spsr());
        }
    }

    // ---- Flags ----

    /// Whether condition field `condition` passes (gbatek: ARM CPU Flags & Condition Field (cond)).
    pub fn cond_passed(&self, condition: u32) -> bool {
        alu::condition_passed(condition, self.cpsr)
    }

    pub fn negative(&self) -> bool {
        self.cpsr & FLAG_NEGATIVE != 0
    }
    pub fn zero(&self) -> bool {
        self.cpsr & FLAG_ZERO != 0
    }
    pub fn carry(&self) -> bool {
        self.cpsr & FLAG_CARRY != 0
    }
    pub fn overflow(&self) -> bool {
        self.cpsr & FLAG_OVERFLOW != 0
    }

    fn set_flag(&mut self, flag: u32, set: bool) {
        if set {
            self.cpsr |= flag;
        } else {
            self.cpsr &= !flag;
        }
    }
    pub fn set_negative(&mut self, set: bool) {
        self.set_flag(FLAG_NEGATIVE, set);
    }
    pub fn set_zero(&mut self, set: bool) {
        self.set_flag(FLAG_ZERO, set);
    }
    pub fn set_carry(&mut self, set: bool) {
        self.set_flag(FLAG_CARRY, set);
    }
    pub fn set_overflow(&mut self, set: bool) {
        self.set_flag(FLAG_OVERFLOW, set);
    }

    /// N and Z from a result (multiplies, and part of every flag-setting op).
    pub fn set_negative_and_zero(&mut self, result: u32) {
        self.set_negative(result >> 31 != 0);
        self.set_zero(result == 0);
    }

    /// Logical ops with S=1: N and Z from the result, C from the shifter, V unchanged.
    pub fn set_logical_flags(&mut self, result: u32, shifter_carry: bool) {
        self.set_negative_and_zero(result);
        self.set_carry(shifter_carry);
    }

    /// Arithmetic ops with S=1: N, Z, C and V from the result.
    pub fn set_arithmetic_flags(&mut self, result: alu::ArithmeticResult) {
        self.set_negative_and_zero(result.value);
        self.set_carry(result.carry);
        self.set_overflow(result.overflow);
    }

    // ---- Exceptions ----

    /// Enter `exception` (gbatek: ARM CPU Exceptions): R14_<mode> = return address, SPSR_<mode> =
    /// old CPSR, ARM state, new mode, I=1 (and F=1 for Reset), PC = vector.
    ///
    /// Undefined and SWI are raised by the decoders while executing; R14 is the next instruction
    /// (return with MOVS PC,R14). Interrupt is raised between instructions; R14 is the next
    /// instruction + 4 (return with SUBS PC,R14,#4). Reset leaves R14 and SPSR alone.
    pub fn enter_exception(&mut self, exception: Exception) {
        let next_instruction = match exception {
            Exception::Interrupt => self.registers[PROGRAM_COUNTER],
            _ => self.current_instruction_address().wrapping_add(self.instruction_size()),
        };
        let (new_mode, vector, return_address) = match exception {
            Exception::Reset => (MODE_SUPERVISOR, 0x00, None),
            Exception::UndefinedInstruction => (MODE_UNDEFINED, 0x04, Some(next_instruction)),
            Exception::SoftwareInterrupt => (MODE_SUPERVISOR, 0x08, Some(next_instruction)),
            Exception::Interrupt => (MODE_IRQ, 0x18, Some(next_instruction.wrapping_add(4))),
        };
        let old_cpsr = self.cpsr;
        let mut new_cpsr = (old_cpsr & !(MODE_MASK | CPSR_THUMB)) | new_mode | CPSR_IRQ_DISABLE;
        if exception == Exception::Reset {
            new_cpsr |= CPSR_FIQ_DISABLE;
        }
        self.set_cpsr(new_cpsr);
        if let Some(return_address) = return_address {
            self.registers[LINK_REGISTER] = return_address;
            self.set_spsr(old_cpsr);
        }
        self.branch_to(vector);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::irq::IrqSource;

    fn cpu_in_mode(mode: u32) -> Cpu {
        let mut cpu = Cpu::new();
        cpu.set_cpsr(mode);
        cpu
    }

    #[test]
    fn reset_state() {
        let cpu = Cpu::new();
        assert_eq!(cpu.mode(), MODE_SUPERVISOR);
        assert!(!cpu.in_thumb_state());
        assert_eq!(cpu.cpsr & (CPSR_IRQ_DISABLE | CPSR_FIQ_DISABLE), CPSR_IRQ_DISABLE | CPSR_FIQ_DISABLE);
        assert_eq!(cpu.registers[PROGRAM_COUNTER], 0);
    }

    #[test]
    fn exception_modes_bank_r13_r14_only() {
        let mut cpu = cpu_in_mode(MODE_SYSTEM);
        for index in 0..15 {
            cpu.registers[index] = 100 + index as u32;
        }
        let banked_modes = [MODE_SUPERVISOR, MODE_ABORT, MODE_IRQ, MODE_UNDEFINED];
        for (number, &mode) in banked_modes.iter().enumerate() {
            cpu.set_cpsr(mode);
            assert_eq!(cpu.registers[8], 108, "R8-R12 shared in mode {mode:X}");
            cpu.registers[13] = 1000 + number as u32;
            cpu.registers[14] = 2000 + number as u32;
        }
        cpu.set_cpsr(MODE_USER); // User shares its bank with System
        assert_eq!(cpu.registers[13], 113);
        assert_eq!(cpu.registers[14], 114);
        for (number, &mode) in banked_modes.iter().enumerate() {
            cpu.set_cpsr(mode);
            assert_eq!(cpu.registers[13], 1000 + number as u32);
            assert_eq!(cpu.registers[14], 2000 + number as u32);
        }
    }

    #[test]
    fn fiq_mode_banks_r8_to_r14() {
        let mut cpu = cpu_in_mode(MODE_USER);
        for index in 0..15 {
            cpu.registers[index] = index as u32;
        }
        cpu.set_cpsr(MODE_FIQ);
        assert_eq!(cpu.registers[7], 7, "R0-R7 are never banked");
        for index in 8..15 {
            assert_eq!(cpu.registers[index], 0, "R{index}_fiq starts at 0");
            cpu.registers[index] = 0xF0 + index as u32;
        }
        assert_eq!(cpu.read_user_register(9), 9);
        assert_eq!(cpu.read_user_register(14), 14);
        cpu.write_user_register(10, 0xAA);
        cpu.set_cpsr(MODE_IRQ); // FIQ -> IRQ restores user R8-R12
        assert_eq!(cpu.registers[8], 8);
        assert_eq!(cpu.registers[10], 0xAA);
        cpu.set_cpsr(MODE_FIQ);
        assert_eq!(cpu.registers[12], 0xFC);
        assert_eq!(cpu.registers[14], 0xFE);
    }

    #[test]
    fn spsr_is_per_mode_and_absent_in_user_and_system() {
        let mut cpu = cpu_in_mode(MODE_IRQ);
        cpu.set_spsr(0x1234_5612);
        cpu.set_cpsr(MODE_SUPERVISOR);
        cpu.set_spsr(0xABCD_0013);
        cpu.set_cpsr(MODE_IRQ);
        assert_eq!(cpu.spsr(), 0x1234_5612);
        cpu.set_cpsr(MODE_SYSTEM);
        assert!(!cpu.has_spsr());
        cpu.set_spsr(0xDEAD_BEEF);
        assert_eq!(cpu.spsr(), cpu.cpsr);
    }

    #[test]
    fn msr_field_masks_and_user_protection() {
        let mut cpu = cpu_in_mode(MODE_SYSTEM);
        cpu.write_cpsr_fields(0xF000_0000 | MODE_IRQ, MSR_FIELD_FLAGS);
        assert_eq!(cpu.cpsr, 0xF000_0000 | MODE_SYSTEM, "flags only");
        cpu.write_cpsr_fields(MODE_IRQ | CPSR_THUMB, MSR_FIELD_CONTROL);
        assert_eq!(cpu.mode(), MODE_IRQ, "control byte switches mode");
        assert!(!cpu.in_thumb_state(), "MSR never changes T");
        assert_eq!(cpu.cpsr >> 28, 0xF, "flags byte untouched without f");
        cpu.set_cpsr(MODE_USER);
        cpu.write_cpsr_fields(0x8000_0000 | MODE_SUPERVISOR, MSR_FIELD_FLAGS | MSR_FIELD_CONTROL);
        assert_eq!(cpu.cpsr, 0x8000_0000 | MODE_USER, "User mode may only write flags");
        let mut cpu = cpu_in_mode(MODE_SUPERVISOR);
        cpu.set_spsr(0x0000_0010);
        cpu.write_spsr_fields(0x6000_00FF, MSR_FIELD_FLAGS);
        assert_eq!(cpu.spsr(), 0x6000_0010);
    }

    #[test]
    fn msr_never_changes_reserved_bits() {
        let mut cpu = cpu_in_mode(MODE_SYSTEM);
        let all_fields = MSR_FIELD_FLAGS | MSR_FIELD_STATUS | MSR_FIELD_EXTENSION | MSR_FIELD_CONTROL;
        cpu.write_cpsr_fields(0xFFFF_FF00 | MODE_SYSTEM, all_fields);
        assert_eq!(cpu.cpsr, 0xFF00_0000 | MODE_SYSTEM, "bits 8-23 stay clear, flags field bits 24-27 writable");
        cpu.set_cpsr(MODE_IRQ);
        cpu.set_spsr(0);
        cpu.write_spsr_fields(0xFFFF_FFFF, all_fields);
        assert_eq!(cpu.spsr(), 0xFF00_00FF, "SPSR reserved bits kept, T writable");
    }

    /// Bus with `halfwords` stored at the start of IWRAM (0x03000000).
    fn bus_with_iwram_code(halfwords: &[u16]) -> Bus {
        let mut bus = Bus::new(&[]);
        for (index, &halfword) in halfwords.iter().enumerate() {
            bus.write16(0x0300_0000 + 2 * index as u32, halfword);
        }
        bus.take_access_cycles();
        bus
    }

    const IWRAM_START: u32 = 0x0300_0000;

    #[test]
    fn step_executes_arm_opcode_and_advances_pc() {
        // MOV R0,R0 (0xE1A00000): condition AL, no visible effect besides the PC.
        let mut bus = bus_with_iwram_code(&[0x0000, 0xE1A0]);
        let mut cpu = cpu_in_mode(MODE_SYSTEM);
        cpu.registers[PROGRAM_COUNTER] = IWRAM_START;
        let cycles = cpu.step(&mut bus);
        assert_eq!(cpu.registers[PROGRAM_COUNTER], IWRAM_START + 4);
        assert!(cycles >= 1, "at least the IWRAM fetch");
    }

    #[test]
    fn step_skips_arm_opcode_whose_condition_fails() {
        // MOVNE R0,#1 (0x13A00001) with Z=1: skipped, costs only the 1-cycle IWRAM fetch.
        let mut bus = bus_with_iwram_code(&[0x0001, 0x13A0]);
        let mut cpu = cpu_in_mode(MODE_SYSTEM | FLAG_ZERO);
        cpu.registers[PROGRAM_COUNTER] = IWRAM_START;
        let cycles = cpu.step(&mut bus);
        assert_eq!(cpu.registers[0], 0);
        assert_eq!(cpu.registers[PROGRAM_COUNTER], IWRAM_START + 4);
        assert_eq!(cycles, 1);
    }

    #[test]
    fn step_fetches_thumb_halfwords() {
        // Two THUMB MOV R8,R8 (0x46C0) at 0x03000000 and 0x03000002.
        let mut bus = bus_with_iwram_code(&[0x46C0, 0x46C0]);
        let mut cpu = cpu_in_mode(MODE_SYSTEM | CPSR_THUMB);
        cpu.registers[PROGRAM_COUNTER] = IWRAM_START;
        cpu.step(&mut bus);
        assert_eq!(cpu.registers[PROGRAM_COUNTER], IWRAM_START + 2);
        cpu.step(&mut bus);
        assert_eq!(cpu.registers[PROGRAM_COUNTER], IWRAM_START + 4);
        assert!(cpu.in_thumb_state());
    }

    #[test]
    fn software_interrupt_from_thumb() {
        let mut cpu = cpu_in_mode(MODE_USER | CPSR_THUMB | FLAG_CARRY);
        cpu.registers[13] = 0x0300_7F00;
        // Executing a THUMB SWI at 0x08000100: R15 reads as +4.
        cpu.registers[PROGRAM_COUNTER] = 0x0800_0104;
        cpu.enter_exception(Exception::SoftwareInterrupt);
        assert_eq!(cpu.mode(), MODE_SUPERVISOR);
        assert!(!cpu.in_thumb_state());
        assert!(cpu.cpsr & CPSR_IRQ_DISABLE != 0);
        assert!(cpu.carry(), "flags are kept");
        assert_eq!(cpu.registers[LINK_REGISTER], 0x0800_0102);
        assert_eq!(cpu.spsr(), MODE_USER | CPSR_THUMB | FLAG_CARRY);
        assert_eq!(cpu.registers[PROGRAM_COUNTER], 0x08);
        cpu.restore_cpsr_from_spsr();
        assert_eq!(cpu.registers[13], 0x0300_7F00, "User SP back after return");
        assert!(cpu.in_thumb_state());
    }

    #[test]
    fn undefined_from_arm_and_interrupt_between_instructions() {
        let mut cpu = cpu_in_mode(MODE_SYSTEM | CPSR_FIQ_DISABLE);
        cpu.registers[PROGRAM_COUNTER] = 0x0800_0008; // executing ARM opcode at 0x08000000
        cpu.enter_exception(Exception::UndefinedInstruction);
        assert_eq!(cpu.mode(), MODE_UNDEFINED);
        assert_eq!(cpu.registers[LINK_REGISTER], 0x0800_0004);
        assert_eq!(cpu.registers[PROGRAM_COUNTER], 0x04);

        let mut cpu = cpu_in_mode(MODE_SYSTEM | CPSR_THUMB);
        cpu.registers[PROGRAM_COUNTER] = 0x0800_0200; // next instruction to execute
        cpu.enter_exception(Exception::Interrupt);
        assert_eq!(cpu.mode(), MODE_IRQ);
        assert_eq!(cpu.registers[LINK_REGISTER], 0x0800_0204, "SUBS PC,R14,#4 returns to 0x08000200");
        assert_eq!(cpu.cpsr & CPSR_FIQ_DISABLE, 0, "IRQ leaves F unchanged");
        assert_eq!(cpu.registers[PROGRAM_COUNTER], 0x18);
    }

    #[test]
    fn halt_idles_until_ie_and_if_then_irq_needs_ime_and_cpsr_i() {
        let mut bus = Bus::new(&[]);
        let mut cpu = cpu_in_mode(MODE_SYSTEM);
        cpu.registers[PROGRAM_COUNTER] = 0x0300_0000;
        bus.irq.halt_requested = true;
        bus.irq.interrupt_enable = 1;
        let idle = cpu.service_interrupts_and_halt(&mut bus);
        assert!(matches!(idle, Some(1..=MAX_HALT_STEP_CYCLES)));

        bus.irq.request(IrqSource::VBlank);
        assert_eq!(cpu.service_interrupts_and_halt(&mut bus), None, "IE AND IF wakes even with IME=0");
        assert!(!bus.irq.halt_requested);
        assert_eq!(cpu.mode(), MODE_SYSTEM, "no IRQ while IME=0");

        bus.irq.master_enable = true;
        cpu.cpsr |= CPSR_IRQ_DISABLE;
        cpu.service_interrupts_and_halt(&mut bus);
        assert_eq!(cpu.mode(), MODE_SYSTEM, "no IRQ while CPSR.I=1");

        cpu.cpsr &= !CPSR_IRQ_DISABLE;
        cpu.service_interrupts_and_halt(&mut bus);
        assert_eq!(cpu.mode(), MODE_IRQ);
        assert_eq!(cpu.registers[LINK_REGISTER], 0x0300_0004);
        assert!(bus.irq.executing_bios, "the IRQ vector is BIOS code");
    }

    #[test]
    fn branch_aligns_for_state() {
        let mut cpu = cpu_in_mode(MODE_SYSTEM);
        cpu.write_register(PROGRAM_COUNTER, 0x0800_0003);
        assert_eq!(cpu.registers[PROGRAM_COUNTER], 0x0800_0000);
        cpu.set_thumb_state(true);
        cpu.branch_to(0x0800_0003);
        assert_eq!(cpu.registers[PROGRAM_COUNTER], 0x0800_0002);
    }

    #[test]
    fn flag_helpers() {
        let mut cpu = cpu_in_mode(MODE_SYSTEM | FLAG_OVERFLOW);
        cpu.set_logical_flags(0, true);
        assert!(cpu.zero() && cpu.carry() && !cpu.negative() && cpu.overflow());
        cpu.set_arithmetic_flags(alu::add(0x7FFF_FFFF, 1));
        assert!(cpu.negative() && !cpu.zero() && !cpu.carry() && cpu.overflow());
        assert!(cpu.cond_passed(0xA), "GE: N = V = 1");
        assert!(!cpu.cond_passed(0xB), "LT fails");
    }
}
