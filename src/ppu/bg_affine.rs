//! Rotation/scaling backgrounds for modes 1-2 (gbatek: LCD I/O BG Rotation/Scaling).
//!
//! Phase 0 stub; filled in by the Phase 3 affine + bitmap agent.
#![allow(dead_code)] // not wired into the compositor yet

use super::{LayerPixel, Ppu, SCREEN_WIDTH};

/// Render one scanline of an affine background into `output`. Stub: every pixel transparent.
pub fn render_affine_background_line(
    _ppu: &Ppu,
    _background_index: usize,
    _line: usize,
    output: &mut [LayerPixel; SCREEN_WIDTH],
) {
    output.fill(LayerPixel { is_transparent: true, ..LayerPixel::default() });
}
