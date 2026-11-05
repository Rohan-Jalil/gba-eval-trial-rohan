//! ARM (32-bit) instruction decode and execute (gbatek: ARM Instruction Summary and ARM Opcodes).
//!
//! Cycle counts returned here are internal (I) cycles only, from gbatek "ARM CPU Instruction Cycle
//! Times"; the bus counts every S/N access itself.

use super::alu::{self, ShiftType};
use super::{Cpu, Exception, PROGRAM_COUNTER};
use crate::bus::Bus;

/// Bit `index` of `opcode` as a bool.
fn bit(opcode: u32, index: u32) -> bool {
    (opcode >> index) & 1 != 0
}

/// 4-bit register field starting at bit `lowest_bit`.
fn register_field(opcode: u32, lowest_bit: u32) -> usize {
    ((opcode >> lowest_bit) & 0xF) as usize
}

/// Read register `index`; R15 reads as the pipelined PC+8 plus `extra_pc_offset`
/// (4 for shift-by-register operands and stored R15, gbatek: "Using R15 (PC)").
fn read_operand(cpu: &Cpu, index: usize, extra_pc_offset: u32) -> u32 {
    if index == PROGRAM_COUNTER {
        cpu.registers[PROGRAM_COUNTER].wrapping_add(extra_pc_offset)
    } else {
        cpu.registers[index]
    }
}

/// Execute one ARM opcode whose condition already passed (checked in `Cpu::step`). R15 reads as
/// the opcode's address + 8. Returns internal cycles only; bus accesses are counted by the bus.
pub(super) fn execute_arm(cpu: &mut Cpu, bus: &mut Bus, opcode: u32) -> u32 {
    match (opcode >> 25) & 0b111 {
        0b000 => execute_group_zero(cpu, bus, opcode),
        0b001 => {
            if is_psr_transfer(opcode) {
                execute_psr_transfer(cpu, opcode)
            } else {
                execute_data_processing(cpu, opcode)
            }
        }
        0b010 => execute_single_data_transfer(cpu, bus, opcode),
        0b011 if bit(opcode, 4) => undefined_instruction(cpu),
        0b011 => execute_single_data_transfer(cpu, bus, opcode),
        0b100 => execute_block_data_transfer(cpu, bus, opcode),
        0b101 => execute_branch(cpu, opcode),
        0b111 if bit(opcode, 24) => {
            cpu.enter_exception(Exception::SoftwareInterrupt);
            0
        }
        // Coprocessor opcodes: the GBA has no coprocessor, so they trap as undefined.
        _ => undefined_instruction(cpu),
    }
}

/// Opcode bits 27-25 = 000: BX, multiplies, SWP, halfword transfers, PSR transfer and ALU ops
/// share this space (gbatek: ARM Instruction Summary).
fn execute_group_zero(cpu: &mut Cpu, bus: &mut Bus, opcode: u32) -> u32 {
    if opcode & 0x0FFF_FFF0 == 0x012F_FF10 {
        return execute_branch_and_exchange(cpu, opcode);
    }
    if opcode & 0xF0 == 0x90 {
        return match (opcode >> 23) & 0b11 {
            0b00 => execute_multiply(cpu, opcode),
            0b01 => execute_multiply_long(cpu, opcode),
            0b10 if (opcode >> 20) & 0b11 == 0 => execute_swap(cpu, bus, opcode),
            _ => undefined_instruction(cpu),
        };
    }
    if bit(opcode, 7) && bit(opcode, 4) {
        return execute_halfword_transfer(cpu, bus, opcode);
    }
    if is_psr_transfer(opcode) {
        return execute_psr_transfer(cpu, opcode);
    }
    execute_data_processing(cpu, opcode)
}

/// TST/TEQ/CMP/CMN with S=0 encode MRS/MSR (gbatek: ARM Opcodes: PSR Transfer).
fn is_psr_transfer(opcode: u32) -> bool {
    (opcode >> 23) & 0b11 == 0b10 && !bit(opcode, 20)
}

/// Take the Undefined Instruction exception (gbatek: ARM CPU Exceptions).
fn undefined_instruction(cpu: &mut Cpu) -> u32 {
    cpu.enter_exception(Exception::UndefinedInstruction);
    0
}

// ---- Branches ----

/// B and BL: PC-relative jump by a signed 24-bit word offset (gbatek: ARM Opcodes: Branch and
/// Branch with Link (B, BL, BLX)). BL stores the next instruction's address in R14.
fn execute_branch(cpu: &mut Cpu, opcode: u32) -> u32 {
    let word_offset = ((opcode << 8) as i32) >> 8;
    let target = cpu.registers[PROGRAM_COUNTER].wrapping_add((word_offset << 2) as u32);
    if bit(opcode, 24) {
        cpu.registers[super::LINK_REGISTER] = cpu.current_instruction_address().wrapping_add(4);
    }
    cpu.branch_to(target);
    0
}

/// BX Rn: jump and switch to THUMB if bit 0 of Rn is set (gbatek: ARM Opcodes: Branch and
/// Exchange (BX, BLX)).
fn execute_branch_and_exchange(cpu: &mut Cpu, opcode: u32) -> u32 {
    let target = read_operand(cpu, register_field(opcode, 0), 0);
    cpu.set_thumb_state(target & 1 != 0);
    cpu.branch_to(target);
    0
}

// ---- Data processing ----

/// Second operand of an ALU op and the shifter carry-out (gbatek: ARM Opcodes: Data Processing
/// (ALU), "Second Operand"). Also returns the extra PC offset (4 when shifting by register).
fn alu_second_operand(cpu: &Cpu, opcode: u32) -> (u32, bool, u32) {
    if bit(opcode, 25) {
        let (value, carry) = alu::rotated_immediate(opcode & 0xFF, (opcode >> 8) & 0xF, cpu.carry());
        return (value, carry, 0);
    }
    let shift_type = ShiftType::from_bits((opcode >> 5) & 0b11);
    if bit(opcode, 4) {
        let operand = read_operand(cpu, register_field(opcode, 0), 4);
        let shift_register = cpu.registers[register_field(opcode, 8)];
        let (value, carry) = alu::shift_by_register(shift_type, operand, shift_register, cpu.carry());
        (value, carry, 4)
    } else {
        let operand = read_operand(cpu, register_field(opcode, 0), 0);
        let (value, carry) = alu::shift_by_immediate(shift_type, operand, (opcode >> 7) & 0x1F, cpu.carry());
        (value, carry, 0)
    }
}

/// The 16 ALU opcodes (gbatek: ARM Opcodes: Data Processing (ALU)). Internal cycles: 1I for a
/// shift by register.
fn execute_data_processing(cpu: &mut Cpu, opcode: u32) -> u32 {
    let (second, shifter_carry, extra_pc_offset) = alu_second_operand(cpu, opcode);
    let first = read_operand(cpu, register_field(opcode, 16), extra_pc_offset);
    let alu_opcode = (opcode >> 21) & 0xF;
    let carry = cpu.carry();
    let (value, arithmetic) = match alu_opcode {
        0x0 | 0x8 => (first & second, None),
        0x1 | 0x9 => (first ^ second, None),
        0xC => (first | second, None),
        0xD => (second, None),
        0xE => (first & !second, None),
        0xF => (!second, None),
        _ => {
            let result = match alu_opcode {
                0x2 | 0xA => alu::subtract(first, second),
                0x3 => alu::subtract(second, first),
                0x4 | 0xB => alu::add(first, second),
                0x5 => alu::add_with_carry(first, second, carry),
                0x6 => alu::subtract_with_carry(first, second, carry),
                _ => alu::subtract_with_carry(second, first, carry), // 0x7 RSC
            };
            (result.value, Some(result))
        }
    };

    let is_test = (0x8..=0xB).contains(&alu_opcode);
    let destination_register = register_field(opcode, 12);
    let set_flags = bit(opcode, 20);
    if set_flags && destination_register == PROGRAM_COUNTER {
        // "If S=1, Rd=R15: CPSR = SPSR_<current mode>, PC = result". The test ops (TST..CMN)
        // with Rd=R15 restore the CPSR too but leave the PC alone (jsmolka arm.gba checks this).
        cpu.restore_cpsr_from_spsr();
    } else if set_flags {
        match arithmetic {
            Some(result) => cpu.set_arithmetic_flags(result),
            None => cpu.set_logical_flags(value, shifter_carry),
        }
    }
    if !is_test {
        cpu.write_register(destination_register, value);
    }
    (extra_pc_offset != 0) as u32
}

// ---- PSR transfer ----

/// MRS Rd,Psr and MSR Psr{_field},Op (gbatek: ARM Opcodes: PSR Transfer (MRS, MSR)). 1S, no I.
fn execute_psr_transfer(cpu: &mut Cpu, opcode: u32) -> u32 {
    let use_spsr = bit(opcode, 22);
    if !bit(opcode, 21) {
        let value = if use_spsr { cpu.spsr() } else { cpu.cpsr };
        cpu.write_register(register_field(opcode, 12), value);
        return 0;
    }
    let value = if bit(opcode, 25) {
        alu::rotated_immediate(opcode & 0xFF, (opcode >> 8) & 0xF, cpu.carry()).0
    } else {
        cpu.registers[register_field(opcode, 0)]
    };
    if use_spsr {
        cpu.write_spsr_fields(value, opcode);
    } else {
        cpu.write_cpsr_fields(value, opcode);
    }
    0
}

// ---- Multiply ----

/// The "m" internal cycles of a multiply: 1 to 4 depending on how many top bytes of the
/// multiplier are all zero (or all one, when `signed`) (gbatek: ARM Opcodes: Multiply).
fn multiplier_cycles(multiplier: u32, signed: bool) -> u32 {
    let significant = |mask: u32| {
        let top = multiplier & mask;
        top != 0 && !(signed && top == mask)
    };
    if !significant(0xFFFF_FF00) {
        1
    } else if !significant(0xFFFF_0000) {
        2
    } else if !significant(0xFF00_0000) {
        3
    } else {
        4
    }
}

/// MUL and MLA (gbatek: ARM Opcodes: Multiply and Multiply-Accumulate). Flags: N and Z; C is
/// "destroyed" on ARMv4 and is left unchanged here. Internal cycles: m (MUL) or m+1 (MLA).
fn execute_multiply(cpu: &mut Cpu, opcode: u32) -> u32 {
    let multiplier = cpu.registers[register_field(opcode, 8)];
    let mut result = cpu.registers[register_field(opcode, 0)].wrapping_mul(multiplier);
    let accumulate = bit(opcode, 21);
    if accumulate {
        result = result.wrapping_add(cpu.registers[register_field(opcode, 12)]);
    }
    cpu.write_register(register_field(opcode, 16), result);
    if bit(opcode, 20) {
        cpu.set_negative_and_zero(result);
    }
    multiplier_cycles(multiplier, true) + accumulate as u32
}

/// UMULL, UMLAL, SMULL, SMLAL: 64-bit product in RdHi:RdLo (gbatek: ARM Opcodes: Multiply Long).
/// Flags: N and Z of the 64-bit result. Internal cycles: m+1 (MULL) or m+2 (MLAL).
fn execute_multiply_long(cpu: &mut Cpu, opcode: u32) -> u32 {
    let signed = bit(opcode, 22);
    let accumulate = bit(opcode, 21);
    let low_register = register_field(opcode, 12);
    let high_register = register_field(opcode, 16);
    let multiplicand = cpu.registers[register_field(opcode, 0)];
    let multiplier = cpu.registers[register_field(opcode, 8)];
    let mut result = if signed {
        (multiplicand as i32 as i64).wrapping_mul(multiplier as i32 as i64) as u64
    } else {
        (multiplicand as u64) * (multiplier as u64)
    };
    if accumulate {
        let accumulator = ((cpu.registers[high_register] as u64) << 32) | cpu.registers[low_register] as u64;
        result = result.wrapping_add(accumulator);
    }
    cpu.write_register(low_register, result as u32);
    cpu.write_register(high_register, (result >> 32) as u32);
    if bit(opcode, 20) {
        cpu.set_negative((result >> 63) != 0);
        cpu.set_zero(result == 0);
        if multiplier_cycles(multiplier, signed) == 4 {
            cpu.set_carry(long_multiply_carry(multiplicand, multiplier, signed));
        }
    }
    multiplier_cycles(multiplier, signed) + 1 + accumulate as u32
}

/// C after a long multiply that used all four multiplier cycles. gbatek only says "C=destroyed";
/// this rule is fitted to the expected flags shown by the mgba-suite multiply-long test screens:
/// take the Booth digit of multiplier bits 31-29 (for unsigned, bit 31 counts as +2 instead of
/// -2), form digit * Rm with negative digits as the one's complement, and C is bit 32.
// ponytail: C stays unchanged for m < 4 and for MUL/MLA, where no test shows the expected value.
fn long_multiply_carry(multiplicand: u32, multiplier: u32, signed: bool) -> bool {
    let top_bit = |index: u32| ((multiplier >> index) & 1) as i64;
    let bit31_weight = if signed { -2 } else { 2 };
    let digit = bit31_weight * top_bit(31) + top_bit(30) + top_bit(29);
    let magnitude = multiplicand as u64 * digit.unsigned_abs();
    let partial_product = if digit < 0 { !magnitude } else { magnitude };
    (partial_product >> 32) & 1 != 0
}

// ---- Memory ----

/// Word load with the ARMv4 misaligned rotation: read the aligned word, then rotate right by
/// (address AND 3) * 8 (gbatek: ARM CPU Memory Alignments, "Mis-aligned LDR,SWP").
fn read_word_rotated(bus: &mut Bus, address: u32) -> u32 {
    bus.read32(address & !3).rotate_right((address & 3) * 8)
}

/// Apply an up/down offset to a base address.
fn offset_address(base: u32, offset: u32, up: bool) -> u32 {
    if up { base.wrapping_add(offset) } else { base.wrapping_sub(offset) }
}

/// Shared addressing for LDR/STR and LDRH/STRH: returns (transfer address, written-back base).
/// Post-indexing always writes back; pre-indexing only with W (gbatek: ARM Opcodes: Memory).
fn indexed_addresses(base: u32, offset: u32, opcode: u32) -> (u32, Option<u32>) {
    let pre_index = bit(opcode, 24);
    let offset_base = offset_address(base, offset, bit(opcode, 23));
    let transfer_address = if pre_index { offset_base } else { base };
    let write_back = !pre_index || bit(opcode, 21);
    (transfer_address, write_back.then_some(offset_base))
}

/// LDR, STR, LDRB, STRB (gbatek: ARM Opcodes: Memory: Single Data Transfer). The T bit (forced
/// user access) has no effect on the GBA. Internal cycles: 1I for loads.
fn execute_single_data_transfer(cpu: &mut Cpu, bus: &mut Bus, opcode: u32) -> u32 {
    let offset = if bit(opcode, 25) {
        let shift_type = ShiftType::from_bits((opcode >> 5) & 0b11);
        let offset_register = cpu.registers[register_field(opcode, 0)];
        alu::shift_by_immediate(shift_type, offset_register, (opcode >> 7) & 0x1F, cpu.carry()).0
    } else {
        opcode & 0xFFF
    };
    let base_register = register_field(opcode, 16);
    let data_register = register_field(opcode, 12);
    let (address, written_back_base) = indexed_addresses(cpu.registers[base_register], offset, opcode);
    let is_byte = bit(opcode, 22);

    if bit(opcode, 20) {
        let value = if is_byte { bus.read8(address) as u32 } else { read_word_rotated(bus, address) };
        if let Some(new_base) = written_back_base {
            cpu.write_register(base_register, new_base);
        }
        cpu.write_register(data_register, value); // a loaded Rd == Rn wins over write-back
        1
    } else {
        let value = read_operand(cpu, data_register, 4); // stored R15 is PC+12
        if is_byte {
            bus.write8(address, value as u8);
        } else {
            bus.write32(address, value);
        }
        if let Some(new_base) = written_back_base {
            cpu.write_register(base_register, new_base);
        }
        0
    }
}

/// STRH, LDRH, LDRSB, LDRSH (gbatek: ARM Opcodes: Memory: Halfword, Doubleword, and Signed Data
/// Transfer), with the ARMv4 misaligned quirks from "ARM CPU Memory Alignments". 1I for loads.
fn execute_halfword_transfer(cpu: &mut Cpu, bus: &mut Bus, opcode: u32) -> u32 {
    let offset = if bit(opcode, 22) {
        ((opcode >> 4) & 0xF0) | (opcode & 0xF)
    } else {
        cpu.registers[register_field(opcode, 0)]
    };
    let base_register = register_field(opcode, 16);
    let data_register = register_field(opcode, 12);
    let (address, written_back_base) = indexed_addresses(cpu.registers[base_register], offset, opcode);
    let transfer_kind = (opcode >> 5) & 0b11;

    if bit(opcode, 20) {
        let value = match transfer_kind {
            1 => load_halfword_unsigned(bus, address),
            2 => bus.read8(address) as i8 as u32,
            3 => load_halfword_signed(bus, address),
            _ => return undefined_instruction(cpu), // SWP space; never reached via decode
        };
        if let Some(new_base) = written_back_base {
            cpu.write_register(base_register, new_base);
        }
        cpu.write_register(data_register, value);
        1
    } else {
        // L=0 with opcode 2/3 is LDRD/STRD (ARMv5TE); on ARMv4 treat every store as STRH (guess).
        let value = read_operand(cpu, data_register, 4);
        bus.write16(address, value as u16);
        if let Some(new_base) = written_back_base {
            cpu.write_register(base_register, new_base);
        }
        0
    }
}

/// LDRH: an odd address reads the aligned halfword rotated right by 8 (ARMv4).
fn load_halfword_unsigned(bus: &mut Bus, address: u32) -> u32 {
    (bus.read16(address & !1) as u32).rotate_right((address & 1) * 8)
}

/// LDRSH: an odd address acts as LDRSB (ARMv4).
fn load_halfword_signed(bus: &mut Bus, address: u32) -> u32 {
    if address & 1 != 0 {
        bus.read8(address) as i8 as u32
    } else {
        bus.read16(address) as i16 as u32
    }
}

/// SWP and SWPB: Rd=[Rn], [Rn]=Rm; the word read is rotated, the write is not (gbatek: ARM
/// Opcodes: Memory: Single Data Swap (SWP)). Internal cycles: 1I.
fn execute_swap(cpu: &mut Cpu, bus: &mut Bus, opcode: u32) -> u32 {
    let address = cpu.registers[register_field(opcode, 16)];
    let source = cpu.registers[register_field(opcode, 0)];
    let loaded = if bit(opcode, 22) {
        let byte = bus.read8(address) as u32;
        bus.write8(address, source as u8);
        byte
    } else {
        let word = read_word_rotated(bus, address);
        bus.write32(address, source);
        word
    };
    cpu.write_register(register_field(opcode, 12), loaded);
    1
}

/// LDM and STM (gbatek: ARM Opcodes: Memory: Block Data Transfer (LDM, STM)), including the
/// ARMv4 edge cases: an empty list transfers R15 and moves the base by 0x40; STM with the base in
/// the list stores the old base only if it is the first register; LDM with the base in the list
/// does not write back. Internal cycles: 1I for LDM.
fn execute_block_data_transfer(cpu: &mut Cpu, bus: &mut Bus, opcode: u32) -> u32 {
    let base_register = register_field(opcode, 16);
    let (register_list, transfer_count) = match opcode & 0xFFFF {
        0 => (1 << PROGRAM_COUNTER, 16),
        list => (list, list.count_ones()),
    };
    let base = cpu.registers[base_register];
    let total_size = transfer_count * 4;
    let up = bit(opcode, 23);
    let pre_index = bit(opcode, 24);
    let new_base = offset_address(base, total_size, up);
    // Registers always go to increasing addresses starting from the lowest one.
    let lowest_address = match (up, pre_index) {
        (true, false) => base,
        (true, true) => base.wrapping_add(4),
        (false, false) => new_base.wrapping_add(4),
        (false, true) => new_base,
    };
    let is_load = bit(opcode, 20);
    let write_back = bit(opcode, 21);
    let loads_program_counter = is_load && register_list & (1 << PROGRAM_COUNTER) != 0;
    let user_bank = bit(opcode, 22) && !loads_program_counter;
    let registers = (0..16).filter(|index| register_list & (1 << index) != 0);

    if !is_load {
        let first_register = register_list.trailing_zeros() as usize;
        for (slot, index) in registers.enumerate() {
            let value = if index == base_register && write_back && index != first_register {
                new_base
            } else if index == PROGRAM_COUNTER {
                cpu.registers[PROGRAM_COUNTER].wrapping_add(4) // stored R15 is PC+12
            } else if user_bank {
                cpu.read_user_register(index)
            } else {
                cpu.registers[index]
            };
            bus.write32(lowest_address.wrapping_add(slot as u32 * 4), value);
        }
        if write_back {
            cpu.write_register(base_register, new_base);
        }
        return 0;
    }

    let mut loaded_program_counter = None;
    for (slot, index) in registers.enumerate() {
        let value = bus.read32(lowest_address.wrapping_add(slot as u32 * 4));
        if index == PROGRAM_COUNTER {
            loaded_program_counter = Some(value);
        } else if user_bank {
            cpu.write_user_register(index, value);
        } else {
            cpu.registers[index] = value;
        }
    }
    if write_back && register_list & (1 << base_register) == 0 {
        cpu.write_register(base_register, new_base);
    }
    if let Some(target) = loaded_program_counter {
        if bit(opcode, 22) {
            cpu.restore_cpsr_from_spsr();
        }
        cpu.branch_to(target);
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::{FLAG_CARRY, FLAG_NEGATIVE, FLAG_ZERO, MODE_IRQ, MODE_SYSTEM};

    const IWRAM: u32 = 0x0300_0000;

    /// A System-mode CPU "executing" at 0x08000000 (R15 reads 0x08000008), and an empty bus.
    fn setup() -> (Cpu, Bus) {
        let mut cpu = Cpu::new();
        cpu.set_cpsr(MODE_SYSTEM);
        cpu.registers[PROGRAM_COUNTER] = 0x0800_0008;
        (cpu, Bus::new(&[0; 0x200]))
    }

    #[test]
    fn data_processing_flags_and_pc_operand() {
        let (mut cpu, mut bus) = setup();
        cpu.registers[1] = 5;
        execute_arm(&mut cpu, &mut bus, 0xE251_0005); // SUBS R0,R1,#5
        assert_eq!(cpu.registers[0], 0);
        assert!(cpu.zero() && cpu.carry());
        execute_arm(&mut cpu, &mut bus, 0xE1A0_000F); // MOV R0,PC
        assert_eq!(cpu.registers[0], 0x0800_0008);
        cpu.registers[2] = 0;
        let cycles = execute_arm(&mut cpu, &mut bus, 0xE1A0_021F); // MOV R0,PC,LSL R2
        assert_eq!((cpu.registers[0], cycles), (0x0800_000C, 1));
    }

    #[test]
    fn movs_pc_restores_cpsr() {
        let (mut cpu, mut bus) = setup();
        cpu.set_cpsr(MODE_IRQ);
        cpu.set_spsr(MODE_SYSTEM | FLAG_CARRY);
        cpu.registers[14] = 0x0800_0100;
        execute_arm(&mut cpu, &mut bus, 0xE1B0_F00E); // MOVS PC,R14
        assert_eq!(cpu.cpsr, MODE_SYSTEM | FLAG_CARRY);
        assert_eq!(cpu.registers[PROGRAM_COUNTER], 0x0800_0100);
    }

    #[test]
    fn multiply_long_signed() {
        let (mut cpu, mut bus) = setup();
        cpu.registers[2] = (-3i32) as u32;
        cpu.registers[3] = 7;
        execute_arm(&mut cpu, &mut bus, 0xE0D1_0392); // SMULLS R0,R1,R2,R3
        assert_eq!((cpu.registers[0], cpu.registers[1]), ((-21i32) as u32, 0xFFFF_FFFF));
        assert!(cpu.cpsr & FLAG_NEGATIVE != 0 && cpu.cpsr & FLAG_ZERO == 0);
    }

    #[test]
    fn long_multiply_carry_matches_observed_flags() {
        // (Rm, Rs, signed, expected C) taken from the mgba-suite multiply-long screens.
        let cases = [
            (0xFFFF_FFFF, 0xFFFF_FFFF, false, true),
            (0x8000_0000, 0xFFFF_FFFF, false, false),
            (0x7FFF_FFFF, 0x7FFF_FFFF, false, false),
            (0x8000_0001, 0x8000_0000, false, true),
            (0xFFFF_FFFF, 0x7FFF_FFFF, true, true),
            (0x7FFF_FFFF, 0x7FFF_FFFF, true, false),
            (0x0000_0000, 0x8000_0000, true, true),
            (0xFFFF_FFFF, 0x8000_0001, true, false),
        ];
        for (multiplicand, multiplier, signed, expected) in cases {
            assert_eq!(long_multiply_carry(multiplicand, multiplier, signed), expected, "{multiplicand:X} * {multiplier:X}");
        }
    }

    #[test]
    fn misaligned_loads() {
        let (mut cpu, mut bus) = setup();
        bus.write32(IWRAM, 0x4433_2211);
        cpu.registers[1] = IWRAM + 1;
        execute_arm(&mut cpu, &mut bus, 0xE591_0000); // LDR R0,[R1]
        assert_eq!(cpu.registers[0], 0x1144_3322);
        execute_arm(&mut cpu, &mut bus, 0xE1D1_00B0); // LDRH R0,[R1]
        assert_eq!(cpu.registers[0], 0x1100_0022);
        bus.write32(IWRAM, 0x0000_8000);
        execute_arm(&mut cpu, &mut bus, 0xE1D1_00F0); // LDRSH R0,[R1] (odd: LDRSB)
        assert_eq!(cpu.registers[0], 0xFFFF_FF80);
    }

    #[test]
    fn block_transfer_edge_cases() {
        let (mut cpu, mut bus) = setup();
        cpu.registers[0] = IWRAM + 0x100;
        cpu.registers[1] = 0x11;
        execute_arm(&mut cpu, &mut bus, 0xE920_0003); // STMDB R0!,{R0,R1}: R0 first, old base
        assert_eq!(cpu.registers[0], IWRAM + 0xF8);
        assert_eq!(bus.read32(IWRAM + 0xF8), IWRAM + 0x100);
        assert_eq!(bus.read32(IWRAM + 0xFC), 0x11);

        cpu.registers[0] = IWRAM;
        execute_arm(&mut cpu, &mut bus, 0xE8A0_0000); // STMIA R0!,{} stores PC+12, base += 0x40
        assert_eq!(bus.read32(IWRAM), 0x0800_000C);
        assert_eq!(cpu.registers[0], IWRAM + 0x40);

        cpu.registers[1] = IWRAM + 0xF8;
        execute_arm(&mut cpu, &mut bus, 0xE8B1_0006); // LDMIA R1!,{R1,R2}: no write-back
        assert_eq!((cpu.registers[1], cpu.registers[2]), (IWRAM + 0x100, 0x11));
    }

    #[test]
    fn branch_link_and_exchange() {
        let (mut cpu, mut bus) = setup();
        execute_arm(&mut cpu, &mut bus, 0xEBFF_FFFE); // BL to self
        assert_eq!(cpu.registers[14], 0x0800_0004);
        assert_eq!(cpu.registers[PROGRAM_COUNTER], 0x0800_0000);
        cpu.registers[PROGRAM_COUNTER] = 0x0800_0008;
        cpu.registers[0] = 0x0800_0101;
        execute_arm(&mut cpu, &mut bus, 0xE12F_FF10); // BX R0
        assert!(cpu.in_thumb_state());
        assert_eq!(cpu.registers[PROGRAM_COUNTER], 0x0800_0100);
    }
}

