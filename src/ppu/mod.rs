//! LCD video controller, IO 0x000-0x05F: line timing, DISPSTAT/VCOUNT, framebuffer
//! (gbatek: GBA LCD Video Controller, LCD Dimensions and Timings, LCD I/O Display Control).
//!
//! The PPU owns palette RAM, VRAM and OAM; the bus reads and writes them directly.
//! Each visible line is rendered at HBlank start into a `LineLayers`: every enabled BG renders
//! into its own `[LayerPixel; 240]`, sprites into one more plus the OBJ-window mask, and
//! `compose_line` (the only place layers are merged) produces the framebuffer line.

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
const DISPCNT: u32 = 0x000; // display control: BG mode, frame select, forced blank, layer enables
const BG0CNT: u32 = 0x008; // BG0 control; BG1CNT-BG3CNT follow every 2 bytes
const BG0HOFS: u32 = 0x010; // BG0 X scroll; BG0VOFS follows, then BG1HOFS... every 4 bytes
const BG2X: u32 = 0x028; // BG2 reference point X (28-bit), BG2Y at 0x02C; BG3X/BG3Y at 0x038/0x03C
const BG2PA: u32 = 0x020; // BG2 affine parameters PA, PB, PC, PD; BG3PA at 0x030
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

const DISPCNT_BG_MODE_MASK: u16 = 0x7;
const DISPCNT_FRAME_SELECT: u16 = 1 << 4;
const DISPCNT_FORCED_BLANK: u16 = 1 << 7;
const DISPCNT_BG0_ENABLE: u16 = 1 << 8;
const DISPCNT_OBJ_ENABLE: u16 = 1 << 12;

/// Opaque black in the ABI framebuffer format 0xAABBGGRR.
const BLACK: u32 = 0xFF00_0000;
/// Opaque white, shown on every line during forced blank (gbatek: LCD I/O Display Control).
const WHITE: u32 = 0xFFFF_FFFF;

const TRANSPARENT_PIXEL: LayerPixel =
    LayerPixel { color: 0, priority: 3, is_transparent: true, is_semi_transparent_sprite: false };

/// How a BG layer is drawn in the current BG mode (gbatek: LCD I/O Display Control, mode table).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BackgroundKind {
    Text,
    Affine,
    Bitmap,
    Unused,
}

/// Layer kind of BG `background_index` in `bg_mode`.
fn background_kind(bg_mode: u16, background_index: usize) -> BackgroundKind {
    use BackgroundKind::*;
    match (bg_mode, background_index) {
        (0, _) => Text,
        (1, 0 | 1) => Text,
        (1, 2) => Affine,
        (2, 2 | 3) => Affine,
        (3..=5, 2) => Bitmap,
        _ => Unused,
    }
}

/// BGR555 to 0xAABBGGRR; each 5-bit channel is widened by repeating its top bits.
fn bgr555_to_abgr8888(color: u16) -> u32 {
    let widen = |channel: u16| -> u32 {
        let channel = (channel & 0x1F) as u32;
        (channel << 3) | (channel >> 2)
    };
    let red = widen(color);
    let green = widen(color >> 5);
    let blue = widen(color >> 10);
    0xFF00_0000 | (blue << 16) | (green << 8) | red
}

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

/// Everything the layer renderers produce for one scanline; `compose_line` merges it
/// (Phase 3 contract: renderers fill their own entry, only the compositor reads them all).
pub struct LineLayers {
    /// BG0-BG3, each 240 pixels.
    pub backgrounds: [[LayerPixel; SCREEN_WIDTH]; 4],
    /// Whether each BG is enabled and exists in the current BG mode.
    pub background_enabled: [bool; 4],
    /// All sprites merged into one layer (obj.rs resolves sprite-vs-sprite priority).
    pub sprites: [LayerPixel; SCREEN_WIDTH],
    /// DISPCNT OBJ enable.
    pub sprites_enabled: bool,
    /// Pixels covered by an opaque OBJ-window sprite (gbatek: LCD I/O Window Feature, OBJ Window).
    pub obj_window: [bool; SCREEN_WIDTH],
}

impl LineLayers {
    fn new() -> Self {
        LineLayers {
            backgrounds: [[TRANSPARENT_PIXEL; SCREEN_WIDTH]; 4],
            background_enabled: [false; 4],
            sprites: [TRANSPARENT_PIXEL; SCREEN_WIDTH],
            sprites_enabled: false,
            obj_window: [false; SCREEN_WIDTH],
        }
    }
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
    /// Internal BG2/BG3 reference points (X, Y), signed 20.8 fixed point (BGxX/BGxY are 28-bit).
    /// Reloaded from BGxX/BGxY on write and at VBlank, advanced by PB/PD after every visible line
    /// (gbatek: LCD I/O BG Rotation/Scaling, "Internal Reference Point Registers").
    affine_reference: [(i32, i32); 2],
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
            // VCOUNT 0 equals the power-on LYC 0, so the match flag starts set.
            dispstat: DISPSTAT_VCOUNT_FLAG,
            registers: {
                // BG2/BG3 PA and PD power on as 1.0 (identity): bitmap-mode ROMs that never
                // write them still show an unscaled picture. gbatek gives no reset value.
                let mut registers = PlainRegisters::new(IO_START);
                for offset in [BG2PA, BG2PA + 6, BG2PA + 0x10, BG2PA + 0x16] {
                    registers.write(offset, 0x100, 0xFFFF);
                }
                registers
            },
            affine_reference: [(0, 0); 2],
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
            self.advance_affine_reference_points();
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
            self.reload_affine_reference_points();
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

    /// Compare VCOUNT with the LYC setting (DISPSTAT bits 8-15) at line start and raise the IRQ.
    fn update_vcount_match(&mut self, irq: &mut Irq) {
        if self.refresh_vcount_flag() && self.dispstat & DISPSTAT_VCOUNT_IRQ_ENABLE != 0 {
            irq.request(IrqSource::VCounterMatch);
        }
    }

    /// Set or clear the V-Counter flag from VCOUNT == LYC; returns whether they match.
    fn refresh_vcount_flag(&mut self) -> bool {
        let matches = self.vcount == self.dispstat >> 8;
        if matches {
            self.dispstat |= DISPSTAT_VCOUNT_FLAG;
        } else {
            self.dispstat &= !DISPSTAT_VCOUNT_FLAG;
        }
        matches
    }

    /// DISPCNT (gbatek: LCD I/O Display Control).
    pub(crate) fn dispcnt(&self) -> u16 {
        self.registers.read(DISPCNT)
    }

    /// Raw value of any PPU register stored at `offset` (0x000-0x05F), e.g. WININ, BLDCNT,
    /// MOSAIC. Unlike `io_read16`, write-only registers return what was written.
    #[allow(dead_code)] // used by the Phase 3 effects/sprites workers
    pub(crate) fn register(&self, offset: u32) -> u16 {
        self.registers.read(offset)
    }

    /// Affine parameters PA, PB, PC, PD of BG2 (`affine_index` 0) or BG3 (1), signed 8.8 fixed
    /// point (gbatek: LCD I/O BG Rotation/Scaling).
    pub(crate) fn affine_parameters(&self, affine_index: usize) -> [i16; 4] {
        let base = BG2PA + 0x10 * affine_index as u32;
        [0, 1, 2, 3].map(|parameter| self.registers.read(base + 2 * parameter) as i16)
    }

    /// Current internal reference point (X, Y) of BG2 (`affine_index` 0) or BG3 (1) for the line
    /// being drawn, signed 20.8 fixed point.
    pub(crate) fn affine_reference_point(&self, affine_index: usize) -> (i32, i32) {
        self.affine_reference[affine_index]
    }

    /// BGxX/BGxY as written: 28-bit two's complement, sign-extended to i32.
    fn written_reference_point(&self, affine_index: usize) -> (i32, i32) {
        let base = BG2X + 0x10 * affine_index as u32;
        let read28 = |offset: u32| {
            let raw = self.registers.read(offset) as u32 | (self.registers.read(offset + 2) as u32) << 16;
            ((raw << 4) as i32) >> 4
        };
        (read28(base), read28(base + 4))
    }

    /// Copy BGxX/BGxY into the internal reference points (at VBlank).
    fn reload_affine_reference_points(&mut self) {
        for affine_index in 0..2 {
            self.affine_reference[affine_index] = self.written_reference_point(affine_index);
        }
    }

    /// After each visible line the internal points move by (PB, PD) (gbatek: dmx, dmy).
    fn advance_affine_reference_points(&mut self) {
        for affine_index in 0..2 {
            let [_, parameter_b, _, parameter_d] = self.affine_parameters(affine_index);
            let (x, y) = &mut self.affine_reference[affine_index];
            *x = x.wrapping_add(parameter_b as i32);
            *y = y.wrapping_add(parameter_d as i32);
        }
    }

    /// BGxCNT for BG `background_index` (gbatek: LCD I/O BG Control).
    pub(crate) fn background_control(&self, background_index: usize) -> u16 {
        self.registers.read(BG0CNT + 2 * background_index as u32)
    }

    /// (BGxHOFS, BGxVOFS), 9 bits each (gbatek: LCD I/O BG Scrolling).
    pub(crate) fn background_scroll(&self, background_index: usize) -> (u16, u16) {
        let horizontal = self.registers.read(BG0HOFS + 4 * background_index as u32);
        let vertical = self.registers.read(BG0HOFS + 4 * background_index as u32 + 2);
        (horizontal & 0x1FF, vertical & 0x1FF)
    }

    /// DISPCNT frame select bit for bitmap modes 4 and 5.
    pub(crate) fn displays_frame_one(&self) -> bool {
        self.dispcnt() & DISPCNT_FRAME_SELECT != 0
    }

    /// BGR555 colour at palette RAM entry `index` (0-255 BG, 256-511 OBJ).
    pub(crate) fn palette_color(&self, index: usize) -> u16 {
        let offset = (index * 2) & (PALETTE_SIZE - 1);
        u16::from_le_bytes([self.palette[offset], self.palette[offset + 1]])
    }

    /// VRAM byte at `offset`; offsets past the end read 0.
    pub(crate) fn vram_byte(&self, offset: usize) -> u8 {
        self.vram.get(offset).copied().unwrap_or(0)
    }

    /// VRAM halfword at `offset` (little endian).
    pub(crate) fn vram_halfword(&self, offset: usize) -> u16 {
        u16::from_le_bytes([self.vram_byte(offset), self.vram_byte(offset + 1)])
    }

    /// Draw one visible scanline into the framebuffer.
    fn render_line(&mut self, line: usize) {
        let start = line * SCREEN_WIDTH;
        let dispcnt = self.dispcnt();
        if dispcnt & DISPCNT_FORCED_BLANK != 0 {
            self.framebuffer[start..start + SCREEN_WIDTH].fill(WHITE);
            return;
        }
        let bg_mode = dispcnt & DISPCNT_BG_MODE_MASK;
        let mut layers = LineLayers::new();
        for background_index in 0..4 {
            if dispcnt & (DISPCNT_BG0_ENABLE << background_index) == 0 {
                continue;
            }
            let layer = &mut layers.backgrounds[background_index];
            layers.background_enabled[background_index] = true;
            match background_kind(bg_mode, background_index) {
                BackgroundKind::Text => bg_text::render_text_background_line(self, background_index, line, layer),
                BackgroundKind::Affine => {
                    bg_affine::render_affine_background_line(self, background_index, line, layer)
                }
                BackgroundKind::Bitmap => bitmap::render_bitmap_line(self, line, layer),
                BackgroundKind::Unused => layers.background_enabled[background_index] = false,
            }
        }
        // The OBJ window works even when OBJ display is off, so sprites always render.
        layers.sprites_enabled = dispcnt & DISPCNT_OBJ_ENABLE != 0;
        obj::render_sprite_line(self, line, &mut layers.sprites, &mut layers.obj_window);
        self.compose_line(line, &layers);
    }

    /// Merge the line's layers over the backdrop (gbatek: LCD I/O BG Control, "BG0 is having
    /// the highest" priority on ties; LCD OBJ - OAM Attributes, a sprite is in front of a BG
    /// of the same priority; LCD Color Palettes, Backdrop Color).
    // ponytail: priority only; the Phase 3 effects worker adds windows and blending here.
    fn compose_line(&mut self, line: usize, layers: &LineLayers) {
        let backdrop = self.palette_color(0);
        let start = line * SCREEN_WIDTH;
        for x in 0..SCREEN_WIDTH {
            let mut best: Option<LayerPixel> = None;
            if layers.sprites_enabled && !layers.sprites[x].is_transparent {
                best = Some(layers.sprites[x]);
            }
            for (background_index, layer) in layers.backgrounds.iter().enumerate() {
                let pixel = layer[x];
                if !layers.background_enabled[background_index] || pixel.is_transparent {
                    continue;
                }
                if best.is_none_or(|current| pixel.priority < current.priority) {
                    best = Some(pixel);
                }
            }
            let color = best.map_or(backdrop, |pixel| pixel.color);
            self.framebuffer[start + x] = bgr555_to_abgr8888(color);
        }
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
            DISPSTAT => {
                self.dispstat = merge(self.dispstat, value, mask & DISPSTAT_WRITABLE);
                // ponytail: flag only; a write that creates a match does not raise the IRQ
                // (io_write16 has no Irq access, and gbatek does not say whether it should).
                self.refresh_vcount_flag();
            }
            VCOUNT => {}
            // Writing a reference point register also reloads the internal point at once.
            0x028..=0x02E | 0x038..=0x03E => {
                self.registers.write(offset, value, mask);
                let affine_index = ((offset - BG2X) / 0x10) as usize;
                self.affine_reference[affine_index] = self.written_reference_point(affine_index);
            }
            _ => self.registers.write(offset, value, mask),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::Bus;

    #[test]
    fn affine_reference_point_reloads_on_write_and_advances_per_line() {
        let mut bus = Bus::new(&[]);
        bus.write32(0x0400_0028, 0x0FFF_FF00); // BG2X = -1.0 (28-bit two's complement)
        bus.write16(0x0400_0022, 0x0080); // BG2PB = 0.5
        assert_eq!(bus.ppu.affine_reference_point(0), (-256, 0));
        let mut irq = crate::irq::Irq::new();
        bus.ppu.step(CYCLES_PER_LINE, &mut irq); // line 0 drawn, point advanced
        assert_eq!(bus.ppu.affine_reference_point(0), (-128, 256), "PD powers on as 1.0");
    }

    const RED: u16 = 0x001F;
    const GREEN: u16 = 0x03E0;
    const BLUE: u16 = 0x7C00;

    fn pixel(bus: &Bus, x: usize, line: usize) -> u32 {
        bus.ppu.framebuffer[line * SCREEN_WIDTH + x]
    }

    #[test]
    fn color_conversion_fills_all_eight_bits() {
        assert_eq!(bgr555_to_abgr8888(0x7FFF), 0xFFFF_FFFF);
        assert_eq!(bgr555_to_abgr8888(RED), 0xFF00_00FF);
        assert_eq!(bgr555_to_abgr8888(BLUE), 0xFFFF_0000);
    }

    #[test]
    fn mode3_draws_vram_colours_and_forced_blank_is_white() {
        let mut bus = Bus::new(&[]);
        bus.write16(0x0400_0000, 0x0403); // mode 3, BG2 on
        bus.write16(0x0600_0000 + (10 * 240 + 5) * 2, GREEN);
        bus.write32(0x0400_002C, 10 << 8); // BG2Y: the internal point as it stands on line 10
        bus.ppu.render_line(10);
        assert_eq!(pixel(&bus, 5, 10), 0xFF00_FF00);
        assert_eq!(pixel(&bus, 6, 10), BLACK);
        bus.write16(0x0400_0000, 0x0483);
        bus.ppu.render_line(10);
        assert_eq!(pixel(&bus, 5, 10), WHITE);
    }

    #[test]
    fn mode4_uses_frame_select_and_index_zero_shows_backdrop() {
        let mut bus = Bus::new(&[]);
        bus.write16(0x0500_0000, BLUE); // backdrop
        bus.write16(0x0500_0002, RED);
        bus.write16(0x0600_A000, 0x0001); // frame 1: x=0 -> index 1, x=1 -> index 0
        bus.write16(0x0400_0000, 0x0414); // mode 4, frame 1, BG2 on
        bus.ppu.render_line(0);
        assert_eq!(pixel(&bus, 0, 0), bgr555_to_abgr8888(RED));
        assert_eq!(pixel(&bus, 1, 0), bgr555_to_abgr8888(BLUE));
        bus.write16(0x0400_0000, 0x0404); // frame 0 is empty
        bus.ppu.render_line(0);
        assert_eq!(pixel(&bus, 0, 0), bgr555_to_abgr8888(BLUE));
    }

    #[test]
    fn mode5_is_160_by_128_with_backdrop_outside() {
        let mut bus = Bus::new(&[]);
        bus.write16(0x0500_0000, BLUE);
        bus.write16(0x0600_0000 + (2 * 160 + 159) * 2, RED);
        bus.write16(0x0400_0000, 0x0405);
        bus.write32(0x0400_002C, 2 << 8); // BG2Y: the internal point as it stands on line 2
        bus.ppu.render_line(2);
        assert_eq!(pixel(&bus, 159, 2), bgr555_to_abgr8888(RED));
        assert_eq!(pixel(&bus, 160, 2), bgr555_to_abgr8888(BLUE));
    }

    #[test]
    fn text_bg_scrolls_flips_and_selects_screen_blocks() {
        let mut bus = Bus::new(&[]);
        bus.write16(0x0500_0000 + (2 * 16 + 1) * 2, GREEN); // palette bank 2, colour 1
        bus.write16(0x0600_0000 + 32, 1); // 4bpp tile 1: dot (0,0) = colour 1
        bus.write16(0x0400_0008, (8 << 8) | (1 << 14)); // BG0: 4bpp, map at 0x4000, 512x256
        // SC1 (right 256 pixels), map row 0 column 0: tile 1, palette bank 2, horizontal flip.
        bus.write16(0x0600_4000 + 0x800, 1 | (1 << 10) | (2 << 12));
        bus.write16(0x0400_0010, 256); // BG0HOFS: SC1 starts at the left screen edge
        bus.write16(0x0400_0000, 0x0100); // mode 0, BG0 on
        bus.ppu.render_line(0);
        assert_eq!(pixel(&bus, 7, 0), bgr555_to_abgr8888(GREEN), "flipped dot lands at x=7");
        assert_eq!(pixel(&bus, 0, 0), BLACK);
        // HOFS 255: x=0 shows background column 255, so SC1 column 0 starts at x=1.
        bus.write16(0x0400_0010, 255);
        bus.ppu.render_line(0);
        assert_eq!(pixel(&bus, 8, 0), bgr555_to_abgr8888(GREEN));
        // VOFS 1 shows dot row 1 on line 0, which is empty.
        bus.write16(0x0400_0010, 256);
        bus.write16(0x0400_0012, 1);
        bus.ppu.render_line(0);
        assert_eq!(pixel(&bus, 7, 0), BLACK);
    }

    #[test]
    fn text_bg_8bpp_and_priority_order() {
        let mut bus = Bus::new(&[]);
        bus.write16(0x0500_0000 + 5 * 2, RED);
        bus.write16(0x0500_0000 + 2, BLUE);
        bus.write16(0x0600_0000 + 64, 5); // 8bpp tile 1: dot (0,0) = index 5
        bus.write16(0x0600_0000 + 32 * 5, 1); // 4bpp tile 5: dot (0,0) = index 1
        bus.write16(0x0600_4000, 1); // BG0 map (block 8): tile 1
        bus.write16(0x0600_4800, 5); // BG1 map (block 9): tile 5
        bus.write16(0x0400_0008, (8 << 8) | (1 << 7) | 1); // BG0: 8bpp, priority 1
        bus.write16(0x0400_000A, 9 << 8); // BG1: 4bpp, priority 0
        bus.write16(0x0400_0000, 0x0300);
        bus.ppu.render_line(0);
        assert_eq!(pixel(&bus, 0, 0), bgr555_to_abgr8888(BLUE), "BG1 priority 0 wins");
        bus.write16(0x0400_000A, (9 << 8) | 1); // equal priority: BG0 wins
        bus.ppu.render_line(0);
        assert_eq!(pixel(&bus, 0, 0), bgr555_to_abgr8888(RED));
    }

    #[test]
    fn vcount_flag_tracks_lyc_writes_and_power_on() {
        let mut bus = Bus::new(&[]);
        assert_ne!(bus.read16(0x0400_0004) & DISPSTAT_VCOUNT_FLAG, 0);
        bus.write16(0x0400_0004, 5 << 8);
        assert_eq!(bus.read16(0x0400_0004) & DISPSTAT_VCOUNT_FLAG, 0);
        bus.write16(0x0400_0004, 0);
        assert_ne!(bus.read16(0x0400_0004) & DISPSTAT_VCOUNT_FLAG, 0);
    }
}
