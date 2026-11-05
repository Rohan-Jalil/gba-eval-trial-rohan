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

/// Wait-state settings derived from WAITCNT (0x4000204) and the internal memory control register
/// (0x4000800) (gbatek: GBA System Control). All values are total cycles for one 16-bit access.
#[derive(Clone, Copy, Debug)]
pub struct WaitStates {
    /// Game Pak ROM wait states 0-2: first (non-sequential) access.
    rom_nonsequential: [u32; 3],
    /// Game Pak ROM wait states 0-2: second (sequential) access.
    rom_sequential: [u32; 3],
    /// Game Pak SRAM, any width.
    sram: u32,
    /// On-board 256K work RAM, 8/16-bit (a 32-bit access costs twice this).
    ewram: u32,
}

// WAITCNT fields (gbatek: 4000204h WAITCNT).
const WAITCNT_SRAM_SHIFT: u32 = 0; // bits 0-1, SRAM wait control
const WAITCNT_WS0_FIRST_SHIFT: u32 = 2; // bits 2-3, wait state 0 first access
const WAITCNT_WS0_SECOND_BIT: u32 = 4; // wait state 0 second access
const WAITCNT_WS1_FIRST_SHIFT: u32 = 5; // bits 5-6, wait state 1 first access
const WAITCNT_WS1_SECOND_BIT: u32 = 7; // wait state 1 second access
const WAITCNT_WS2_FIRST_SHIFT: u32 = 8; // bits 8-9, wait state 2 first access
const WAITCNT_WS2_SECOND_BIT: u32 = 10; // wait state 2 second access

/// WAITCNT first-access wait counts for field values 0..3 (gbatek: WAITCNT bits 0-1, 2-3, 5-6, 8-9).
const FIRST_ACCESS_WAITS: [u32; 4] = [4, 3, 2, 8];
/// WAITCNT second-access wait counts for field value 0 per wait state (1 when the bit is set).
const SECOND_ACCESS_WAITS: [u32; 3] = [2, 4, 8];

impl WaitStates {
    /// Decode WAITCNT and the internal memory control word (gbatek: 4000204h WAITCNT,
    /// 4000800h Internal Memory Control). Each access takes 1 cycle plus its wait states.
    pub fn from_registers(waitcnt: u16, memory_control: u32) -> Self {
        // Total cycles for a first access whose 2-bit WAITCNT field starts at `shift`.
        let first_access = |shift: u32| 1 + FIRST_ACCESS_WAITS[((waitcnt >> shift) & 3) as usize];
        // Total cycles for a second access of wait state `state`, selected by WAITCNT bit `bit`.
        let second_access = |state: usize, bit: u32| {
            if waitcnt & (1 << bit) != 0 { 2 } else { 1 + SECOND_ACCESS_WAITS[state] }
        };
        // ponytail: only the 256K WRAM wait field of 0x4000800 is decoded; bit 0 (WRAM disable) and
        // bit 5 (256K WRAM enable) are ignored, add them if a ROM toggles them.
        let ewram_wait_field = (memory_control >> 24) & 0xF;
        WaitStates {
            rom_nonsequential: [
                first_access(WAITCNT_WS0_FIRST_SHIFT),
                first_access(WAITCNT_WS1_FIRST_SHIFT),
                first_access(WAITCNT_WS2_FIRST_SHIFT),
            ],
            rom_sequential: [
                second_access(0, WAITCNT_WS0_SECOND_BIT),
                second_access(1, WAITCNT_WS1_SECOND_BIT),
                second_access(2, WAITCNT_WS2_SECOND_BIT),
            ],
            sram: first_access(WAITCNT_SRAM_SHIFT),
            // 0-14 = 15..1 wait states; 15 locks up on hardware, treated as 1 wait state here.
            ewram: 1 + 15u32.saturating_sub(ewram_wait_field).max(1),
        }
    }

    /// Cycle cost of one access (gbatek: GBA Memory Map, "Address Bus Width and CPU Read/Write
    /// Access Widths"; GBA System Control, WAITCNT). `sequential` only matters for Game Pak ROM.
    // ponytail: no Game Pak prefetch buffer and no video-memory contention; Phase 5 adds them.
    fn access_cost(&self, region: Region, address: u32, width: Width, sequential: bool) -> u32 {
        let is_word = width == Width::Word;
        match region {
            Region::Bios | Region::Iwram | Region::Io | Region::Oam | Region::Unmapped => 1,
            Region::Ewram => if is_word { 2 * self.ewram } else { self.ewram },
            Region::Palette | Region::Vram => if is_word { 2 } else { 1 },
            Region::Rom => {
                let wait_state = (((address >> 24) - 0x08) / 2) as usize;
                let first = if sequential {
                    self.rom_sequential[wait_state]
                } else {
                    self.rom_nonsequential[wait_state]
                };
                // The 16-bit Game Pak bus splits a word into two halves; the second is sequential.
                if is_word { first + self.rom_sequential[wait_state] } else { first }
            }
            Region::Sram => self.sram,
        }
    }
}

/// Offset returned for IO addresses with no register; io.rs reads them as open bus.
pub const UNMAPPED_IO_OFFSET: u32 = 0xFFFF_FFFC;

pub const WAITCNT_OFFSET: u32 = 0x204; // WAITCNT, Game Pak wait-state control
pub const MEMORY_CONTROL_OFFSET: u32 = 0x800; // undocumented internal memory control, low half
pub const MEMORY_CONTROL_HIGH_OFFSET: u32 = 0x802; // internal memory control, high half

/// BIOS address whose opcode BIOS read protection returns before any other BIOS code ran.
const BIOS_STARTUP_PREFETCH_ADDRESS: u32 = 0xDC + 8;

/// Power-on value of the internal memory control register at 0x4000800 (gbatek).
pub const MEMORY_CONTROL_RESET: u32 = 0x0D00_0020;

/// Game Pak accesses are forced non-sequential at the start of every 128K block (gbatek: WAITCNT notes).
const ROM_SEQUENTIAL_BLOCK_MASK: u32 = 0x1FFFF;

fn width_in_bytes(width: Width) -> u32 {
    match width {
        Width::Byte => 1,
        Width::Halfword => 2,
        Width::Word => 4,
    }
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
    /// Current wait-state table, refreshed when WAITCNT or 0x4000800 is written.
    pub wait_states: WaitStates,
    /// Shadow copies of WAITCNT and the internal memory control register, kept here because the
    /// wait-state table is derived from them. The registers themselves are stored by irq.rs.
    waitcnt: u16,
    memory_control: u32,
    /// Address that would continue the previous access sequentially (for S vs N cycles).
    next_sequential_address: u32,
    /// One-shot override for the next access's sequential/non-sequential timing, for callers
    /// that know better than address tracking (DMA alternates reads and writes).
    forced_sequential: Option<bool>,
    /// Address of the instruction the CPU is executing and its state, set by the CPU before each
    /// fetch. Open-bus reads and BIOS read protection are derived from it.
    executing_address: u32,
    executing_thumb: bool,
    /// The last opcode word prefetched while executing inside the BIOS (BIOS read protection).
    last_bios_prefetch: u32,
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
            wait_states: WaitStates::from_registers(0, MEMORY_CONTROL_RESET),
            waitcnt: 0,
            memory_control: MEMORY_CONTROL_RESET,
            next_sequential_address: u32::MAX,
            forced_sequential: None,
            executing_address: 0,
            executing_thumb: false,
            // gbatek: after startup the protected value is the opcode at [00DCh+8].
            last_bios_prefetch: bios_read(BIOS_STARTUP_PREFETCH_ADDRESS, 4),
        }
    }

    /// Return and reset the cycles spent on bus accesses.
    pub fn take_access_cycles(&mut self) -> u32 {
        std::mem::take(&mut self.access_cycles)
    }

    /// Add the cost of one access to `access_cycles`. An access is sequential when it continues
    /// directly after the previous one, except at a Game Pak 128K block boundary.
    fn charge(&mut self, address: u32, width: Width) -> Region {
        let region = region_of(address);
        let continues_previous = match self.forced_sequential.take() {
            Some(sequential) => sequential,
            None => address == self.next_sequential_address,
        };
        let sequential = continues_previous && (address & ROM_SEQUENTIAL_BLOCK_MASK) != 0;
        self.access_cycles += self.wait_states.access_cost(region, address, width, sequential);
        self.next_sequential_address = address.wrapping_add(width_in_bytes(width));
        region
    }

    /// Make the next access sequential (S) or non-sequential (N) regardless of its address.
    /// DMA uses this: its reads and writes alternate, but each side is sequential after the
    /// first unit (gbatek: DMA Transfer Time, 2N+2(n-1)S+xI).
    pub fn force_next_access_sequential(&mut self, sequential: bool) {
        self.forced_sequential = Some(sequential);
    }

    /// Track writes to WAITCNT (0x204) and the internal memory control register (0x800/0x802)
    /// and recompute the wait-state table (gbatek: GBA System Control).
    pub fn note_wait_control_write(&mut self, offset: u32, value: u16, mask: u16) {
        match offset {
            WAITCNT_OFFSET => self.waitcnt = crate::io::merge(self.waitcnt, value, mask),
            MEMORY_CONTROL_OFFSET | MEMORY_CONTROL_HIGH_OFFSET => {
                let shift = (offset - MEMORY_CONTROL_OFFSET) * 8;
                let old_half = (self.memory_control >> shift) as u16;
                let new_half = crate::io::merge(old_half, value, mask) as u32;
                self.memory_control = (self.memory_control & !(0xFFFF << shift)) | (new_half << shift);
            }
            _ => return,
        }
        self.wait_states = WaitStates::from_registers(self.waitcnt, self.memory_control);
    }

    /// SRAM read at any width: the byte at the exact address, repeated across the result
    /// (gbatek: Accessing SRAM Area by 16bit/32bit).
    fn sram_read_replicated(&mut self, address: u32, width: Width) -> u32 {
        self.charge(address, width);
        self.cart.sram_read8(address) as u32 * 0x0101_0101
    }

    /// SRAM write at 16/32-bit width: only the byte at the exact address changes, set to the
    /// low byte of `value ROR (address * 8)` (gbatek: Accessing SRAM Area by 16bit/32bit).
    fn sram_write_rotated(&mut self, address: u32, value: u32, width: Width) {
        self.charge(address, width);
        let rotated = value.rotate_right((address & 3) * 8);
        self.cart.sram_write8(address, rotated as u8);
    }

    /// Offset into VRAM: 96K repeated in 128K steps, 0x18000-0x1FFFF mirroring 0x10000-0x17FFF
    /// (gbatek: Memory Mirrors). In bitmap modes 3-5, 0x18000-0x1BFFF is not mapped: reads give 0
    /// and writes are dropped (measured: nba-hw ppu-vram-mirror reference; gbatek is silent).
    fn vram_offset(&self, address: u32) -> Option<usize> {
        let offset = (address & 0x1FFFF) as usize;
        if offset < 0x18000 {
            Some(offset)
        } else if offset < 0x1C000 && self.vram_bg_area_size() > 0x10000 {
            None
        } else {
            Some(offset - 0x8000)
        }
    }

    /// Size of the BG part of VRAM, where byte writes are allowed: 64K in tiled modes, 80K in
    /// bitmap modes 3-5 (gbatek: Writing 8bit Data to Video Memory).
    fn vram_bg_area_size(&self) -> usize {
        let bg_mode = self.ppu.dispcnt() & 7; // DISPCNT bits 0-2
        if bg_mode >= 3 { 0x14000 } else { 0x10000 }
    }

    /// Called by the CPU before it fetches the instruction at `address`. Keeps the state needed
    /// for open-bus values and BIOS read protection (gbatek: Reading from Unused Memory).
    pub fn note_instruction_fetch(&mut self, address: u32, thumb: bool) {
        self.executing_address = address;
        self.executing_thumb = thumb;
        if (address as usize) < BIOS_SIZE {
            let prefetch = address.wrapping_add(if thumb { 4 } else { 8 });
            self.last_bios_prefetch = bios_read(prefetch & !3, 4);
        }
    }

    /// Value seen when reading unused memory: the opcode the CPU most recently prefetched
    /// (gbatek: Reading from Unused Memory). ARM: [$+8]. THUMB depends on the code's region.
    pub fn open_bus_value(&self) -> u32 {
        let address = self.executing_address;
        if !self.executing_thumb {
            return self.peek16(address.wrapping_add(8)) as u32
                | (self.peek16(address.wrapping_add(10)) as u32) << 16;
        }
        let halfword_after = |delta: u32| self.peek16(address.wrapping_add(delta)) as u32;
        let word_aligned = address & 2 == 0;
        match region_of(address) {
            Region::Bios | Region::Oam => {
                if word_aligned {
                    halfword_after(4) | halfword_after(6) << 16
                } else {
                    halfword_after(2) | halfword_after(4) << 16
                }
            }
            // OldLO/OldHI are usually [$+2] (gbatek); the LDR-overwrite case is not modelled.
            Region::Iwram => {
                if word_aligned {
                    halfword_after(4) | halfword_after(2) << 16
                } else {
                    halfword_after(2) | halfword_after(4) << 16
                }
            }
            _ => halfword_after(4) * 0x0001_0001,
        }
    }

    /// Read a halfword without charging cycles or touching IO, for open-bus values.
    fn peek16(&self, address: u32) -> u16 {
        let address = address & !1;
        match region_of(address) {
            Region::Bios => bios_read(address, 2) as u16,
            Region::Ewram => read_le16(&self.ewram, (address as usize) & (EWRAM_SIZE - 1)),
            Region::Iwram => read_le16(&self.iwram, (address as usize) & (IWRAM_SIZE - 1)),
            Region::Palette => read_le16(&self.ppu.palette, (address & 0x3FF) as usize),
            Region::Vram => self
                .vram_offset(address)
                .map_or(0, |offset| read_le16(&self.ppu.vram, offset)),
            Region::Oam => read_le16(&self.ppu.oam, (address & 0x3FF) as usize),
            Region::Rom => self.cart.rom_read16(address),
            Region::Io | Region::Sram | Region::Unmapped => 0,
        }
    }

    /// Read of the BIOS region (0x00000000-0x01FFFFFF), returned as the aligned word containing
    /// `address`. Past 16K is unused memory. While the CPU executes outside the BIOS, reads return
    /// the last opcode prefetched inside it (gbatek: BIOS ROM read protection).
    fn bios_region_word(&self, address: u32) -> u32 {
        if (address as usize) >= BIOS_SIZE {
            self.open_bus_value()
        } else if (self.executing_address as usize) >= BIOS_SIZE {
            self.last_bios_prefetch
        } else {
            bios_read(address & !3, 4)
        }
    }

    pub fn read8(&mut self, address: u32) -> u8 {
        match self.charge(address, Width::Byte) {
            Region::Bios => byte_of_word(self.bios_region_word(address), address),
            Region::Ewram => self.ewram[(address as usize) & (EWRAM_SIZE - 1)],
            Region::Iwram => self.iwram[(address as usize) & (IWRAM_SIZE - 1)],
            Region::Io => self.io_read8(io_offset(address)),
            Region::Palette => self.ppu.palette[(address & 0x3FF) as usize],
            Region::Vram => self.vram_offset(address).map_or(0, |offset| self.ppu.vram[offset]),
            Region::Oam => self.ppu.oam[(address & 0x3FF) as usize],
            Region::Rom => self.cart.rom_read8(address),
            Region::Sram => self.cart.sram_read8(address),
            Region::Unmapped => byte_of_word(self.open_bus_value(), address),
        }
    }

    pub fn read16(&mut self, address: u32) -> u16 {
        if region_of(address) == Region::Sram {
            return self.sram_read_replicated(address, Width::Halfword) as u16;
        }
        // Misaligned addresses are force-aligned; rotating misaligned reads belongs to the CPU.
        let address = address & !1;
        match self.charge(address, Width::Halfword) {
            Region::Bios => halfword_of_word(self.bios_region_word(address), address),
            Region::Ewram => read_le16(&self.ewram, (address as usize) & (EWRAM_SIZE - 1)),
            Region::Iwram => read_le16(&self.iwram, (address as usize) & (IWRAM_SIZE - 1)),
            Region::Io => self.io_read16(io_offset(address)),
            Region::Palette => read_le16(&self.ppu.palette, (address & 0x3FF) as usize),
            Region::Vram => self
                .vram_offset(address)
                .map_or(0, |offset| read_le16(&self.ppu.vram, offset)),
            Region::Oam => read_le16(&self.ppu.oam, (address & 0x3FF) as usize),
            Region::Rom => self.cart.rom_read16(address),
            Region::Sram => unreachable!("SRAM handled before alignment"),
            Region::Unmapped => halfword_of_word(self.open_bus_value(), address),
        }
    }

    pub fn read32(&mut self, address: u32) -> u32 {
        if region_of(address) == Region::Sram {
            return self.sram_read_replicated(address, Width::Word);
        }
        let address = address & !3;
        match self.charge(address, Width::Word) {
            Region::Bios => self.bios_region_word(address),
            Region::Ewram => read_le32(&self.ewram, (address as usize) & (EWRAM_SIZE - 1)),
            Region::Iwram => read_le32(&self.iwram, (address as usize) & (IWRAM_SIZE - 1)),
            Region::Io => self.io_read32(io_offset(address)),
            Region::Palette => read_le32(&self.ppu.palette, (address & 0x3FF) as usize),
            Region::Vram => self
                .vram_offset(address)
                .map_or(0, |offset| read_le32(&self.ppu.vram, offset)),
            Region::Oam => read_le32(&self.ppu.oam, (address & 0x3FF) as usize),
            Region::Rom => {
                let low = self.cart.rom_read16(address) as u32;
                let high = self.cart.rom_read16(address + 2) as u32;
                low | (high << 16)
            }
            Region::Sram => unreachable!("SRAM handled before alignment"),
            Region::Unmapped => self.open_bus_value(),
        }
    }

    pub fn write8(&mut self, address: u32, value: u8) {
        match self.charge(address, Width::Byte) {
            Region::Ewram => self.ewram[(address as usize) & (EWRAM_SIZE - 1)] = value,
            Region::Iwram => self.iwram[(address as usize) & (IWRAM_SIZE - 1)] = value,
            Region::Io => self.io_write8(io_offset(address), value),
            // Palette and BG VRAM have a 16-bit data bus: a byte write lands in both halves.
            Region::Palette => {
                write_le16(&mut self.ppu.palette, (address & 0x3FE) as usize, value as u16 * 0x0101)
            }
            Region::Vram => {
                // Byte writes to OBJ VRAM are ignored.
                if let Some(offset) = self.vram_offset(address & !1) {
                    if offset < self.vram_bg_area_size() {
                        write_le16(&mut self.ppu.vram, offset, value as u16 * 0x0101);
                    }
                }
            }
            Region::Oam => {} // OAM ignores 8-bit writes
            Region::Sram => self.cart.sram_write8(address, value),
            Region::Bios | Region::Rom | Region::Unmapped => {}
        }
    }

    pub fn write16(&mut self, address: u32, value: u16) {
        if region_of(address) == Region::Sram {
            return self.sram_write_rotated(address, value as u32 * 0x0001_0001, Width::Halfword);
        }
        let address = address & !1;
        match self.charge(address, Width::Halfword) {
            Region::Ewram => write_le16(&mut self.ewram, (address as usize) & (EWRAM_SIZE - 1), value),
            Region::Iwram => write_le16(&mut self.iwram, (address as usize) & (IWRAM_SIZE - 1), value),
            Region::Io => self.io_write16(io_offset(address), value),
            Region::Palette => write_le16(&mut self.ppu.palette, (address & 0x3FF) as usize, value),
            Region::Vram => {
                if let Some(offset) = self.vram_offset(address) {
                    write_le16(&mut self.ppu.vram, offset, value);
                }
            }
            Region::Oam => write_le16(&mut self.ppu.oam, (address & 0x3FF) as usize, value),
            Region::Sram => unreachable!("SRAM handled before alignment"),
            Region::Rom => self.cart.rom_write16(address, value),
            Region::Bios | Region::Unmapped => {}
        }
    }

    pub fn write32(&mut self, address: u32, value: u32) {
        if region_of(address) == Region::Sram {
            return self.sram_write_rotated(address, value, Width::Word);
        }
        let address = address & !3;
        match self.charge(address, Width::Word) {
            Region::Ewram => write_le32(&mut self.ewram, (address as usize) & (EWRAM_SIZE - 1), value),
            Region::Iwram => write_le32(&mut self.iwram, (address as usize) & (IWRAM_SIZE - 1), value),
            Region::Io => self.io_write32(io_offset(address), value),
            Region::Palette => write_le32(&mut self.ppu.palette, (address & 0x3FF) as usize, value),
            Region::Vram => {
                if let Some(offset) = self.vram_offset(address) {
                    write_le32(&mut self.ppu.vram, offset, value);
                }
            }
            Region::Oam => write_le32(&mut self.ppu.oam, (address & 0x3FF) as usize, value),
            Region::Sram => unreachable!("SRAM handled before alignment"),
            Region::Rom => {
                self.cart.rom_write16(address, value as u16);
                self.cart.rom_write16(address + 2, (value >> 16) as u16);
            }
            Region::Bios | Region::Unmapped => {}
        }
    }
}

/// IO offset from 0x04000000. Only 0x000-0x3FF and 0x800-0x803 are registers; the rest reads 0.
/// The internal memory control register at 0x800 repeats every 64K (gbatek: Memory Mirrors).
fn io_offset(address: u32) -> u32 {
    let offset = address & 0x00FF_FFFF;
    let in_64k_block = offset & 0xFFFF;
    if offset < 0x400 {
        offset
    } else if (0x800..0x804).contains(&in_64k_block) {
        in_64k_block
    } else {
        UNMAPPED_IO_OFFSET | (address & 3) // no owner; low bits kept to pick the open-bus half
    }
}

/// The byte of `word` that `address` selects within its aligned word.
fn byte_of_word(word: u32, address: u32) -> u8 {
    (word >> ((address & 3) * 8)) as u8
}

/// The halfword of `word` that `address` selects within its aligned word.
fn halfword_of_word(word: u32, address: u32) -> u16 {
    (word >> ((address & 2) * 8)) as u16
}

/// Little-endian read of `size` bytes from the BIOS. Outside 0x0000-0x3FFF reads 0.
fn bios_read(address: u32, size: usize) -> u32 {
    let offset = address as usize;
    if offset + size > BIOS_SIZE {
        return 0;
    }
    match size {
        1 => BIOS[offset] as u32,
        2 => read_le16(BIOS, offset) as u32,
        _ => read_le32(BIOS, offset),
    }
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
    fn rom_wait_states_follow_waitcnt_and_sequential_access() {
        let mut bus = Bus::new(&[0; 0x40000]);
        bus.read16(0x0800_0100); // non-sequential, default WS0 first access 4 waits
        assert_eq!(bus.take_access_cycles(), 5);
        bus.read16(0x0800_0102); // sequential, WS0 second access 2 waits
        assert_eq!(bus.take_access_cycles(), 3);
        bus.read32(0x0800_0200); // N + S
        assert_eq!(bus.take_access_cycles(), 8);
        bus.write16(0x0400_0204, 0x4317); // WS0 3,1; SRAM 8; WS2 8,8; prefetch on
        bus.take_access_cycles();
        bus.read16(0x0800_0300);
        assert_eq!(bus.take_access_cycles(), 4);
        bus.read16(0x0800_0302);
        assert_eq!(bus.take_access_cycles(), 2);
        bus.read16(0x0801_FFFE);
        bus.take_access_cycles();
        bus.read16(0x0802_0000); // 128K block start is always non-sequential
        assert_eq!(bus.take_access_cycles(), 4);
        bus.read8(0x0E00_0000); // SRAM field 3 = 8 waits
        assert_eq!(bus.take_access_cycles(), 9);
    }

    #[test]
    fn forced_sequential_overrides_address_tracking() {
        let mut bus = Bus::new(&[0; 0x1000]);
        bus.read16(0x0800_0100);
        bus.write16(0x0300_0000, 0);
        bus.take_access_cycles();
        bus.force_next_access_sequential(true);
        bus.read16(0x0800_0102);
        assert_eq!(bus.take_access_cycles(), 3, "S cycle despite the IWRAM write in between");
        bus.read16(0x0800_0104);
        assert_eq!(bus.take_access_cycles(), 3, "tracking resumes after the override");
    }

    #[test]
    fn ewram_wait_follows_memory_control() {
        let mut bus = Bus::new(&[]);
        bus.write32(0x0400_0800, 0x0E00_0020); // 1 wait state
        bus.take_access_cycles();
        bus.read32(0x0200_0000);
        assert_eq!(bus.take_access_cycles(), 4);
    }

    #[test]
    fn sram_wide_accesses_use_the_exact_byte() {
        let mut bus = Bus::new(&[]);
        bus.write8(0x0E00_0001, 0x5A);
        assert_eq!(bus.read16(0x0E00_0001), 0x5A5A);
        assert_eq!(bus.read32(0x0E00_0001), 0x5A5A_5A5A);
        bus.write32(0x0E00_0002, 0x4433_2211); // ROR 16 -> low byte 0x33
        assert_eq!(bus.read8(0x0E00_0002), 0x33);
        assert_eq!(bus.read8(0x0E00_0000), 0xFF, "neighbour untouched");
    }

    #[test]
    fn vram_byte_writes_skip_obj_area() {
        let mut bus = Bus::new(&[]);
        bus.write8(0x0600_0001, 0x12);
        assert_eq!(bus.read16(0x0600_0000), 0x1212);
        bus.write8(0x0601_0000, 0x34);
        assert_eq!(bus.read16(0x0601_0000), 0, "OBJ VRAM ignores byte writes in tiled modes");
        bus.write16(0x0400_0000, 3); // DISPCNT: bitmap mode 3
        bus.write8(0x0601_0000, 0x34);
        assert_eq!(bus.read16(0x0601_0000), 0x3434, "BG area reaches 0x13FFF in bitmap modes");
    }

    #[test]
    fn memory_control_register_mirrors_every_64k() {
        let mut bus = Bus::new(&[]);
        bus.write32(0x0401_0800, 0x0E00_0020);
        assert_eq!(bus.read32(0x0400_0800), 0x0E00_0020);
    }

    #[test]
    fn unreadable_io_and_unused_memory_follow_open_bus_rules() {
        let mut bus = Bus::new(&[]);
        bus.iwram[0x108..0x10C].copy_from_slice(&0xE1A0_0000u32.to_le_bytes()); // [$+8]
        bus.note_instruction_fetch(0x0300_0100, false);
        bus.write16(0x0400_00B8, 0x1234); // DMA0CNT_L, write-only, DMA0CNT_H readable
        assert_eq!(bus.read16(0x0400_00B8), 0);
        assert_eq!(bus.read32(0x0400_00B0), 0xE1A0_0000, "DMA0SAD fragment is fully write-only");
        assert_eq!(bus.read32(0x0400_00E0), 0xE1A0_0000, "unused IO");
        assert_eq!(bus.read16(0x1000_0002), 0xE1A0, "unused memory");
        bus.write16(0x0400_0010, 7); // BG0HOFS, write-only, BG0VOFS too
        assert_eq!(bus.read16(0x0400_0010), 0x0000, "open bus low half of 0xE1A00000");
    }

    #[test]
    fn unmapped_io_halves_follow_the_address() {
        let mut bus = Bus::new(&[]);
        bus.iwram[0x108..0x10C].copy_from_slice(&0x1111_2222u32.to_le_bytes());
        bus.note_instruction_fetch(0x0300_0100, false);
        assert_eq!(bus.read16(0x0400_0400), 0x2222);
        assert_eq!(bus.read16(0x0400_0402), 0x1111);
        assert_eq!(bus.read8(0x0400_0403), 0x11);
    }

    #[test]
    fn bios_reads_are_protected_outside_the_bios() {
        let mut bus = Bus::new(&[]);
        bus.note_instruction_fetch(0x0000_0100, false);
        let protected_value = bios_read(0x108, 4);
        assert_eq!(bus.read32(0x0000_0000), bios_read(0, 4), "readable from inside the BIOS");
        bus.note_instruction_fetch(0x0800_0000, false);
        assert_eq!(bus.read32(0x0000_0000), protected_value);
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
