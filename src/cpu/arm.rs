//! ARM (32-bit) instruction decode and execute (gbatek: ARM Instruction Summary and ARM Opcodes).
//!
//! Phase 0 stub; filled in by the Phase 1 ARM agent.

use super::Cpu;
use crate::bus::Bus;

/// Execute one ARM opcode whose condition already passed (checked in `Cpu::step`). R15 reads as
/// the opcode's address + 8. Returns internal cycles only; bus accesses are counted by the bus.
/// Stub until the Phase 1 ARM agent lands: does nothing, 1 cycle.
pub(super) fn execute_arm(_cpu: &mut Cpu, _bus: &mut Bus, _opcode: u32) -> u32 {
    1
}
