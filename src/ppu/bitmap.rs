//! Bitmap modes 3, 4 and 5 (gbatek: LCD VRAM Bitmap BG Modes).
//!
//! Phase 0 stub; filled in by the Phase 1 PPU agent.
#![allow(dead_code)] // not wired into the compositor yet

use super::{LayerPixel, Ppu, SCREEN_WIDTH};

/// Render one scanline of the bitmap background (BG2) into `output`. Stub: every pixel transparent.
pub fn render_bitmap_line(_ppu: &Ppu, _line: usize, output: &mut [LayerPixel; SCREEN_WIDTH]) {
    output.fill(LayerPixel { is_transparent: true, ..LayerPixel::default() });
}
