//! IO register dispatch: maps an offset inside 0x04000000 to the component that owns it
//! (gbatek: GBA I/O Map).
//!
//! Every owner exposes `io_read16(offset) -> u16` and `io_write16(offset, value, mask)`, where
//! `offset` is the halfword-aligned offset from 0x04000000 and `mask` marks the bytes really written
//! (0x00FF / 0xFF00 for 8-bit writes, 0xFFFF otherwise). 8- and 32-bit accesses are split or merged
//! here, so owners only ever see halfwords. The mask matters for registers where a read-modify-write
//! would be wrong: IF acknowledge, HALTCNT, the sound FIFOs, write-only registers.

use crate::bus::Bus;

/// Owner ranges, inclusive (PLAN.md section 2).
const PPU_RANGE: (u32, u32) = (0x000, 0x05F);
const APU_RANGE: (u32, u32) = (0x060, 0x0AF);
const DMA_RANGE: (u32, u32) = (0x0B0, 0x0DF);
const TIMER_RANGE: (u32, u32) = (0x100, 0x10F);
const SERIAL_LOW_RANGE: (u32, u32) = (0x120, 0x12F);
const KEYPAD_RANGE: (u32, u32) = (0x130, 0x133);
const SERIAL_HIGH_RANGE: (u32, u32) = (0x134, 0x15F);
const SYSTEM_RANGE: (u32, u32) = (crate::irq::IO_START, crate::irq::IO_END);

/// Replace only the masked bits of `old` with those of `new`. Shared helper for every owner.
pub fn merge(old: u16, new: u16, mask: u16) -> u16 {
    (old & !mask) | (new & mask)
}

fn in_range(offset: u32, range: (u32, u32)) -> bool {
    offset >= range.0 && offset <= range.1
}

/// Plain halfword storage for a block of IO registers starting at `base`. Stubs and simple
/// owners use it so a write reads back unchanged.
pub struct PlainRegisters<const HALFWORDS: usize> {
    base: u32,
    values: [u16; HALFWORDS],
}

impl<const HALFWORDS: usize> PlainRegisters<HALFWORDS> {
    pub fn new(base: u32) -> Self {
        PlainRegisters { base, values: [0; HALFWORDS] }
    }

    pub fn read(&self, offset: u32) -> u16 {
        self.values[((offset - self.base) / 2) as usize]
    }

    pub fn write(&mut self, offset: u32, value: u16, mask: u16) {
        let slot = &mut self.values[((offset - self.base) / 2) as usize];
        *slot = merge(*slot, value, mask);
    }
}

/// Serial port registers 0x120-0x15F (SIOCNT, SIODATA, RCNT, JOY_*): plain storage, never connected.
pub struct SerialStub {
    registers: PlainRegisters<0x20>,
}

impl SerialStub {
    pub fn new() -> Self {
        SerialStub { registers: PlainRegisters::new(SERIAL_LOW_RANGE.0) }
    }

    pub fn io_read16(&mut self, offset: u32) -> u16 {
        self.registers.read(offset)
    }

    pub fn io_write16(&mut self, offset: u32, value: u16, mask: u16) {
        self.registers.write(offset, value, mask)
    }
}

/// Bits that read back from the halfword at `offset`; 0 means the halfword is write-only or
/// unused (gbatek: GBA I/O Map, plus each register's bit table: unused and write-only bits
/// read as 0).
fn readable_bits(offset: u32) -> u16 {
    match offset {
        0x000 => 0xFFFF,          // DISPCNT
        0x002 => 0x0001,          // green swap
        0x004 => 0xFF3F,          // DISPSTAT (bits 6-7 unused)
        0x006 => 0x00FF,          // VCOUNT
        0x008 | 0x00A => 0xDFFF,  // BG0CNT, BG1CNT (bit 13 only exists on BG2/BG3)
        0x00C | 0x00E => 0xFFFF,  // BG2CNT, BG3CNT
        0x048 | 0x04A => 0x3F3F,  // WININ, WINOUT
        0x050 => 0x3FFF,          // BLDCNT
        0x052 => 0x1F1F,          // BLDALPHA
        0x060 => 0x007F,          // SOUND1CNT_L sweep
        0x062 | 0x068 => 0xFFC0,  // SOUND1CNT_H, SOUND2CNT_L: length is write-only
        0x064 | 0x06C | 0x074 => 0x4000, // SOUNDxCNT_X/H: only the length flag reads back
        0x070 => 0x00E0,          // SOUND3CNT_L
        0x072 => 0xE000,          // SOUND3CNT_H: volume and force-75% bits; length is write-only
        0x078 => 0xFF00,          // SOUND4CNT_L: envelope only
        0x07C => 0x40FF,          // SOUND4CNT_H
        0x080 => 0xFF77,          // SOUNDCNT_L
        0x082 => 0x770F,          // SOUNDCNT_H (FIFO reset bits are write-only)
        0x084 => 0x008F,          // SOUNDCNT_X
        0x088 => 0xC3FE,          // SOUNDBIAS
        0x090..=0x09E => 0xFFFF,  // WAVE_RAM
        0x0BA | 0x0C6 | 0x0D2 => 0xF7E0, // DMA0CNT_H-DMA2CNT_H
        0x0DE => 0xFFE0,          // DMA3CNT_H (bit 11 Game Pak DRQ)
        0x100 | 0x104 | 0x108 | 0x10C => 0xFFFF, // TMxCNT_L
        0x102 | 0x106 | 0x10A | 0x10E => 0x00C7, // TMxCNT_H
        0x120..=0x12A => 0xFFFF,  // SIODATA / SIOMULTI0-3, SIOCNT, SIODATA8
        0x130 => 0x03FF,          // KEYINPUT
        0x132 => 0xC3FF,          // KEYCNT
        0x134 => 0xFFFF,          // RCNT
        0x140 => 0xFFFF,          // JOYCNT
        0x150..=0x158 => 0xFFFF,  // JOY_RECV, JOY_TRANS, JOYSTAT
        0x200 | 0x202 => 0x3FFF,  // IE, IF
        0x204 => 0x5FFF,          // WAITCNT (bit 13 unused; bit 15 reads 0: GBA cartridge)
        0x208 => 0x0001,          // IME
        0x300 => 0x0001,          // POSTFLG bit 0; HALTCNT (high byte) is write-only
        0x800 | 0x802 => 0xFFFF,  // internal memory control
        _ => 0,
    }
}

impl Bus {
    /// Halfword read with gbatek's rules for unreadable ports ("Reading from Unused or
    /// Write-Only I/O Ports"): if the other half of the 32-bit fragment is readable the result is
    /// 0, otherwise the open-bus value. MOSAIC (0x04C) and 0x04E therefore read as open bus,
    /// contrary to gbatek's example, because the mgba-io-read reference expects it.
    fn io_dispatch_read16(&mut self, offset: u32) -> u16 {
        let bits = readable_bits(offset);
        if bits != 0 {
            self.io_owner_read16(offset) & bits
        } else if readable_bits(offset ^ 2) != 0 {
            0
        } else {
            (self.open_bus_value() >> ((offset & 2) * 8)) as u16
        }
    }

    /// Route a halfword read to its owner.
    fn io_owner_read16(&mut self, offset: u32) -> u16 {
        if in_range(offset, PPU_RANGE) {
            self.ppu.io_read16(offset)
        } else if in_range(offset, APU_RANGE) {
            self.apu.io_read16(offset)
        } else if in_range(offset, DMA_RANGE) {
            self.dma.io_read16(offset)
        } else if in_range(offset, TIMER_RANGE) {
            self.timers.catch_up(self.access_cycles, &mut self.irq);
            self.timers.io_read16(offset)
        } else if in_range(offset, KEYPAD_RANGE) {
            self.keypad.io_read16(offset)
        } else if in_range(offset, SERIAL_LOW_RANGE) || in_range(offset, SERIAL_HIGH_RANGE) {
            self.serial.io_read16(offset)
        } else if in_range(offset, SYSTEM_RANGE) {
            self.irq.io_read16(offset)
        } else {
            0
        }
    }

    /// Route a halfword write (with byte mask) to its owner. Unmapped offsets are ignored.
    fn io_dispatch_write16(&mut self, offset: u32, value: u16, mask: u16) {
        if in_range(offset, PPU_RANGE) {
            self.ppu.io_write16(offset, value, mask)
        } else if in_range(offset, APU_RANGE) {
            self.apu.io_write16(offset, value, mask)
        } else if in_range(offset, DMA_RANGE) {
            self.dma.io_write16(offset, value, mask)
        } else if in_range(offset, TIMER_RANGE) {
            self.timers.catch_up(self.access_cycles, &mut self.irq);
            self.timers.io_write16(offset, value, mask)
        } else if in_range(offset, KEYPAD_RANGE) {
            self.keypad.io_write16(offset, value, mask)
        } else if in_range(offset, SERIAL_LOW_RANGE) || in_range(offset, SERIAL_HIGH_RANGE) {
            self.serial.io_write16(offset, value, mask)
        } else if in_range(offset, SYSTEM_RANGE) {
            self.irq.io_write16(offset, value, mask);
            self.note_wait_control_write(offset, value, mask);
        }
    }

    pub fn io_read8(&mut self, offset: u32) -> u8 {
        let halfword = self.io_dispatch_read16(offset & !1);
        (halfword >> ((offset & 1) * 8)) as u8
    }

    pub fn io_read16(&mut self, offset: u32) -> u16 {
        self.io_dispatch_read16(offset & !1)
    }

    pub fn io_read32(&mut self, offset: u32) -> u32 {
        let aligned = offset & !3;
        let low = self.io_dispatch_read16(aligned) as u32;
        let high = self.io_dispatch_read16(aligned + 2) as u32;
        low | (high << 16)
    }

    pub fn io_write8(&mut self, offset: u32, value: u8) {
        let shift = (offset & 1) * 8;
        let halfword = (value as u16) << shift;
        let mask = 0xFFu16 << shift;
        self.io_dispatch_write16(offset & !1, halfword, mask);
    }

    pub fn io_write16(&mut self, offset: u32, value: u16) {
        self.io_dispatch_write16(offset & !1, value, 0xFFFF);
    }

    pub fn io_write32(&mut self, offset: u32, value: u32) {
        let aligned = offset & !3;
        self.io_dispatch_write16(aligned, value as u16, 0xFFFF);
        self.io_dispatch_write16(aligned + 2, (value >> 16) as u16, 0xFFFF);
    }
}
