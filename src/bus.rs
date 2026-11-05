//! System bus: memory map, region dispatch and access-cycle costs (gbatek: GBA Memory Map).
//!
//! The CPU only ever talks to the bus. Every access adds its cycle cost to `access_cycles`,
//! which the CPU drains with `take_access_cycles()` to report how long a step took.

use crate::apu::Apu;
use crate::cart::Cart;
use crate::dma::Dma;
use crate::io::SerialStub;
use crate::irq::Irq;
use crate::keypad::Keypad;
use crate::ppu::Ppu;
use crate::timer::Timers;

/// The 16 KiB BIOS stub supplied by the benchmark, mapped at 0x00000000.
static BIOS: &[u8; BIOS_SIZE] = include_bytes!("../assets/gba_bios_stub.bin");

pub const BIOS_SIZE: usize = 0x4000;
pub const EWRAM_SIZE: usize = 0x40000; // on-board work RAM, 256 KiB
pub const IWRAM_SIZE: usize = 0x8000; // on-chip work RAM, 32 KiB

/// Access width, used to pick the cycle cost.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Width {
    Byte,
    Halfword,
    Word,
}

/// Memory regions, selected by address bits 24-27.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Region {
    Bios,
    Ewram,
    Iwram,
    Io,
    Palette,
    Vram,
    Oam,
    Rom,
    Sram,
    Unmapped,
}

fn region_of(address: u32) -> Region {
    match address >> 24 {
        0x00 => Region::Bios,
        0x02 => Region::Ewram,
        0x03 => Region::Iwram,
        0x04 => Region::Io,
        0x05 => Region::Palette,
        0x06 => Region::Vram,
        0x07 => Region::Oam,
        0x08..=0x0D => Region::Rom,
        0x0E | 0x0F => Region::Sram,
        _ => Region::Unmapped,
    }
}

/// Cycle cost with default wait states (gbatek: GBA Memory Map, "Address Bus Width and CPU
/// Read/Write Access Widths").
// ponytail: fixed default wait states, no WAITCNT, no sequential/non-sequential split, no
// video-memory contention; Phase 2/5 wire WAITCNT and prefetch in here.
fn access_cost(region: Region, width: Width) -> u32 {
    let is_word = width == Width::Word;
    match region {
        Region::Bios | Region::Iwram | Region::Io | Region::Oam | Region::Unmapped => 1,
        Region::Ewram => if is_word { 6 } else { 3 },
        Region::Palette | Region::Vram => if is_word { 2 } else { 1 },
        Region::Rom => if is_word { 8 } else { 5 },
        Region::Sram => 5,
    }
}

/// Offset into VRAM: 96 KiB mirrored in 128 KiB steps, the upper 32 KiB mirroring 0x10000-0x17FFF.
fn vram_offset(address: u32) -> usize {
    let offset = address & 0x1FFFF;
    (if offset >= 0x18000 { offset - 0x8000 } else { offset }) as usize
}

fn read_le16(memory: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([memory[offset], memory[offset + 1]])
}

fn read_le32(memory: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([memory[offset], memory[offset + 1], memory[offset + 2], memory[offset + 3]])
}

fn write_le16(memory: &mut [u8], offset: usize, value: u16) {
    memory[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_le32(memory: &mut [u8], offset: usize, value: u32) {
    memory[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

pub struct Bus {
    pub ewram: Vec<u8>,
    pub iwram: Vec<u8>,
    pub ppu: Ppu, // also owns palette RAM, VRAM and OAM
    pub apu: Apu,
    pub dma: Dma,
    pub timers: Timers,
    pub keypad: Keypad,
    pub serial: SerialStub,
    pub irq: Irq,
    pub cart: Cart,
    /// Cycles spent on accesses since the last `take_access_cycles()`.
    pub access_cycles: u32,
}

impl Bus {
    pub fn new(rom: &[u8]) -> Self {
        Bus {
            ewram: vec![0; EWRAM_SIZE],
            iwram: vec![0; IWRAM_SIZE],
            ppu: Ppu::new(),
            apu: Apu::new(),
            dma: Dma::new(),
            timers: Timers::new(),
            keypad: Keypad::new(),
            serial: SerialStub::new(),
            irq: Irq::new(),
            cart: Cart::new(rom),
            access_cycles: 0,
        }
    }

    /// Return and reset the cycles spent on bus accesses.
    pub fn take_access_cycles(&mut self) -> u32 {
        std::mem::take(&mut self.access_cycles)
    }

    fn charge(&mut self, address: u32, width: Width) -> Region {
        let region = region_of(address);
        self.access_cycles += access_cost(region, width);
        region
    }

    pub fn read8(&mut self, address: u32) -> u8 {
        match self.charge(address, Width::Byte) {
            Region::Bios => bios_read(address, 1) as u8,
            Region::Ewram => self.ewram[(address as usize) & (EWRAM_SIZE - 1)],
            Region::Iwram => self.iwram[(address as usize) & (IWRAM_SIZE - 1)],
            Region::Io => self.io_read8(io_offset(address)),
            Region::Palette => self.ppu.palette[(address & 0x3FF) as usize],
            Region::Vram => self.ppu.vram[vram_offset(address)],
            Region::Oam => self.ppu.oam[(address & 0x3FF) as usize],
            Region::Rom => self.cart.rom_read8(address),
            Region::Sram => self.cart.sram_read8(address),
            Region::Unmapped => 0,
        }
    }

    pub fn read16(&mut self, address: u32) -> u16 {
        // ponytail: misaligned addresses are force-aligned; rotation of misaligned reads belongs to the CPU.
        let address = address & !1;
        match self.charge(address, Width::Halfword) {
            Region::Bios => bios_read(address, 2) as u16,
            Region::Ewram => read_le16(&self.ewram, (address as usize) & (EWRAM_SIZE - 1)),
            Region::Iwram => read_le16(&self.iwram, (address as usize) & (IWRAM_SIZE - 1)),
            Region::Io => self.io_read16(io_offset(address)),
            Region::Palette => read_le16(&self.ppu.palette, (address & 0x3FF) as usize),
            Region::Vram => read_le16(&self.ppu.vram, vram_offset(address)),
            Region::Oam => read_le16(&self.ppu.oam, (address & 0x3FF) as usize),
            Region::Rom => self.cart.rom_read16(address),
            Region::Sram => self.cart.sram_read8(address) as u16 * 0x0101,
            Region::Unmapped => 0,
        }
    }

    pub fn read32(&mut self, address: u32) -> u32 {
        let address = address & !3;
        match self.charge(address, Width::Word) {
            Region::Bios => bios_read(address, 4),
            Region::Ewram => read_le32(&self.ewram, (address as usize) & (EWRAM_SIZE - 1)),
            Region::Iwram => read_le32(&self.iwram, (address as usize) & (IWRAM_SIZE - 1)),
            Region::Io => self.io_read32(io_offset(address)),
            Region::Palette => read_le32(&self.ppu.palette, (address & 0x3FF) as usize),
            Region::Vram => read_le32(&self.ppu.vram, vram_offset(address)),
            Region::Oam => read_le32(&self.ppu.oam, (address & 0x3FF) as usize),
            Region::Rom => {
                let low = self.cart.rom_read16(address) as u32;
                let high = self.cart.rom_read16(address + 2) as u32;
                low | (high << 16)
            }
            Region::Sram => self.cart.sram_read8(address) as u32 * 0x0101_0101,
            Region::Unmapped => 0,
        }
    }

    pub fn write8(&mut self, address: u32, value: u8) {
        match self.charge(address, Width::Byte) {
            Region::Ewram => self.ewram[(address as usize) & (EWRAM_SIZE - 1)] = value,
            Region::Iwram => self.iwram[(address as usize) & (IWRAM_SIZE - 1)] = value,
            Region::Io => self.io_write8(io_offset(address), value),
            // Palette and VRAM have a 16-bit data bus: a byte write lands in both halves.
            // ponytail: VRAM byte writes into the OBJ area should be ignored; mode-dependent, left to the PPU agent.
            Region::Palette => {
                write_le16(&mut self.ppu.palette, (address & 0x3FE) as usize, value as u16 * 0x0101)
            }
            Region::Vram => {
                write_le16(&mut self.ppu.vram, vram_offset(address & !1), value as u16 * 0x0101)
            }
            Region::Oam => {} // OAM ignores 8-bit writes
            Region::Sram => self.cart.sram_write8(address, value),
            Region::Bios | Region::Rom | Region::Unmapped => {}
        }
    }

    pub fn write16(&mut self, address: u32, value: u16) {
        let address = address & !1;
        match self.charge(address, Width::Halfword) {
            Region::Ewram => write_le16(&mut self.ewram, (address as usize) & (EWRAM_SIZE - 1), value),
            Region::Iwram => write_le16(&mut self.iwram, (address as usize) & (IWRAM_SIZE - 1), value),
            Region::Io => self.io_write16(io_offset(address), value),
            Region::Palette => write_le16(&mut self.ppu.palette, (address & 0x3FF) as usize, value),
            Region::Vram => write_le16(&mut self.ppu.vram, vram_offset(address), value),
            Region::Oam => write_le16(&mut self.ppu.oam, (address & 0x3FF) as usize, value),
            Region::Sram => self.cart.sram_write8(address, value as u8),
            Region::Rom => self.cart.rom_write16(address, value),
            Region::Bios | Region::Unmapped => {}
        }
    }

    pub fn write32(&mut self, address: u32, value: u32) {
        let address = address & !3;
        match self.charge(address, Width::Word) {
            Region::Ewram => write_le32(&mut self.ewram, (address as usize) & (EWRAM_SIZE - 1), value),
            Region::Iwram => write_le32(&mut self.iwram, (address as usize) & (IWRAM_SIZE - 1), value),
            Region::Io => self.io_write32(io_offset(address), value),
            Region::Palette => write_le32(&mut self.ppu.palette, (address & 0x3FF) as usize, value),
            Region::Vram => write_le32(&mut self.ppu.vram, vram_offset(address), value),
            Region::Oam => write_le32(&mut self.ppu.oam, (address & 0x3FF) as usize, value),
            Region::Sram => self.cart.sram_write8(address, value as u8),
            Region::Rom => {
                self.cart.rom_write16(address, value as u16);
                self.cart.rom_write16(address + 2, (value >> 16) as u16);
            }
            Region::Bios | Region::Unmapped => {}
        }
    }
}

/// IO offset from 0x04000000. Only 0x000-0x3FF and 0x800-0x803 are registers; the rest reads 0.
fn io_offset(address: u32) -> u32 {
    let offset = address & 0x00FF_FFFF;
    if offset < 0x400 || (0x800..0x804).contains(&offset) {
        offset
    } else {
        0xFFFF_FFFF // no owner: io.rs treats it as unmapped
    }
}

/// Little-endian read of `size` bytes from the BIOS. Outside 0x0000-0x3FFF reads 0.
// ponytail: no BIOS read protection (last-fetched-opcode value); add when a test reads BIOS from ROM code.
fn bios_read(address: u32, size: usize) -> u32 {
    let offset = address as usize;
    if offset + size > BIOS_SIZE {
        return 0;
    }
    (0..size).fold(0, |value, i| value | (BIOS[offset + i] as u32) << (8 * i))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_every_width_in_ram_regions() {
        let mut bus = Bus::new(&[]);
        for base in [0x0200_0000u32, 0x0300_0000, 0x0500_0000, 0x0600_0000, 0x0700_0000] {
            bus.write32(base + 0x10, 0xDEAD_BEEF);
            assert_eq!(bus.read32(base + 0x10), 0xDEAD_BEEF, "word at {base:#x}");
            assert_eq!(bus.read16(base + 0x12), 0xDEAD, "high halfword at {base:#x}");
            assert_eq!(bus.read8(base + 0x10), 0xEF, "low byte at {base:#x}");

            bus.write16(base + 0x20, 0x1234);
            assert_eq!(bus.read16(base + 0x20), 0x1234, "halfword at {base:#x}");
        }
        for base in [0x0200_0000u32, 0x0300_0000] {
            bus.write8(base + 0x31, 0xAB);
            assert_eq!(bus.read8(base + 0x31), 0xAB, "byte at {base:#x}");
        }
        // Mirrors: EWRAM every 256 KiB, IWRAM every 32 KiB.
        assert_eq!(bus.read32(0x0204_0010), 0xDEAD_BEEF);
        assert_eq!(bus.read32(0x0300_8010), 0xDEAD_BEEF);
    }

    #[test]
    fn bios_is_mapped_and_read_only() {
        let mut bus = Bus::new(&[]);
        let reset_vector = bus.read32(0);
        assert_eq!(reset_vector, u32::from_le_bytes([BIOS[0], BIOS[1], BIOS[2], BIOS[3]]));
        bus.write32(0, 0);
        assert_eq!(bus.read32(0), reset_vector);
    }

    #[test]
    fn io_byte_write_acknowledges_only_its_own_if_bits() {
        let mut bus = Bus::new(&[]);
        bus.irq.interrupt_flags = 0x0101;
        bus.write8(0x0400_0202, 0x01); // acknowledge bit 0 only
        assert_eq!(bus.read16(0x0400_0202), 0x0100);
        bus.write32(0x0400_0200, 0x0000_3FFF); // IE = all, IF write of 0 clears nothing
        assert_eq!(bus.read32(0x0400_0200), 0x0100_3FFF);
    }

    #[test]
    fn access_cycles_follow_default_wait_states() {
        let mut bus = Bus::new(&[]);
        bus.read32(0x0200_0000);
        assert_eq!(bus.take_access_cycles(), 6);
        bus.read16(0x0800_0000);
        assert_eq!(bus.take_access_cycles(), 5);
    }
}
