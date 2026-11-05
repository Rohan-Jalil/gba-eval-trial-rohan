//! ARM7TDMI core: register file, modes, CPSR/SPSR, exceptions, step()
//! (gbatek: ARM CPU Register Set, ARM CPU Flags & Condition Field, ARM CPU Exceptions).
//!
//! Phase 0 stub: holds the reset state and burns one cycle per step. The Phase 1 CPU core agent
//! adds banking, exceptions and dispatch to arm.rs / thumb.rs.

#![allow(dead_code)] // fields and constants are used once Phase 1 lands

mod alu;
mod arm;
mod thumb;

use crate::bus::Bus;

/// CPSR mode bits 0-4 (gbatek: ARM CPU Register Set).
pub const MODE_USER: u32 = 0x10;
pub const MODE_FIQ: u32 = 0x11;
pub const MODE_IRQ: u32 = 0x12;
pub const MODE_SUPERVISOR: u32 = 0x13;
pub const MODE_ABORT: u32 = 0x17;
pub const MODE_UNDEFINED: u32 = 0x1B;
pub const MODE_SYSTEM: u32 = 0x1F;

pub const CPSR_THUMB: u32 = 1 << 5; // T: 0 = ARM, 1 = THUMB state
pub const CPSR_FIQ_DISABLE: u32 = 1 << 6; // F
pub const CPSR_IRQ_DISABLE: u32 = 1 << 7; // I

pub const PROGRAM_COUNTER: usize = 15; // R15
pub const LINK_REGISTER: usize = 14; // R14
pub const STACK_POINTER: usize = 13; // R13

pub struct Cpu {
    /// R0-R15 of the current mode.
    pub registers: [u32; 16],
    /// Current program status register.
    pub cpsr: u32,
    /// Saved program status register of the current mode.
    pub spsr: u32,
}

impl Cpu {
    /// Reset state: Supervisor mode, ARM state, IRQ and FIQ disabled, PC at the reset vector 0.
    pub fn new() -> Self {
        Cpu {
            registers: [0; 16],
            cpsr: MODE_SUPERVISOR | CPSR_IRQ_DISABLE | CPSR_FIQ_DISABLE,
            spsr: 0,
        }
    }

    /// Execute one instruction (or one idle halt slice) and return the cycles it used,
    /// including the bus access cycles from `bus.take_access_cycles()`. Stub: 1 cycle.
    pub fn step(&mut self, bus: &mut Bus) -> u32 {
        1 + bus.take_access_cycles()
    }
}
