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
    /// Set by the CPU before each instruction: true while it executes BIOS code (0x0000-0x3FFF).
    /// POSTFLG and HALTCNT only accept writes from BIOS code.
    pub executing_bios: bool,
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
            executing_bios: false,
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
        self.master_enable && self.wake_condition()
    }

    /// True when some interrupt is both enabled (IE) and requested (IF), ignoring IME.
    /// This is what ends Halt (gbatek: GBA System Control, HALTCNT).
    pub fn wake_condition(&self) -> bool {
        (self.interrupt_enable & self.interrupt_flags & VALID_IRQ_BITS) != 0
    }

    pub fn io_read16(&mut self, offset: u32) -> u16 {
        match offset {
            IE => self.interrupt_enable,
            IF => self.interrupt_flags,
            IME => self.master_enable as u16,
            // POSTFLG bit 0 is set by the real BIOS at boot and cannot be cleared afterwards, so it
            // always reads 1 here (the stub boot skips setting it). HALTCNT is write-only and reads 0.
            POSTFLG_HALTCNT => 1,
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
            POSTFLG_HALTCNT => self.write_postflg_haltcnt(mask),
            _ => self.other_registers.write(offset, value, mask),
        }
    }

    /// HALTCNT write (gbatek: GBA System Control). Writes from outside the BIOS are ignored, and
    /// POSTFLG writes change nothing (bit 0 is sticky, see `io_read16`). gbatek states both rules
    /// only for the NDS7 ports; nba-hw/haltcnt.gba shows the GBA behaves the same.
    fn write_postflg_haltcnt(&mut self, mask: u16) {
        if self.executing_bios && mask & 0xFF00 != 0 {
            // ponytail: HALTCNT bit 7 (Stop) is treated as Halt: video and sound keep running and any
            // enabled IRQ wakes the CPU, not only Keypad/GamePak/Serial. Add real Stop if a ROM needs it.
            self.halt_requested = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HALTCNT_BYTE: u16 = 0xFF00;

    #[test]
    fn if_is_acknowledged_by_writing_one() {
        let mut irq = Irq::new();
        irq.request(IrqSource::VBlank);
        irq.request(IrqSource::Timer0);
        irq.io_write16(IF, 1 << IrqSource::VBlank as u16, 0xFFFF);
        assert_eq!(irq.interrupt_flags, 1 << IrqSource::Timer0 as u16);
    }

    #[test]
    fn pending_needs_ime_but_wake_does_not() {
        let mut irq = Irq::new();
        irq.io_write16(IE, 1, 0xFFFF);
        irq.request(IrqSource::VBlank);
        assert!(irq.wake_condition() && !irq.pending());
        irq.io_write16(IME, 1, 0xFFFF);
        assert!(irq.pending());
    }

    #[test]
    fn haltcnt_only_halts_from_bios_and_postflg_reads_one() {
        let mut irq = Irq::new();
        irq.io_write16(POSTFLG_HALTCNT, 0, HALTCNT_BYTE);
        assert!(!irq.halt_requested, "write from outside the BIOS is ignored");
        irq.executing_bios = true;
        irq.io_write16(POSTFLG_HALTCNT, 0, 0x00FF);
        assert!(!irq.halt_requested, "a POSTFLG-only write does not halt");
        irq.io_write16(POSTFLG_HALTCNT, 0, HALTCNT_BYTE);
        assert!(irq.halt_requested);
        assert_eq!(irq.io_read16(POSTFLG_HALTCNT), 1);
    }
}
