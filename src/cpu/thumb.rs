//! THUMB (16-bit) instruction decode and execute (gbatek: THUMB Instruction Summary and THUMB Opcodes).
//!
//! Phase 0 stub; filled in by the Phase 1 THUMB agent.

use super::Cpu;
use crate::bus::Bus;

/// Execute one THUMB opcode. R15 reads as the opcode's address + 4. Returns internal cycles only;
/// bus accesses are counted by the bus. Stub until the Phase 1 THUMB agent lands: does nothing, 1 cycle.
pub(super) fn execute_thumb(_cpu: &mut Cpu, _bus: &mut Bus, _opcode: u16) -> u32 {
    1
}
