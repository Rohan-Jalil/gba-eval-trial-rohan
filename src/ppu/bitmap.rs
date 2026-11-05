//! Bitmap modes 3, 4 and 5, drawn as BG2 through the BG2 rotation/scaling transform
//! (gbatek: LCD VRAM Bitmap BG Modes, LCD I/O BG Rotation/Scaling).

use super::bg_affine::AffineWalk;
use super::{LayerPixel, Ppu, SCREEN_WIDTH};

/// Offset of frame 1 in modes 4 and 5.
const FRAME_ONE_OFFSET: usize = 0xA000;
const FULL_WIDTH: i32 = 240;
const FULL_HEIGHT: i32 = 160;
const MODE5_WIDTH: i32 = 160;
const MODE5_HEIGHT: i32 = 128;

/// Render one scanline of the bitmap background (BG2) into `output`. The area overflow bit is
/// ignored in bitmap modes: texture pixels outside the bitmap are always transparent.
// ponytail: no BG mosaic here; add it when the effects worker exposes a mosaic helper.
pub fn render_bitmap_line(ppu: &Ppu, _line: usize, output: &mut [LayerPixel; SCREEN_WIDTH]) {
    let bg_mode = ppu.dispcnt() & 7;
    let priority = (ppu.background_control(2) & 3) as u8;
    let frame_base = if bg_mode != 3 && ppu.displays_frame_one() { FRAME_ONE_OFFSET } else { 0 };
    let (width, height) = if bg_mode == 5 { (MODE5_WIDTH, MODE5_HEIGHT) } else { (FULL_WIDTH, FULL_HEIGHT) };

    let mut walk = AffineWalk::for_line(ppu, 0);
    for pixel in output.iter_mut() {
        let (texture_x, texture_y) = walk.pixel();
        walk.advance();
        if !(0..width).contains(&texture_x) || !(0..height).contains(&texture_y) {
            *pixel = LayerPixel { priority, is_transparent: true, ..LayerPixel::default() };
            continue;
        }
        let dot_index = (texture_y * width + texture_x) as usize;
        let (color, is_transparent) = if bg_mode == 4 {
            let palette_index = ppu.vram_byte(frame_base + dot_index) as usize;
            (ppu.palette_color(palette_index), palette_index == 0)
        } else {
            (ppu.vram_halfword(frame_base + dot_index * 2), false)
        };
        *pixel = LayerPixel { color, priority, is_transparent, is_semi_transparent_sprite: false };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::Bus;

    const RED: u16 = 0x001F;

    fn render(bus: &Bus) -> [LayerPixel; SCREEN_WIDTH] {
        let mut output = [LayerPixel::default(); SCREEN_WIDTH];
        render_bitmap_line(&bus.ppu, 0, &mut output);
        output
    }

    #[test]
    fn mode3_follows_reference_point_and_scale() {
        let mut bus = Bus::new(&[]);
        bus.write16(0x0400_0000, 0x0403); // mode 3, BG2 on
        bus.write16(0x0400_0020, 0x100); // PA = 1.0
        bus.write16(0x0600_0000 + (10 * 240 + 6) * 2, RED);
        bus.write32(0x0400_002C, 10 << 8); // BG2Y = 10: this line shows bitmap row 10
        bus.write32(0x0400_0028, 1 << 8); // BG2X = 1
        let output = render(&bus);
        assert_eq!(output[5].color, RED);
        bus.write16(0x0400_0020, 0x200); // PA = 2.0: pixel x samples column 1 + 2x
        assert_eq!(render(&bus)[5].color, 0, "x=5 now samples column 11");
        assert_ne!(render(&bus)[5].color, RED);
        assert!(render(&bus)[120].is_transparent, "column 241 is outside the bitmap");
    }

    #[test]
    fn mode5_is_transparent_outside_160_by_128_even_with_wrap_bit() {
        let mut bus = Bus::new(&[]);
        bus.write16(0x0400_0000, 0x0415); // mode 5, frame 1, BG2 on
        bus.write16(0x0400_000C, 1 << 13); // BG2CNT wrap bit is ignored in bitmap modes
        bus.write16(0x0400_0020, 0x100);
        bus.write16(0x0600_A000 + 159 * 2, RED);
        let output = render(&bus);
        assert_eq!((output[159].color, output[159].is_transparent), (RED, false));
        assert!(output[160].is_transparent);
    }
}
