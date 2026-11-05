//! THUMB (16-bit) instruction decode and execute (gbatek: THUMB Instruction Summary and THUMB Opcodes).
//!
//! Every handler returns internal (I) cycles only; the bus counts its own S/N access cycles
//! (gbatek: ARM CPU Instruction Cycle Times). R15 reads as the opcode's address + 4.

use super::alu::{self, ShiftType};
use super::{Cpu, Exception, LINK_REGISTER, PROGRAM_COUNTER, STACK_POINTER};
use crate::bus::Bus;

/// Execute one THUMB opcode and return its internal cycles.
pub(super) fn execute_thumb(cpu: &mut Cpu, bus: &mut Bus, opcode: u16) -> u32 {
    let opcode = opcode as u32;
    match opcode >> 11 {
        0b00000..=0b00010 => move_shifted_register(cpu, opcode),
        0b00011 => add_subtract(cpu, opcode),
        0b00100..=0b00111 => immediate_operation(cpu, opcode),
        0b01000 if opcode & 0x0400 == 0 => alu_operation(cpu, opcode),
        0b01000 => hi_register_operation(cpu, opcode),
        0b01001 => load_pc_relative(cpu, bus, opcode),
        0b01010 | 0b01011 if opcode & 0x0200 == 0 => load_store_register_offset(cpu, bus, opcode),
        0b01010 | 0b01011 => load_store_sign_extended(cpu, bus, opcode),
        0b01100..=0b01111 => load_store_immediate_offset(cpu, bus, opcode),
        0b10000 | 0b10001 => load_store_halfword(cpu, bus, opcode),
        0b10010 | 0b10011 => load_store_sp_relative(cpu, bus, opcode),
        0b10100 | 0b10101 => load_address(cpu, opcode),
        0b10110 if opcode & 0x0F00 == 0 => adjust_stack_pointer(cpu, opcode),
        0b10110 | 0b10111 if opcode & 0x0600 == 0x0400 => push_pop(cpu, bus, opcode),
        0b11000 | 0b11001 => load_store_multiple(cpu, bus, opcode),
        0b11010 | 0b11011 => conditional_branch_or_swi(cpu, opcode),
        0b11100 => unconditional_branch(cpu, opcode),
        0b11110 => long_branch_first_half(cpu, opcode),
        0b11111 => long_branch_second_half(cpu, opcode),
        // BKPT, BLX and other ARMv5 encodings do not exist on the ARMv4T.
        _ => undefined(cpu),
    }
}

/// Low register number (R0-R7) from the 3-bit field at `shift`.
fn low_register(opcode: u32, shift: u32) -> usize {
    ((opcode >> shift) & 7) as usize
}

fn undefined(cpu: &mut Cpu) -> u32 {
    cpu.enter_exception(Exception::UndefinedInstruction);
    0
}

/// THUMB.1: LSL/LSR/ASR Rd,Rs,#offset (gbatek: THUMB Opcodes: Register Operations).
fn move_shifted_register(cpu: &mut Cpu, opcode: u32) -> u32 {
    let shift_type = ShiftType::from_bits(opcode >> 11);
    let shift_amount = (opcode >> 6) & 0x1F;
    let source = cpu.registers[low_register(opcode, 3)];
    let (result, carry) = alu::shift_by_immediate(shift_type, source, shift_amount, cpu.carry());
    cpu.registers[low_register(opcode, 0)] = result;
    cpu.set_logical_flags(result, carry);
    0
}

/// THUMB.2: ADD/SUB Rd,Rs,Rn and ADD/SUB Rd,Rs,#imm3.
fn add_subtract(cpu: &mut Cpu, opcode: u32) -> u32 {
    let operand_field = (opcode >> 6) & 7;
    let is_immediate = opcode & (1 << 10) != 0;
    let is_subtract = opcode & (1 << 9) != 0;
    let operand = if is_immediate { operand_field } else { cpu.registers[operand_field as usize] };
    let source = cpu.registers[low_register(opcode, 3)];
    let result = if is_subtract { alu::subtract(source, operand) } else { alu::add(source, operand) };
    cpu.registers[low_register(opcode, 0)] = result.value;
    cpu.set_arithmetic_flags(result);
    0
}

/// THUMB.3: MOV/CMP/ADD/SUB Rd,#imm8.
fn immediate_operation(cpu: &mut Cpu, opcode: u32) -> u32 {
    let destination = low_register(opcode, 8);
    let immediate = opcode & 0xFF;
    let current = cpu.registers[destination];
    match (opcode >> 11) & 3 {
        0 => {
            cpu.registers[destination] = immediate;
            cpu.set_negative_and_zero(immediate);
        }
        1 => cpu.set_arithmetic_flags(alu::subtract(current, immediate)),
        2 => {
            let result = alu::add(current, immediate);
            cpu.registers[destination] = result.value;
            cpu.set_arithmetic_flags(result);
        }
        _ => {
            let result = alu::subtract(current, immediate);
            cpu.registers[destination] = result.value;
            cpu.set_arithmetic_flags(result);
        }
    }
    0
}

/// THUMB.4: the 16 two-register ALU operations.
fn alu_operation(cpu: &mut Cpu, opcode: u32) -> u32 {
    let destination = low_register(opcode, 0);
    let first = cpu.registers[destination];
    let second = cpu.registers[low_register(opcode, 3)];
    let carry_in = cpu.carry();
    let shift = |cpu: &mut Cpu, shift_type| {
        let (result, carry) = alu::shift_by_register(shift_type, first, second, carry_in);
        cpu.registers[destination] = result;
        cpu.set_logical_flags(result, carry);
        1 // shift by register costs 1I
    };
    let write_logical = |cpu: &mut Cpu, result| {
        cpu.registers[destination] = result;
        cpu.set_negative_and_zero(result);
        0
    };
    let write_arithmetic = |cpu: &mut Cpu, result: alu::ArithmeticResult| {
        cpu.registers[destination] = result.value;
        cpu.set_arithmetic_flags(result);
        0
    };
    match (opcode >> 6) & 0xF {
        0x0 => write_logical(cpu, first & second),
        0x1 => write_logical(cpu, first ^ second),
        0x2 => shift(cpu, ShiftType::LogicalLeft),
        0x3 => shift(cpu, ShiftType::LogicalRight),
        0x4 => shift(cpu, ShiftType::ArithmeticRight),
        0x5 => write_arithmetic(cpu, alu::add_with_carry(first, second, carry_in)),
        0x6 => write_arithmetic(cpu, alu::subtract_with_carry(first, second, carry_in)),
        0x7 => shift(cpu, ShiftType::RotateRight),
        0x8 => {
            cpu.set_negative_and_zero(first & second);
            0
        }
        0x9 => write_arithmetic(cpu, alu::subtract(0, second)),
        0xA => {
            cpu.set_arithmetic_flags(alu::subtract(first, second));
            0
        }
        0xB => {
            cpu.set_arithmetic_flags(alu::add(first, second));
            0
        }
        0xC => write_logical(cpu, first | second),
        // ponytail: ARMv4 "destroys" C on MUL; we leave it unchanged (gbatek gives no value).
        0xD => write_logical(cpu, first.wrapping_mul(second)) + multiply_cycles(first),
        0xE => write_logical(cpu, first & !second),
        _ => write_logical(cpu, !second),
    }
}

/// Internal cycles m of MUL from the multiplier's top bytes being all zero or all one
/// (gbatek: ARM Opcodes: Multiply; THUMB.4 uses the incoming Rd value).
fn multiply_cycles(multiplier: u32) -> u32 {
    let top_bits_uniform = |mask: u32| multiplier & mask == 0 || multiplier & mask == mask;
    if top_bits_uniform(0xFFFF_FF00) {
        1
    } else if top_bits_uniform(0xFFFF_0000) {
        2
    } else if top_bits_uniform(0xFF00_0000) {
        3
    } else {
        4
    }
}

/// THUMB.5: ADD/CMP/MOV with R0-R15 and BX.
fn hi_register_operation(cpu: &mut Cpu, opcode: u32) -> u32 {
    let destination = low_register(opcode, 0) | (((opcode >> 7) & 1) << 3) as usize;
    let source = ((opcode >> 3) & 0xF) as usize;
    let source_value = cpu.registers[source];
    match (opcode >> 8) & 3 {
        0 => cpu.write_register(destination, cpu.registers[destination].wrapping_add(source_value)),
        1 => cpu.set_arithmetic_flags(alu::subtract(cpu.registers[destination], source_value)),
        2 => cpu.write_register(destination, source_value),
        _ => {
            // ponytail: MSBd set (ARMv5 BLX) is treated as plain BX on the ARMv4T.
            cpu.set_thumb_state(source_value & 1 != 0);
            cpu.branch_to(source_value);
        }
    }
    0
}

/// PC as a base for PC-relative addressing: ($+4) AND NOT 2.
fn word_aligned_pc(cpu: &Cpu) -> u32 {
    cpu.registers[PROGRAM_COUNTER] & !3
}

/// THUMB.6: LDR Rd,[PC,#imm8*4].
fn load_pc_relative(cpu: &mut Cpu, bus: &mut Bus, opcode: u32) -> u32 {
    let address = word_aligned_pc(cpu).wrapping_add((opcode & 0xFF) << 2);
    cpu.registers[low_register(opcode, 8)] = bus.read32(address);
    1
}

/// Word load with the misaligned rotation (gbatek: ARM CPU Memory Alignments, "rotated read").
fn load_word_rotated(bus: &mut Bus, address: u32) -> u32 {
    bus.read32(address).rotate_right((address & 3) * 8)
}

/// LDRH; an odd address reads the aligned halfword rotated by 8 (ARMv4 behaviour).
fn load_halfword_unsigned(bus: &mut Bus, address: u32) -> u32 {
    (bus.read16(address) as u32).rotate_right((address & 1) * 8)
}

/// LDSH; an odd address acts as LDSB on that byte (ARMv4 behaviour).
fn load_halfword_signed(bus: &mut Bus, address: u32) -> u32 {
    if address & 1 != 0 {
        load_byte_signed(bus, address)
    } else {
        bus.read16(address) as i16 as i32 as u32
    }
}

fn load_byte_signed(bus: &mut Bus, address: u32) -> u32 {
    bus.read8(address) as i8 as i32 as u32
}

/// The data sizes of THUMB.7 to THUMB.11, decoded per format.
#[derive(Clone, Copy)]
enum Transfer {
    StoreWord,
    StoreHalfword,
    StoreByte,
    LoadWord,
    LoadHalfword,
    LoadByte,
    LoadSignedHalfword,
    LoadSignedByte,
}

/// Perform a single load or store of register Rd at `address`; returns internal cycles
/// (1I for loads, none for stores).
fn transfer_single(cpu: &mut Cpu, bus: &mut Bus, transfer: Transfer, register: usize, address: u32) -> u32 {
    let value = cpu.registers[register];
    match transfer {
        Transfer::StoreWord => bus.write32(address & !3, value),
        Transfer::StoreHalfword => bus.write16(address & !1, value as u16),
        Transfer::StoreByte => bus.write8(address, value as u8),
        Transfer::LoadWord => cpu.registers[register] = load_word_rotated(bus, address),
        Transfer::LoadHalfword => cpu.registers[register] = load_halfword_unsigned(bus, address),
        Transfer::LoadByte => cpu.registers[register] = bus.read8(address) as u32,
        Transfer::LoadSignedHalfword => cpu.registers[register] = load_halfword_signed(bus, address),
        Transfer::LoadSignedByte => cpu.registers[register] = load_byte_signed(bus, address),
    }
    match transfer {
        Transfer::StoreWord | Transfer::StoreHalfword | Transfer::StoreByte => 0,
        _ => 1, // loads spend 1I
    }
}

/// Rb + Ro for THUMB.7 and THUMB.8.
fn register_offset_address(cpu: &Cpu, opcode: u32) -> u32 {
    cpu.registers[low_register(opcode, 3)].wrapping_add(cpu.registers[low_register(opcode, 6)])
}

/// THUMB.7: STR/STRB/LDR/LDRB Rd,[Rb,Ro].
fn load_store_register_offset(cpu: &mut Cpu, bus: &mut Bus, opcode: u32) -> u32 {
    let transfer = [Transfer::StoreWord, Transfer::StoreByte, Transfer::LoadWord, Transfer::LoadByte]
        [((opcode >> 10) & 3) as usize];
    let address = register_offset_address(cpu, opcode);
    transfer_single(cpu, bus, transfer, low_register(opcode, 0), address)
}

/// THUMB.8: STRH/LDSB/LDRH/LDSH Rd,[Rb,Ro].
fn load_store_sign_extended(cpu: &mut Cpu, bus: &mut Bus, opcode: u32) -> u32 {
    let transfer = [
        Transfer::StoreHalfword,
        Transfer::LoadSignedByte,
        Transfer::LoadHalfword,
        Transfer::LoadSignedHalfword,
    ][((opcode >> 10) & 3) as usize];
    let address = register_offset_address(cpu, opcode);
    transfer_single(cpu, bus, transfer, low_register(opcode, 0), address)
}

/// THUMB.9: STR/LDR Rd,[Rb,#imm5*4] and STRB/LDRB Rd,[Rb,#imm5].
fn load_store_immediate_offset(cpu: &mut Cpu, bus: &mut Bus, opcode: u32) -> u32 {
    let offset_field = (opcode >> 6) & 0x1F;
    let (transfer, offset) = match (opcode >> 11) & 3 {
        0 => (Transfer::StoreWord, offset_field << 2),
        1 => (Transfer::LoadWord, offset_field << 2),
        2 => (Transfer::StoreByte, offset_field),
        _ => (Transfer::LoadByte, offset_field),
    };
    let address = cpu.registers[low_register(opcode, 3)].wrapping_add(offset);
    transfer_single(cpu, bus, transfer, low_register(opcode, 0), address)
}

/// THUMB.10: STRH/LDRH Rd,[Rb,#imm5*2].
fn load_store_halfword(cpu: &mut Cpu, bus: &mut Bus, opcode: u32) -> u32 {
    let transfer = if opcode & (1 << 11) != 0 { Transfer::LoadHalfword } else { Transfer::StoreHalfword };
    let address = cpu.registers[low_register(opcode, 3)].wrapping_add(((opcode >> 6) & 0x1F) << 1);
    transfer_single(cpu, bus, transfer, low_register(opcode, 0), address)
}

/// THUMB.11: STR/LDR Rd,[SP,#imm8*4].
fn load_store_sp_relative(cpu: &mut Cpu, bus: &mut Bus, opcode: u32) -> u32 {
    let transfer = if opcode & (1 << 11) != 0 { Transfer::LoadWord } else { Transfer::StoreWord };
    let address = cpu.registers[STACK_POINTER].wrapping_add((opcode & 0xFF) << 2);
    transfer_single(cpu, bus, transfer, low_register(opcode, 8), address)
}

/// THUMB.12: ADD Rd,PC,#imm8*4 (PC word-aligned) and ADD Rd,SP,#imm8*4.
fn load_address(cpu: &mut Cpu, opcode: u32) -> u32 {
    let base = if opcode & (1 << 11) != 0 { cpu.registers[STACK_POINTER] } else { word_aligned_pc(cpu) };
    cpu.registers[low_register(opcode, 8)] = base.wrapping_add((opcode & 0xFF) << 2);
    0
}

/// THUMB.13: ADD SP,#+/-imm7*4.
fn adjust_stack_pointer(cpu: &mut Cpu, opcode: u32) -> u32 {
    let offset = (opcode & 0x7F) << 2;
    let stack_pointer = cpu.registers[STACK_POINTER];
    cpu.registers[STACK_POINTER] =
        if opcode & 0x80 != 0 { stack_pointer.wrapping_sub(offset) } else { stack_pointer.wrapping_add(offset) };
    0
}

/// Register list as a bit mask of R0-R15: Rlist in bits 0-7 plus the optional LR/PC bit, so
/// ascending bit order puts the extra register last. An empty list transfers R15 alone
/// (gbatek: "Strange Effects on Invalid Rlist's", ARMv4).
fn register_list(opcode: u32, extra_register: Option<usize>) -> u32 {
    let low_registers = opcode & 0xFF;
    match extra_register {
        Some(register) => low_registers | (1 << register),
        None => low_registers,
    }
}

/// Bytes a register list occupies; an empty list still moves the base by 40h.
fn register_list_size(registers: u32) -> u32 {
    if registers == 0 { 0x40 } else { registers.count_ones() * 4 }
}

/// Store the listed registers at ascending addresses from `start`; an empty list stores R15
/// as $+6 (guess, see `store_multiple_empty_list_value`).
fn store_registers(cpu: &Cpu, bus: &mut Bus, registers: u32, start: u32) {
    let mut address = start & !3;
    if registers == 0 {
        bus.write32(address, store_multiple_empty_list_value(cpu));
    }
    for register in 0..16 {
        if registers & (1 << register) != 0 {
            bus.write32(address, cpu.registers[register]);
            address = address.wrapping_add(4);
        }
    }
}

/// Value stored for R15 by an empty-list STM/PUSH. gbatek does not say; guessed as $+6, one
/// prefetch beyond the usual $+4, mirroring ARM's STM storing $+12.
fn store_multiple_empty_list_value(cpu: &Cpu) -> u32 {
    cpu.registers[PROGRAM_COUNTER].wrapping_add(2)
}

/// Load the listed registers from ascending addresses from `start`; an empty list loads R15.
fn load_registers(cpu: &mut Cpu, bus: &mut Bus, registers: u32, start: u32) {
    let mut address = start & !3;
    if registers == 0 {
        let target = bus.read32(address);
        cpu.branch_to(target);
    }
    for register in 0..16 {
        if registers & (1 << register) != 0 {
            let value = bus.read32(address);
            cpu.write_register(register, value);
            address = address.wrapping_add(4);
        }
    }
}

/// THUMB.14: PUSH {Rlist,LR} (STMDB SP!) and POP {Rlist,PC} (LDMIA SP!). POP PC stays in THUMB.
fn push_pop(cpu: &mut Cpu, bus: &mut Bus, opcode: u32) -> u32 {
    let is_pop = opcode & (1 << 11) != 0;
    let with_extra = opcode & (1 << 8) != 0;
    let stack_pointer = cpu.registers[STACK_POINTER];
    if is_pop {
        let registers = register_list(opcode, with_extra.then_some(PROGRAM_COUNTER));
        load_registers(cpu, bus, registers, stack_pointer);
        cpu.registers[STACK_POINTER] = stack_pointer.wrapping_add(register_list_size(registers));
        1
    } else {
        let registers = register_list(opcode, with_extra.then_some(LINK_REGISTER));
        let start = stack_pointer.wrapping_sub(register_list_size(registers));
        store_registers(cpu, bus, registers, start);
        cpu.registers[STACK_POINTER] = start;
        0
    }
}

/// THUMB.15: STMIA/LDMIA Rb!,{Rlist}, with the ARMv4 base-in-list rules.
fn load_store_multiple(cpu: &mut Cpu, bus: &mut Bus, opcode: u32) -> u32 {
    let base_register = low_register(opcode, 8);
    let base = cpu.registers[base_register];
    let registers = register_list(opcode, None);
    let base_in_list = registers & (1 << base_register) != 0;
    let final_base = base.wrapping_add(register_list_size(registers));
    if opcode & (1 << 11) != 0 {
        load_registers(cpu, bus, registers, base);
        if !base_in_list {
            cpu.registers[base_register] = final_base;
        }
        1
    } else {
        // Rb stores its old value only when it is the lowest (first) register of the list.
        let base_is_first = registers.trailing_zeros() as usize == base_register;
        if base_in_list && !base_is_first {
            cpu.registers[base_register] = final_base;
        }
        store_registers(cpu, bus, registers, base);
        cpu.registers[base_register] = final_base;
        0
    }
}

/// THUMB.16 (Bcond, condition E undefined) and THUMB.17 (SWI, condition F).
fn conditional_branch_or_swi(cpu: &mut Cpu, opcode: u32) -> u32 {
    let condition = (opcode >> 8) & 0xF;
    match condition {
        0xE => undefined(cpu),
        0xF => {
            cpu.enter_exception(Exception::SoftwareInterrupt);
            0
        }
        _ => {
            if cpu.cond_passed(condition) {
                let offset = (opcode as u8 as i8 as i32 as u32) << 1;
                cpu.branch_to(cpu.registers[PROGRAM_COUNTER].wrapping_add(offset));
            }
            0
        }
    }
}

/// Sign-extend the low 11 bits of `opcode`.
fn signed_offset_11(opcode: u32) -> u32 {
    (((opcode << 21) as i32) >> 21) as u32
}

/// THUMB.18: B label.
fn unconditional_branch(cpu: &mut Cpu, opcode: u32) -> u32 {
    cpu.branch_to(cpu.registers[PROGRAM_COUNTER].wrapping_add(signed_offset_11(opcode) << 1));
    0
}

/// THUMB.19 first half: LR = PC + (signed nn << 12).
fn long_branch_first_half(cpu: &mut Cpu, opcode: u32) -> u32 {
    cpu.registers[LINK_REGISTER] = cpu.registers[PROGRAM_COUNTER].wrapping_add(signed_offset_11(opcode) << 12);
    0
}

/// THUMB.19 second half: PC = LR + (nn << 1), LR = next instruction | 1.
fn long_branch_second_half(cpu: &mut Cpu, opcode: u32) -> u32 {
    let target = cpu.registers[LINK_REGISTER].wrapping_add((opcode & 0x7FF) << 1);
    let return_address = cpu.current_instruction_address().wrapping_add(2);
    cpu.registers[LINK_REGISTER] = return_address | 1;
    cpu.branch_to(target);
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::{CPSR_THUMB, MODE_SYSTEM, MODE_SUPERVISOR};

    const CODE: u32 = 0x0300_0000;

    /// A CPU in System mode and THUMB state with `program` placed at IWRAM start.
    fn setup(program: &[u16]) -> (Cpu, Bus) {
        let mut bus = Bus::new(&[]);
        for (index, &opcode) in program.iter().enumerate() {
            bus.write16(CODE + index as u32 * 2, opcode);
        }
        let mut cpu = Cpu::new();
        cpu.set_cpsr(MODE_SYSTEM | CPSR_THUMB);
        cpu.registers[PROGRAM_COUNTER] = CODE;
        cpu.registers[STACK_POINTER] = 0x0300_7F00;
        (cpu, bus)
    }

    fn run(cpu: &mut Cpu, bus: &mut Bus, steps: usize) {
        for _ in 0..steps {
            cpu.step(bus);
        }
    }

    #[test]
    fn shifts_and_immediates() {
        // MOV R0,#0x81 ; LSL R1,R0,#25 ; LSR R2,R0,#0 (=#32) ; ASR R3,R1,#4
        let (mut cpu, mut bus) = setup(&[0x2081, 0x0641, 0x0802, 0x110B]);
        run(&mut cpu, &mut bus, 2);
        assert_eq!(cpu.registers[1], 0x0200_0000);
        assert!(cpu.carry(), "bit 7 shifted out");
        run(&mut cpu, &mut bus, 1);
        assert_eq!(cpu.registers[2], 0);
        assert!(!cpu.carry() && cpu.zero());
        run(&mut cpu, &mut bus, 1);
        assert_eq!(cpu.registers[3], 0x0020_0000);
    }

    #[test]
    fn add_subtract_and_compare() {
        // MOV R0,#1 ; SUB R1,R0,#2 ; CMP R1,#255 ; ADD R2,R1,R0
        let (mut cpu, mut bus) = setup(&[0x2001, 0x1E81, 0x29FF, 0x180A]);
        run(&mut cpu, &mut bus, 2);
        assert_eq!(cpu.registers[1], 0xFFFF_FFFF);
        assert!(cpu.negative() && !cpu.carry());
        run(&mut cpu, &mut bus, 1);
        assert!(cpu.carry() && !cpu.zero());
        run(&mut cpu, &mut bus, 1);
        assert_eq!(cpu.registers[2], 0);
        assert!(cpu.zero() && cpu.carry());
    }

    #[test]
    fn alu_operations() {
        let (mut cpu, mut bus) = setup(&[0x4248, 0x4351, 0x41CA]); // NEG R0,R1 ; MUL R1,R2 ; ROR R2,R1
        cpu.registers[1] = 5;
        cpu.registers[2] = 0x8000_0001;
        run(&mut cpu, &mut bus, 1);
        assert_eq!(cpu.registers[0], (-5i32) as u32);
        assert!(!cpu.carry(), "0 - 5 borrows");
        run(&mut cpu, &mut bus, 1);
        assert_eq!(cpu.registers[1], 0x8000_0005);
        cpu.registers[1] = 36; // ROR by 36 = ROR by 4
        run(&mut cpu, &mut bus, 1);
        assert_eq!(cpu.registers[2], 0x1800_0000);
        assert!(!cpu.carry());
    }

    #[test]
    fn hi_register_ops_and_bx() {
        // MOV R8,R0 ; ADD R0,R8 ; BX R1
        let (mut cpu, mut bus) = setup(&[0x4680, 0x4440, 0x4708]);
        cpu.registers[0] = 3;
        cpu.registers[1] = 0x0800_0100; // ARM target
        run(&mut cpu, &mut bus, 3);
        assert_eq!(cpu.registers[8], 3);
        assert_eq!(cpu.registers[0], 6);
        assert!(!cpu.in_thumb_state());
        assert_eq!(cpu.registers[PROGRAM_COUNTER], 0x0800_0100);
    }

    #[test]
    fn mov_pc_and_add_pc_value() {
        // NOP ; MOV R0,PC (reads $+4 = CODE+6) ; MOV PC,R0
        let (mut cpu, mut bus) = setup(&[0x46C0, 0x4678, 0x4687]);
        run(&mut cpu, &mut bus, 2);
        assert_eq!(cpu.registers[0], CODE + 6);
        run(&mut cpu, &mut bus, 1);
        assert!(cpu.in_thumb_state());
        assert_eq!(cpu.registers[PROGRAM_COUNTER], CODE + 6);
    }

    #[test]
    fn pc_relative_load_and_address_align_pc() {
        // NOP ; LDR R0,[PC,#4] ; ADD R1,PC,#4 ; ...data at CODE+8
        let (mut cpu, mut bus) = setup(&[0x46C0, 0x4801, 0xA101, 0x0000, 0x5678, 0x1234]);
        run(&mut cpu, &mut bus, 3);
        // At CODE+2: PC = CODE+6, aligned to CODE+4, +4 = CODE+8.
        assert_eq!(cpu.registers[0], 0x1234_5678);
        // At CODE+4: PC = CODE+8, +4 = CODE+12.
        assert_eq!(cpu.registers[1], CODE + 12);
    }

    #[test]
    fn misaligned_loads() {
        // LDR R0,[R4,#0] ; LDRH R1,[R4,#0] ; LDSH R2,[R4,R5]
        let (mut cpu, mut bus) = setup(&[0x6820, 0x8821, 0x5F62]);
        bus.write32(0x0300_1000, 0x8899_AABB);
        cpu.registers[4] = 0x0300_1001;
        cpu.registers[5] = 0;
        run(&mut cpu, &mut bus, 3);
        assert_eq!(cpu.registers[0], 0xBB88_99AA);
        assert_eq!(cpu.registers[1], 0xBB00_00AA);
        assert_eq!(cpu.registers[2], 0xFFFF_FFAA, "odd LDSH acts as LDSB");
    }

    #[test]
    fn push_pop_round_trip() {
        // PUSH {R0,R1,LR} ; POP {R2,R3,PC}
        let (mut cpu, mut bus) = setup(&[0xB503, 0xBD0C]);
        cpu.registers[0] = 10;
        cpu.registers[1] = 20;
        cpu.registers[LINK_REGISTER] = 0x0300_0101;
        run(&mut cpu, &mut bus, 1);
        assert_eq!(cpu.registers[STACK_POINTER], 0x0300_7EF4);
        assert_eq!(bus.read32(0x0300_7EFC), 0x0300_0101);
        run(&mut cpu, &mut bus, 1);
        assert_eq!((cpu.registers[2], cpu.registers[3]), (10, 20));
        assert_eq!(cpu.registers[STACK_POINTER], 0x0300_7F00);
        assert_eq!(cpu.registers[PROGRAM_COUNTER], 0x0300_0100);
        assert!(cpu.in_thumb_state());
    }

    #[test]
    fn stmia_and_ldmia_base_in_list() {
        // STMIA R0!,{R0,R1} (old base) ; STMIA R1!,{R0,R1} (new base) ; LDMIA R2!,{R1,R2}
        let (mut cpu, mut bus) = setup(&[0xC003, 0xC103, 0xCA06]);
        cpu.registers[0] = 0x0300_1000;
        cpu.registers[1] = 0x0300_2000;
        run(&mut cpu, &mut bus, 1);
        assert_eq!(bus.read32(0x0300_1000), 0x0300_1000);
        assert_eq!(cpu.registers[0], 0x0300_1008);
        run(&mut cpu, &mut bus, 1);
        assert_eq!(bus.read32(0x0300_2004), 0x0300_2008);
        cpu.registers[2] = 0x0300_2000;
        run(&mut cpu, &mut bus, 1);
        assert_eq!(cpu.registers[1], 0x0300_1008);
        assert_eq!(cpu.registers[2], 0x0300_2008, "loaded value, no writeback");
    }

    #[test]
    fn empty_register_list() {
        let (mut cpu, mut bus) = setup(&[0xC000, 0xC800]); // STMIA R0!,{} ; LDMIA R0!,{}
        cpu.registers[0] = 0x0300_1000;
        run(&mut cpu, &mut bus, 1);
        assert_eq!(cpu.registers[0], 0x0300_1040);
        assert_eq!(bus.read32(0x0300_1000), CODE + 6);
        bus.write32(0x0300_1040, 0x0300_0201);
        run(&mut cpu, &mut bus, 1);
        assert_eq!(cpu.registers[0], 0x0300_1080);
        assert_eq!(cpu.registers[PROGRAM_COUNTER], 0x0300_0200);
    }

    #[test]
    fn branches() {
        // MOV R0,#0 ; BEQ CODE+8 (taken) ; two skipped opcodes ; B . at CODE+8
        let (mut cpu, mut bus) = setup(&[0x2000, 0xD001, 0x0000, 0x0000, 0xE7FE]);
        run(&mut cpu, &mut bus, 2);
        assert_eq!(cpu.registers[PROGRAM_COUNTER], CODE + 8);
        run(&mut cpu, &mut bus, 1);
        assert_eq!(cpu.registers[PROGRAM_COUNTER], CODE + 8, "B . loops");
    }

    #[test]
    fn long_branch_with_link() {
        // BL CODE+0x1004: offset from $+4 is 0x1000
        let (mut cpu, mut bus) = setup(&[0xF001, 0xF800]);
        run(&mut cpu, &mut bus, 2);
        assert_eq!(cpu.registers[PROGRAM_COUNTER], CODE + 0x1004);
        assert_eq!(cpu.registers[LINK_REGISTER], (CODE + 4) | 1);
        // Backwards: BL CODE+0 from CODE: offset -4 -> F7FF FFFE
        let (mut cpu, mut bus) = setup(&[0xF7FF, 0xFFFE]);
        run(&mut cpu, &mut bus, 2);
        assert_eq!(cpu.registers[PROGRAM_COUNTER], CODE);
    }

    #[test]
    fn software_interrupt() {
        let (mut cpu, mut bus) = setup(&[0x46C0, 0xDF05]);
        run(&mut cpu, &mut bus, 2);
        assert_eq!(cpu.mode(), MODE_SUPERVISOR);
        assert_eq!(cpu.registers[LINK_REGISTER], CODE + 4);
        assert_eq!(cpu.registers[PROGRAM_COUNTER], 0x08);
    }
}
