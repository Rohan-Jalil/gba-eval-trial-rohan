//! Four DMA channels, IO 0x0B0-0x0DF (gbatek: GBA DMA Transfers).
//!
//! Phase 0 stub: registers are stored and trigger events are accepted; transfers come in Phase 2.
//! DMA lives inside the `Bus`, so the transfer itself is a free function taking the whole bus.

use crate::bus::Bus;
use crate::io::PlainRegisters;

const IO_START: u32 = 0x0B0; // DMA0SAD, first DMA register

pub struct Dma {
    registers: PlainRegisters<0x18>,
}

impl Dma {
    pub fn new() -> Self {
        Dma { registers: PlainRegisters::new(IO_START) }
    }

    /// PPU entered HBlank on a visible line (start-timing mode 2).
    pub fn notify_hblank(&mut self) {}

    /// PPU entered VBlank (start-timing mode 1).
    pub fn notify_vblank(&mut self) {}

    /// APU sound FIFO A (0) or B (1) wants more samples (start-timing mode 3 on DMA1/DMA2).
    pub fn notify_fifo_request(&mut self, _fifo_index: usize) {}

    pub fn io_read16(&mut self, offset: u32) -> u16 {
        self.registers.read(offset)
    }

    pub fn io_write16(&mut self, offset: u32, value: u16, mask: u16) {
        self.registers.write(offset, value, mask)
    }
}

/// Run any DMA transfer that is due, using the bus for the copies.
/// Returns the cycles the transfers took (the CPU is stalled for that time).
pub fn run_pending_transfers(_bus: &mut Bus) -> u32 {
    0
}
