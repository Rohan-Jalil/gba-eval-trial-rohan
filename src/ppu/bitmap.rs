//! Bitmap modes 3, 4 and 5, drawn as BG2 (gbatek: LCD VRAM Bitmap BG Modes).

use super::{LayerPixel, Ppu, SCREEN_WIDTH};

/// Offset of frame 1 in modes 4 and 5.
const FRAME_ONE_OFFSET: usize = 0xA000;
const MODE5_WIDTH: usize = 160;
const MODE5_HEIGHT: usize = 128;

/// Render one scanline of the bitmap background (BG2) into `output`.
// ponytail: identity mapping; the BG2 affine registers (BG2PA-PD, BG2X/Y) are Phase 3 work.
pub fn render_bitmap_line(ppu: &Ppu, line: usize, output: &mut [LayerPixel; SCREEN_WIDTH]) {
    let bg_mode = ppu.dispcnt() & 7;
    let priority = (ppu.background_control(2) & 3) as u8;
    let frame_base = if ppu.displays_frame_one() { FRAME_ONE_OFFSET } else { 0 };
    for (x, pixel) in output.iter_mut().enumerate() {
        let (color, is_transparent) = match bg_mode {
            3 => (ppu.vram_halfword((line * SCREEN_WIDTH + x) * 2), false),
            4 => {
                let palette_index = ppu.vram_byte(frame_base + line * SCREEN_WIDTH + x) as usize;
                (ppu.palette_color(palette_index), palette_index == 0)
            }
            _ if x < MODE5_WIDTH && line < MODE5_HEIGHT => {
                (ppu.vram_halfword(frame_base + (line * MODE5_WIDTH + x) * 2), false)
            }
            _ => (0, true),
        };
        *pixel = LayerPixel { color, priority, is_transparent, is_semi_transparent_sprite: false };
    }
}
