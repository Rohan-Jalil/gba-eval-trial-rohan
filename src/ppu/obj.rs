//! Sprites, normal and affine (gbatek: LCD OBJ - Overview, LCD OBJ - OAM Attributes).
//!
//! Phase 0 stub; filled in by the Phase 3 sprites agent.
#![allow(dead_code)] // not wired into the compositor yet

use super::{LayerPixel, Ppu, SCREEN_WIDTH};

/// Render one scanline of all sprites into `output` (front-most sprite per pixel, its OBJ
/// priority in `priority`) and mark OBJ-window pixels in `obj_window`. Stub: nothing drawn.
pub fn render_sprite_line(
    _ppu: &Ppu,
    _line: usize,
    output: &mut [LayerPixel; SCREEN_WIDTH],
    obj_window: &mut [bool; SCREEN_WIDTH],
) {
    output.fill(LayerPixel { is_transparent: true, ..LayerPixel::default() });
    obj_window.fill(false);
}
