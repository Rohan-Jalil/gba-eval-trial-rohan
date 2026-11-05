//! Tiled (text) backgrounds for modes 0-1 (gbatek: LCD VRAM Character Data, LCD VRAM BG Screen Data Format).
//!
//! Phase 0 stub; filled in by the Phase 1 PPU agent.
#![allow(dead_code)] // not wired into the compositor yet

use super::{LayerPixel, Ppu, SCREEN_WIDTH};

/// Render one scanline of a text background into `output`. Stub: every pixel transparent.
pub fn render_text_background_line(
    _ppu: &Ppu,
    _background_index: usize,
    _line: usize,
    output: &mut [LayerPixel; SCREEN_WIDTH],
) {
    output.fill(LayerPixel { is_transparent: true, ..LayerPixel::default() });
}
