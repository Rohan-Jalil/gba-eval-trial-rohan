//! Interrupt controller and system-control IO range 0x200-0x804 (gbatek: GBA Interrupt Control).
//!
//! Any component raises an interrupt with `irq.request(IrqSource::...)`. Only the CPU consumes them,
//! through `pending()`.

use crate::io::{merge, PlainRegisters};

const IE: u32 = 0x200; // interrupt enable
const IF: u32 = 0x202; // interrupt request flags / acknowledge
const IME: u32 = 0x208; // interrupt master enable
const POSTFLG_HALTCNT: u32 = 0x300; // low byte POSTFLG (post-boot flag), high byte HALTCNT (low-power mode)

/// First IO offset owned by this module.
pub const IO_START: u32 = 0x200;
/// Last IO offset owned by this module (inclusive; 0x800 is the internal memory control register).
pub const IO_END: u32 = 0x804;

/// IE/IF bit 14-15 are unused.
const VALID_IRQ_BITS: u16 = 0x3FFF;

/// Interrupt sources, numbered by their IE/IF bit (gbatek: GBA Interrupt Control).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrqSource {
    VBlank = 0,
    HBlank = 1,
    VCounterMatch = 2,
    Timer0 = 3,
    Timer1 = 4,
    Timer2 = 5,
    Timer3 = 6,
    Serial = 7,
    Dma0 = 8,
    Dma1 = 9,
    Dma2 = 10,
    Dma3 = 11,
    Keypad = 12,
    GamePak = 13,
}

pub struct Irq {
    pub interrupt_enable: u16,
    pub interrupt_flags: u16,
    pub master_enable: bool,
    /// Set by a write to HALTCNT; the CPU clears it when an enabled interrupt wakes it.
    pub halt_requested: bool,
    /// Plain storage for the rest of 0x200-0x804 (WAITCNT, POSTFLG, ...), indexed by halfword.
    other_registers: PlainRegisters<{ ((IO_END - IO_START) / 2 + 1) as usize }>,
}

impl Irq {
    pub fn new() -> Self {
        Irq {
            interrupt_enable: 0,
            interrupt_flags: 0,
            master_enable: false,
            halt_requested: false,
            other_registers: PlainRegisters::new(IO_START),
        }
    }

    /// Raise an interrupt request flag in IF.
    pub fn request(&mut self, source: IrqSource) {
        self.interrupt_flags |= 1 << source as u16;
    }

    /// True when IME is on and some interrupt is both enabled (IE) and requested (IF).
    /// The CPSR I bit is the CPU's business.
    pub fn pending(&self) -> bool {
        self.master_enable && (self.interrupt_enable & self.interrupt_flags & VALID_IRQ_BITS) != 0
    }

    pub fn io_read16(&mut self, offset: u32) -> u16 {
        match offset {
            IE => self.interrupt_enable,
            IF => self.interrupt_flags,
            IME => self.master_enable as u16,
            _ => self.other_registers.read(offset),
        }
    }

    /// `mask` marks the bytes actually written (0x00FF, 0xFF00 or 0xFFFF); see io.rs.
    pub fn io_write16(&mut self, offset: u32, value: u16, mask: u16) {
        let written = value & mask;
        match offset {
            IE => self.interrupt_enable = merge(self.interrupt_enable, value, mask) & VALID_IRQ_BITS,
            // Writing 1 to an IF bit acknowledges (clears) it.
            IF => self.interrupt_flags &= !written,
            IME => self.master_enable = merge(self.master_enable as u16, value, mask) & 1 != 0,
            _ => {
                if offset == POSTFLG_HALTCNT && mask & 0xFF00 != 0 {
                    // ponytail: HALTCNT bit 7 (stop mode) is treated as halt too; add stop when a ROM needs it.
                    self.halt_requested = true;
                }
                self.other_registers.write(offset, value, mask);
            }
        }
    }
}
