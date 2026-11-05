//! LCD video controller, IO 0x000-0x05F: line timing, DISPSTAT/VCOUNT, framebuffer
//! (gbatek: GBA LCD Video Controller, LCD Dimensions and Timings, LCD I/O Display Control).
//!
//! The PPU owns palette RAM, VRAM and OAM; the bus reads and writes them directly.
//! Phase 0: real line timing and IRQs, no rendering (every visible line is drawn black).

mod bg_affine;
mod bg_text;
mod bitmap;
mod effects;
mod obj;

use crate::io::{merge, PlainRegisters};
use crate::irq::{Irq, IrqSource};

pub const SCREEN_WIDTH: usize = 240;
pub const SCREEN_HEIGHT: usize = 160;

pub const PALETTE_SIZE: usize = 0x400; // BG + OBJ palette RAM, 1 KiB
pub const VRAM_SIZE: usize = 0x18000; // 96 KiB
pub const OAM_SIZE: usize = 0x400; // OBJ attributes, 1 KiB

/// 4 cycles per dot, 308 dots per line (gbatek: LCD Dimensions and Timings).
pub const CYCLES_PER_LINE: u32 = 1232;
/// Lines per frame, visible plus VBlank.
pub const LINES_PER_FRAME: u16 = 228;
/// First VBlank line.
pub const VBLANK_START_LINE: u16 = 160;
/// Cycle within a line at which the DISPSTAT H-Blank flag goes up. gbatek: "Although the drawing
/// time is only 960 cycles (240*4), the H-Blank flag is 0 for a total of 1006 cycles."
/// HBlank IRQ and HBlank DMA are raised at the same point. Tune here if timing tests disagree.
pub const HBLANK_START_CYCLE: u32 = 1006;

const IO_START: u32 = 0x000;
const DISPSTAT: u32 = 0x004; // general LCD status: VBlank/HBlank/VCount flags, IRQ enables, LYC
const VCOUNT: u32 = 0x006; // current scanline (read only)

const DISPSTAT_VBLANK_FLAG: u16 = 1 << 0;
const DISPSTAT_HBLANK_FLAG: u16 = 1 << 1;
const DISPSTAT_VCOUNT_FLAG: u16 = 1 << 2;
const DISPSTAT_VBLANK_IRQ_ENABLE: u16 = 1 << 3;
const DISPSTAT_HBLANK_IRQ_ENABLE: u16 = 1 << 4;
const DISPSTAT_VCOUNT_IRQ_ENABLE: u16 = 1 << 5;
/// Bits the CPU may write: the three IRQ enables and the LYC byte.
const DISPSTAT_WRITABLE: u16 = 0xFF38;

/// Opaque black in the ABI framebuffer format 0xAABBGGRR.
const BLACK: u32 = 0xFF00_0000;

/// One pixel of one layer for one scanline; only the compositor merges layers (Phase 3 contract).
#[derive(Clone, Copy, Default, Debug)]
pub struct LayerPixel {
    /// BGR555 colour.
    pub color: u16,
    /// 0 (front) to 3 (back).
    pub priority: u8,
    pub is_transparent: bool,
    pub is_semi_transparent_sprite: bool,
}

/// What happened during a `Ppu::step`, for gba.rs to forward to DMA.
#[derive(Clone, Copy, Default, Debug)]
pub struct PpuEvents {
    /// HBlank started on a visible line (HBlank DMA trigger).
    pub hblank_started: bool,
    /// Line 160 started (VBlank DMA trigger).
    pub vblank_started: bool,
}

pub struct Ppu {
    pub palette: Vec<u8>,
    pub vram: Vec<u8>,
    pub oam: Vec<u8>,
    /// 240x160 pixels in the ABI format 0xAABBGGRR.
    pub framebuffer: Vec<u32>,
    /// Current scanline, 0-227 (VCOUNT).
    pub vcount: u16,
    /// Cycle within the current line, 0-1231.
    pub line_cycle: u32,
    dispstat: u16,
    /// DISPCNT, BGxCNT, scroll, affine, window, blend registers: stored, read back as written.
    registers: PlainRegisters<0x30>,
}

impl Ppu {
    pub fn new() -> Self {
        Ppu {
            palette: vec![0; PALETTE_SIZE],
            vram: vec![0; VRAM_SIZE],
            oam: vec![0; OAM_SIZE],
            framebuffer: vec![BLACK; SCREEN_WIDTH * SCREEN_HEIGHT],
            vcount: 0,
            line_cycle: 0,
            dispstat: 0,
            registers: PlainRegisters::new(IO_START),
        }
    }

    /// Advance line timing by `cycles`, raising HBlank/VBlank/VCount IRQs on `irq`.
    pub fn step(&mut self, cycles: u32, irq: &mut Irq) -> PpuEvents {
        let mut events = PpuEvents::default();
        let mut remaining = cycles;
        while remaining > 0 {
            let next_boundary =
                if self.line_cycle < HBLANK_START_CYCLE { HBLANK_START_CYCLE } else { CYCLES_PER_LINE };
            let advance = remaining.min(next_boundary - self.line_cycle);
            self.line_cycle += advance;
            remaining -= advance;
            if self.line_cycle == HBLANK_START_CYCLE {
                self.enter_hblank(irq, &mut events);
            } else if self.line_cycle == CYCLES_PER_LINE {
                self.line_cycle = 0;
                self.start_next_line(irq, &mut events);
            }
        }
        events
    }

    /// HBlank begins: draw the finished visible line, set the flag, raise IRQ.
    /// gbatek DISPSTAT: H-Blank conditions occur on every line, including VBlank lines.
    fn enter_hblank(&mut self, irq: &mut Irq, events: &mut PpuEvents) {
        if self.vcount < VBLANK_START_LINE {
            self.render_line(self.vcount as usize);
            events.hblank_started = true;
        }
        self.dispstat |= DISPSTAT_HBLANK_FLAG;
        if self.dispstat & DISPSTAT_HBLANK_IRQ_ENABLE != 0 {
            irq.request(IrqSource::HBlank);
        }
    }

    /// Next scanline: update VCOUNT, the VBlank flag (lines 160-226) and the VCount match.
    fn start_next_line(&mut self, irq: &mut Irq, events: &mut PpuEvents) {
        self.dispstat &= !DISPSTAT_HBLANK_FLAG;
        self.vcount = (self.vcount + 1) % LINES_PER_FRAME;

        if self.vcount == VBLANK_START_LINE {
            self.dispstat |= DISPSTAT_VBLANK_FLAG;
            events.vblank_started = true;
            if self.dispstat & DISPSTAT_VBLANK_IRQ_ENABLE != 0 {
                irq.request(IrqSource::VBlank);
            }
        } else if self.vcount == LINES_PER_FRAME - 1 {
            self.dispstat &= !DISPSTAT_VBLANK_FLAG;
        }
        self.update_vcount_match(irq);
    }

    /// Compare VCOUNT with the LYC setting (DISPSTAT bits 8-15).
    fn update_vcount_match(&mut self, irq: &mut Irq) {
        let lyc = self.dispstat >> 8;
        if self.vcount == lyc {
            self.dispstat |= DISPSTAT_VCOUNT_FLAG;
            if self.dispstat & DISPSTAT_VCOUNT_IRQ_ENABLE != 0 {
                irq.request(IrqSource::VCounterMatch);
            }
        } else {
            self.dispstat &= !DISPSTAT_VCOUNT_FLAG;
        }
    }

    /// Draw one visible scanline into the framebuffer. Stub: black.
    fn render_line(&mut self, line: usize) {
        let start = line * SCREEN_WIDTH;
        self.framebuffer[start..start + SCREEN_WIDTH].fill(BLACK);
    }

    pub fn io_read16(&mut self, offset: u32) -> u16 {
        match offset {
            DISPSTAT => self.dispstat,
            VCOUNT => self.vcount,
            _ => self.registers.read(offset),
        }
    }

    pub fn io_write16(&mut self, offset: u32, value: u16, mask: u16) {
        match offset {
            DISPSTAT => self.dispstat = merge(self.dispstat, value, mask & DISPSTAT_WRITABLE),
            VCOUNT => {}
            _ => self.registers.write(offset, value, mask),
        }
    }
}
