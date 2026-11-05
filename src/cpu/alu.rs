//! Barrel shifter, add/subtract with flags and condition evaluation
//! (gbatek: ARM CPU Flags & Condition Field, ARM Opcodes: Data Processing (ALU)).
//!
//! Everything here is a pure function of its inputs, so the ARM and THUMB decoders share it.

/// CPSR condition flag bits 28-31 (gbatek: ARM CPU Flags & Condition Field).
pub const FLAG_NEGATIVE: u32 = 1 << 31; // N: result bit 31
pub const FLAG_ZERO: u32 = 1 << 30; // Z: result was zero
pub const FLAG_CARRY: u32 = 1 << 29; // C: carry out / no borrow
pub const FLAG_OVERFLOW: u32 = 1 << 28; // V: signed overflow

/// Shift type from opcode bits 5-6 (ARM) or the THUMB shift opcodes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShiftType {
    LogicalLeft,      // LSL
    LogicalRight,     // LSR
    ArithmeticRight,  // ASR
    RotateRight,      // ROR (RRX when the immediate amount is 0)
}

impl ShiftType {
    /// Decode the 2-bit shift type field (0=LSL, 1=LSR, 2=ASR, 3=ROR).
    pub fn from_bits(bits: u32) -> Self {
        match bits & 3 {
            0 => ShiftType::LogicalLeft,
            1 => ShiftType::LogicalRight,
            2 => ShiftType::ArithmeticRight,
            _ => ShiftType::RotateRight,
        }
    }
}

/// A shifted value and the shifter's carry-out.
pub type ShiftResult = (u32, bool);

/// Shift by an amount of 1-255, the rules for a register-specified shift
/// (gbatek: ARM Opcodes: Data Processing (ALU), "Shift by Register").
fn shift_by_nonzero_amount(shift_type: ShiftType, value: u32, amount: u32) -> ShiftResult {
    let bit = |index: u32| (value >> index) & 1 != 0;
    match shift_type {
        ShiftType::LogicalLeft => match amount {
            1..=31 => (value << amount, bit(32 - amount)),
            32 => (0, bit(0)),
            _ => (0, false),
        },
        ShiftType::LogicalRight => match amount {
            1..=31 => (value >> amount, bit(amount - 1)),
            32 => (0, bit(31)),
            _ => (0, false),
        },
        ShiftType::ArithmeticRight => match amount {
            1..=31 => (((value as i32) >> amount) as u32, bit(amount - 1)),
            _ => (((value as i32) >> 31) as u32, bit(31)),
        },
        ShiftType::RotateRight => {
            let rotation = amount % 32;
            if rotation == 0 {
                (value, bit(31))
            } else {
                (value.rotate_right(rotation), bit(rotation - 1))
            }
        }
    }
}

/// Shift by a 5-bit immediate amount, where amount 0 means LSL#0 (no shift, carry unchanged),
/// LSR#32, ASR#32 or RRX (gbatek: ARM Opcodes: Data Processing (ALU), "Zero Shift Amount").
pub fn shift_by_immediate(shift_type: ShiftType, value: u32, amount: u32, carry_in: bool) -> ShiftResult {
    let amount = amount & 31;
    if amount != 0 {
        return shift_by_nonzero_amount(shift_type, value, amount);
    }
    match shift_type {
        ShiftType::LogicalLeft => (value, carry_in),
        ShiftType::LogicalRight | ShiftType::ArithmeticRight => shift_by_nonzero_amount(shift_type, value, 32),
        ShiftType::RotateRight => rotate_right_extended(value, carry_in),
    }
}

/// RRX: rotate right by one through the carry flag.
fn rotate_right_extended(value: u32, carry_in: bool) -> ShiftResult {
    (((carry_in as u32) << 31) | (value >> 1), value & 1 != 0)
}

/// Shift by the low 8 bits of a register; amount 0 leaves value and carry unchanged
/// (gbatek: ARM Opcodes: Data Processing (ALU), "Shift by Register").
pub fn shift_by_register(shift_type: ShiftType, value: u32, register_value: u32, carry_in: bool) -> ShiftResult {
    let amount = register_value & 0xFF;
    if amount == 0 {
        (value, carry_in)
    } else {
        shift_by_nonzero_amount(shift_type, value, amount)
    }
}

/// ARM immediate operand: 8-bit value rotated right by twice the 4-bit rotate field; a zero
/// rotation leaves carry unchanged (gbatek: ARM Opcodes: Data Processing (ALU), I=1).
pub fn rotated_immediate(immediate: u32, rotate_field: u32, carry_in: bool) -> ShiftResult {
    let rotation = (rotate_field & 0xF) * 2;
    if rotation == 0 {
        (immediate & 0xFF, carry_in)
    } else {
        let value = (immediate & 0xFF).rotate_right(rotation);
        (value, value >> 31 != 0)
    }
}

/// Result of an addition or subtraction with its C and V flags.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ArithmeticResult {
    pub value: u32,
    pub carry: bool,
    pub overflow: bool,
}

/// `first + second + carry_in` with unsigned carry-out and signed overflow (ADD, ADC, CMN).
pub fn add_with_carry(first: u32, second: u32, carry_in: bool) -> ArithmeticResult {
    let wide_sum = first as u64 + second as u64 + carry_in as u64;
    let value = wide_sum as u32;
    // Overflow: both operands have the same sign and the result's sign differs.
    let overflow = (!(first ^ second) & (first ^ value)) >> 31 != 0;
    ArithmeticResult { value, carry: wide_sum > u32::MAX as u64, overflow }
}

/// `first - second - (1 - carry_in)`; carry is "no borrow" as on ARM (SUB/CMP pass true, SBC
/// passes C; RSB/RSC swap the operands). Computed as `first + NOT second + carry_in`.
pub fn subtract_with_carry(first: u32, second: u32, carry_in: bool) -> ArithmeticResult {
    add_with_carry(first, !second, carry_in)
}

/// `first - second` (SUB, CMP, THUMB NEG as `0 - x`).
pub fn subtract(first: u32, second: u32) -> ArithmeticResult {
    subtract_with_carry(first, second, true)
}

/// `first + second` (ADD, CMN).
pub fn add(first: u32, second: u32) -> ArithmeticResult {
    add_with_carry(first, second, false)
}

/// Whether condition field `condition` (0-15) passes for the flags in `cpsr`
/// (gbatek: ARM CPU Flags & Condition Field (cond)). NV (15) never passes.
pub fn condition_passed(condition: u32, cpsr: u32) -> bool {
    let negative = cpsr & FLAG_NEGATIVE != 0;
    let zero = cpsr & FLAG_ZERO != 0;
    let carry = cpsr & FLAG_CARRY != 0;
    let overflow = cpsr & FLAG_OVERFLOW != 0;
    match condition & 0xF {
        0x0 => zero,                          // EQ
        0x1 => !zero,                         // NE
        0x2 => carry,                         // CS/HS
        0x3 => !carry,                        // CC/LO
        0x4 => negative,                      // MI
        0x5 => !negative,                     // PL
        0x6 => overflow,                      // VS
        0x7 => !overflow,                     // VC
        0x8 => carry && !zero,                // HI
        0x9 => !carry || zero,                // LS
        0xA => negative == overflow,          // GE
        0xB => negative != overflow,          // LT
        0xC => !zero && negative == overflow, // GT
        0xD => zero || negative != overflow,  // LE
        0xE => true,                          // AL
        _ => false,                           // NV
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ShiftType::*;

    const N: u32 = FLAG_NEGATIVE;
    const Z: u32 = FLAG_ZERO;
    const C: u32 = FLAG_CARRY;
    const V: u32 = FLAG_OVERFLOW;

    #[test]
    fn every_condition_code_against_every_flag_combination() {
        for flags in 0..16u32 {
            let cpsr = flags << 28;
            let (negative, zero, carry, overflow) =
                (cpsr & N != 0, cpsr & Z != 0, cpsr & C != 0, cpsr & V != 0);
            let expected = [
                zero,
                !zero,
                carry,
                !carry,
                negative,
                !negative,
                overflow,
                !overflow,
                carry && !zero,
                !carry || zero,
                negative == overflow,
                negative != overflow,
                !zero && negative == overflow,
                zero || negative != overflow,
                true,
                false,
            ];
            for condition in 0..16 {
                assert_eq!(condition_passed(condition, cpsr), expected[condition as usize], "cond {condition:X} flags {flags:04b}");
            }
        }
        // Spot checks written independently of the table above.
        assert!(condition_passed(0x8, C)); // HI: C=1, Z=0
        assert!(!condition_passed(0x8, C | Z));
        assert!(condition_passed(0xA, N | V)); // GE: N=V=1
        assert!(condition_passed(0xB, N)); // LT: N!=V
        assert!(!condition_passed(0xC, Z)); // GT fails on Z
        assert!(condition_passed(0xD, V)); // LE: N!=V
    }

    #[test]
    fn immediate_shift_by_zero_is_special() {
        let value = 0x8000_0001;
        assert_eq!(shift_by_immediate(LogicalLeft, value, 0, true), (value, true));
        assert_eq!(shift_by_immediate(LogicalLeft, value, 0, false), (value, false));
        assert_eq!(shift_by_immediate(LogicalRight, value, 0, false), (0, true)); // LSR#32
        assert_eq!(shift_by_immediate(ArithmeticRight, value, 0, false), (0xFFFF_FFFF, true)); // ASR#32
        assert_eq!(shift_by_immediate(ArithmeticRight, 0x7FFF_FFFF, 0, true), (0, false));
        assert_eq!(shift_by_immediate(RotateRight, value, 0, false), (0x4000_0000, true)); // RRX
        assert_eq!(shift_by_immediate(RotateRight, value, 0, true), (0xC000_0000, true));
        assert_eq!(shift_by_immediate(RotateRight, 2, 0, true), (0x8000_0001, false));
    }

    #[test]
    fn immediate_shift_by_one_and_thirty_one() {
        let value = 0xC000_0003;
        assert_eq!(shift_by_immediate(LogicalLeft, value, 1, false), (0x8000_0006, true));
        assert_eq!(shift_by_immediate(LogicalLeft, value, 31, false), (0x8000_0000, true));
        assert_eq!(shift_by_immediate(LogicalLeft, 2, 31, true), (0, true)); // bit 1 shifted out
        assert_eq!(shift_by_immediate(LogicalRight, value, 1, false), (0x6000_0001, true));
        assert_eq!(shift_by_immediate(LogicalRight, value, 31, false), (1, true));
        assert_eq!(shift_by_immediate(ArithmeticRight, value, 1, false), (0xE000_0001, true));
        assert_eq!(shift_by_immediate(ArithmeticRight, value, 31, false), (0xFFFF_FFFF, true));
        assert_eq!(shift_by_immediate(RotateRight, value, 1, false), (0xE000_0001, true));
        assert_eq!(shift_by_immediate(RotateRight, value, 31, false), (0x8000_0007, true));
        assert_eq!(shift_by_immediate(RotateRight, 0x4, 31, true), (0x8, false));
    }

    #[test]
    fn register_shift_by_zero_keeps_value_and_carry() {
        for shift_type in [LogicalLeft, LogicalRight, ArithmeticRight, RotateRight] {
            assert_eq!(shift_by_register(shift_type, 0x8000_0001, 0, true), (0x8000_0001, true));
            assert_eq!(shift_by_register(shift_type, 0x8000_0001, 0x100, false), (0x8000_0001, false)); // only low 8 bits
        }
    }

    #[test]
    fn register_shift_by_one_and_thirty_one() {
        assert_eq!(shift_by_register(LogicalLeft, 0x8000_0001, 1, false), (2, true));
        assert_eq!(shift_by_register(LogicalLeft, 3, 31, false), (0x8000_0000, true));
        assert_eq!(shift_by_register(LogicalRight, 0x8000_0001, 1, false), (0x4000_0000, true));
        assert_eq!(shift_by_register(LogicalRight, 0x8000_0001, 31, true), (1, false));
        assert_eq!(shift_by_register(ArithmeticRight, 0x8000_0000, 31, false), (0xFFFF_FFFF, false));
        assert_eq!(shift_by_register(RotateRight, 0x8000_0001, 1, false), (0xC000_0000, true)); // ROR#1, not RRX
        assert_eq!(shift_by_register(RotateRight, 0x8000_0001, 31, false), (0x0000_0003, false));
    }

    #[test]
    fn register_shift_by_thirty_two() {
        assert_eq!(shift_by_register(LogicalLeft, 0x0000_0001, 32, false), (0, true));
        assert_eq!(shift_by_register(LogicalLeft, 0xFFFF_FFFE, 32, true), (0, false));
        assert_eq!(shift_by_register(LogicalRight, 0x8000_0000, 32, false), (0, true));
        assert_eq!(shift_by_register(LogicalRight, 0x7FFF_FFFF, 32, true), (0, false));
        assert_eq!(shift_by_register(ArithmeticRight, 0x8000_0000, 32, false), (0xFFFF_FFFF, true));
        assert_eq!(shift_by_register(ArithmeticRight, 0x7FFF_FFFF, 32, true), (0, false));
        assert_eq!(shift_by_register(RotateRight, 0x8000_0001, 32, false), (0x8000_0001, true));
        assert_eq!(shift_by_register(RotateRight, 0x7FFF_FFFF, 32, true), (0x7FFF_FFFF, false));
    }

    #[test]
    fn register_shift_by_thirty_three_and_more() {
        assert_eq!(shift_by_register(LogicalLeft, 0xFFFF_FFFF, 33, true), (0, false));
        assert_eq!(shift_by_register(LogicalRight, 0xFFFF_FFFF, 33, true), (0, false));
        assert_eq!(shift_by_register(ArithmeticRight, 0x8000_0000, 33, false), (0xFFFF_FFFF, true));
        assert_eq!(shift_by_register(ArithmeticRight, 0x4000_0000, 200, true), (0, false));
        assert_eq!(shift_by_register(RotateRight, 0x8000_0001, 33, false), (0xC000_0000, true)); // as ROR#1
        assert_eq!(shift_by_register(RotateRight, 0x0000_0010, 64, false), (0x10, false)); // multiple of 32
        assert_eq!(shift_by_register(LogicalLeft, 1, 255, true), (0, false));
    }

    #[test]
    fn rotated_immediate_operand() {
        assert_eq!(rotated_immediate(0xFF, 0, true), (0xFF, true));
        assert_eq!(rotated_immediate(0xFF, 0, false), (0xFF, false));
        assert_eq!(rotated_immediate(0xFF, 4, false), (0xFF00_0000, true)); // ROR 8
        assert_eq!(rotated_immediate(0x01, 1, false), (0x4000_0000, false)); // ROR 2
        assert_eq!(rotated_immediate(0x02, 1, true), (0x8000_0000, true));
    }

    #[test]
    fn add_carry_and_overflow() {
        assert_eq!(add(1, 2), ArithmeticResult { value: 3, carry: false, overflow: false });
        assert_eq!(add(0xFFFF_FFFF, 1), ArithmeticResult { value: 0, carry: true, overflow: false });
        assert_eq!(add(0x7FFF_FFFF, 1), ArithmeticResult { value: 0x8000_0000, carry: false, overflow: true });
        assert_eq!(add(0x8000_0000, 0x8000_0000), ArithmeticResult { value: 0, carry: true, overflow: true });
        assert_eq!(add(0xFFFF_FFFF, 0xFFFF_FFFF), ArithmeticResult { value: 0xFFFF_FFFE, carry: true, overflow: false });
    }

    #[test]
    fn add_with_carry_in() {
        assert_eq!(add_with_carry(1, 2, true).value, 4);
        assert_eq!(add_with_carry(0xFFFF_FFFF, 0, true), ArithmeticResult { value: 0, carry: true, overflow: false });
        assert_eq!(add_with_carry(0xFFFF_FFFF, 0xFFFF_FFFF, true), ArithmeticResult { value: 0xFFFF_FFFF, carry: true, overflow: false });
        assert_eq!(add_with_carry(0x7FFF_FFFF, 0, true), ArithmeticResult { value: 0x8000_0000, carry: false, overflow: true });
        assert_eq!(add_with_carry(0x7FFF_FFFF, 0x7FFF_FFFF, true), ArithmeticResult { value: 0xFFFF_FFFF, carry: false, overflow: true });
    }

    #[test]
    fn subtract_carry_means_no_borrow_and_overflow() {
        assert_eq!(subtract(5, 3), ArithmeticResult { value: 2, carry: true, overflow: false });
        assert_eq!(subtract(3, 3), ArithmeticResult { value: 0, carry: true, overflow: false });
        assert_eq!(subtract(3, 5), ArithmeticResult { value: 0xFFFF_FFFE, carry: false, overflow: false });
        assert_eq!(subtract(0x8000_0000, 1), ArithmeticResult { value: 0x7FFF_FFFF, carry: true, overflow: true });
        assert_eq!(subtract(0x7FFF_FFFF, 0xFFFF_FFFF), ArithmeticResult { value: 0x8000_0000, carry: false, overflow: true });
        assert_eq!(subtract(0, 0), ArithmeticResult { value: 0, carry: true, overflow: false }); // CMP 0,0 sets C
        assert_eq!(subtract(0, 0x8000_0000), ArithmeticResult { value: 0x8000_0000, carry: false, overflow: true }); // NEG of INT_MIN
    }

    #[test]
    fn subtract_with_carry_in() {
        // SBC with C=1 behaves like SUB.
        assert_eq!(subtract_with_carry(5, 3, true), subtract(5, 3));
        // SBC with C=0 subtracts one more.
        assert_eq!(subtract_with_carry(5, 3, false), ArithmeticResult { value: 1, carry: true, overflow: false });
        assert_eq!(subtract_with_carry(3, 3, false), ArithmeticResult { value: 0xFFFF_FFFF, carry: false, overflow: false });
        assert_eq!(subtract_with_carry(0, 0, false), ArithmeticResult { value: 0xFFFF_FFFF, carry: false, overflow: false });
        assert_eq!(subtract_with_carry(0x8000_0000, 0, false), ArithmeticResult { value: 0x7FFF_FFFF, carry: true, overflow: true });
        assert_eq!(subtract_with_carry(0x8000_0001, 0, false), ArithmeticResult { value: 0x8000_0000, carry: true, overflow: false });
    }
}
