//! Four hardware timers, IO 0x100-0x10F (gbatek: GBA Timers).
//!
//! Phase 0 stub: registers are stored; counting, prescaler, cascade and overflow IRQs come in Phase 2.

use crate::io::PlainRegisters;
use crate::irq::Irq;

const IO_START: u32 = 0x100; // TM0CNT_L, first timer register

pub struct Timers {
    registers: PlainRegisters<8>,
}

impl Timers {
    pub fn new() -> Self {
        Timers { registers: PlainRegisters::new(IO_START) }
    }

    /// Advance all timers by `cycles`. Returns a bit mask of timers that overflowed (bit 0 = timer 0),
    /// which gba.rs forwards to the APU for FIFO ticks. Overflow IRQs are requested on `irq`.
    pub fn step(&mut self, _cycles: u32, _irq: &mut Irq) -> u8 {
        0
    }

    pub fn io_read16(&mut self, offset: u32) -> u16 {
        self.registers.read(offset)
    }

    pub fn io_write16(&mut self, offset: u32, value: u16, mask: u16) {
        self.registers.write(offset, value, mask)
    }
}
