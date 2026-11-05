//! Rotation/scaling backgrounds for modes 1-2 (gbatek: LCD I/O BG Rotation/Scaling,
//! LCD VRAM BG Screen Data Format (BG Map), Rotation/Scaling BG Screen).

use super::{LayerPixel, Ppu, SCREEN_WIDTH};

const CHARACTER_BLOCK_SIZE: usize = 0x4000; // 16 KiB per character base block
const SCREEN_BLOCK_SIZE: usize = 0x800; // 2 KiB per screen base block
const BGCNT_AREA_OVERFLOW_WRAP: u16 = 1 << 13; // BGxCNT bit 13: wrap instead of transparent

/// Texture coordinate walker for one affine scanline: starts at the internal reference point
/// and steps by (PA, PC) per screen pixel, all in signed 20.8 fixed point.
pub(super) struct AffineWalk {
    pub texture_x: i32,
    pub texture_y: i32,
    step_x: i32,
    step_y: i32,
}

impl AffineWalk {
    /// Walker for BG2 (`affine_index` 0) or BG3 (1) on the line currently being drawn.
    pub(super) fn for_line(ppu: &Ppu, affine_index: usize) -> Self {
        let (texture_x, texture_y) = ppu.affine_reference_point(affine_index);
        let [parameter_a, _, parameter_c, _] = ppu.affine_parameters(affine_index);
        AffineWalk { texture_x, texture_y, step_x: parameter_a as i32, step_y: parameter_c as i32 }
    }

    /// Integer texture pixel (x, y) of the current screen pixel.
    pub(super) fn pixel(&self) -> (i32, i32) {
        (self.texture_x >> 8, self.texture_y >> 8)
    }

    /// Move to the next screen pixel.
    pub(super) fn advance(&mut self) {
        self.texture_x = self.texture_x.wrapping_add(self.step_x);
        self.texture_y = self.texture_y.wrapping_add(self.step_y);
    }
}

/// Render one scanline of an affine background into `output`: 8bpp tiles, one-byte map
/// entries, square maps of 128-1024 pixels, wrapped or transparent outside the area.
// ponytail: no BG mosaic here; add it when the effects worker exposes a mosaic helper.
pub fn render_affine_background_line(
    ppu: &Ppu,
    background_index: usize,
    _line: usize,
    output: &mut [LayerPixel; SCREEN_WIDTH],
) {
    let control = ppu.background_control(background_index);
    let priority = (control & 3) as u8;
    let character_base = ((control >> 2) & 3) as usize * CHARACTER_BLOCK_SIZE;
    let screen_base = ((control >> 8) & 0x1F) as usize * SCREEN_BLOCK_SIZE;
    let wraps = control & BGCNT_AREA_OVERFLOW_WRAP != 0;
    let size_pixels = 128i32 << (control >> 14); // 128, 256, 512 or 1024
    let tiles_per_row = (size_pixels / 8) as usize;

    let mut walk = AffineWalk::for_line(ppu, background_index - 2);
    for pixel in output.iter_mut() {
        let (mut texture_x, mut texture_y) = walk.pixel();
        walk.advance();
        if wraps {
            // Sizes are powers of two, so masking wraps negative coordinates too.
            texture_x &= size_pixels - 1;
            texture_y &= size_pixels - 1;
        } else if !(0..size_pixels).contains(&texture_x) || !(0..size_pixels).contains(&texture_y) {
            *pixel = LayerPixel { priority, is_transparent: true, ..LayerPixel::default() };
            continue;
        }
        let (texture_x, texture_y) = (texture_x as usize, texture_y as usize);
        let map_offset = screen_base + (texture_y / 8) * tiles_per_row + texture_x / 8;
        let tile_number = ppu.vram_byte(map_offset) as usize;
        let dot_offset = character_base + tile_number * 64 + (texture_y % 8) * 8 + texture_x % 8;
        let palette_index = ppu.vram_byte(dot_offset) as usize;
        *pixel = LayerPixel {
            color: ppu.palette_color(palette_index),
            priority,
            is_transparent: palette_index == 0,
            is_semi_transparent_sprite: false,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::Bus;

    const RED: u16 = 0x001F;

    /// BG2 in mode 1: 128x128 map at screen block 8, tile 1 has dot (0,0) = palette index 1.
    fn setup(control_extra: u16) -> Bus {
        let mut bus = Bus::new(&[]);
        bus.write16(0x0500_0002, RED);
        bus.write16(0x0600_0000 + 64, 1); // tile 1, dot (0,0)
        bus.write16(0x0600_4000, 1); // map entry (0,0) = tile 1
        bus.write16(0x0400_000C, (8 << 8) | control_extra); // BG2CNT
        bus.write16(0x0400_0020, 0x100); // PA = 1.0
        bus.write16(0x0400_0026, 0x100); // PD = 1.0
        bus
    }

    fn render(bus: &Bus) -> [LayerPixel; SCREEN_WIDTH] {
        let mut output = [LayerPixel::default(); SCREEN_WIDTH];
        render_affine_background_line(&bus.ppu, 2, 0, &mut output);
        output
    }

    #[test]
    fn identity_draws_tile_and_outside_is_transparent_without_wrap() {
        let bus = setup(0);
        let output = render(&bus);
        assert_eq!((output[0].color, output[0].is_transparent), (RED, false));
        assert!(output[1].is_transparent, "palette index 0 is transparent");
        assert!(output[128].is_transparent, "x=128 is past a 128-pixel map");
    }

    #[test]
    fn wrap_bit_repeats_the_map_and_reference_point_scrolls() {
        let mut bus = setup(BGCNT_AREA_OVERFLOW_WRAP);
        let output = render(&bus);
        assert_eq!(output[128].color, RED);
        assert!(!output[128].is_transparent);
        bus.write32(0x0400_0028, (-5i32 << 8) as u32); // BG2X = -5.0
        let output = render(&bus);
        assert_eq!(output[5].color, RED);
        assert!(output[0].is_transparent);
    }

    #[test]
    fn half_scale_stretches_each_dot_over_two_pixels() {
        let mut bus = setup(0);
        bus.write16(0x0400_0020, 0x80); // PA = 0.5
        let output = render(&bus);
        assert!(!output[0].is_transparent && !output[1].is_transparent);
        assert!(output[2].is_transparent);
    }
}
