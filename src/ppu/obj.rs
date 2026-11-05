//! Sprites, normal and affine (gbatek: LCD OBJ - Overview, LCD OBJ - OAM Attributes,
//! LCD OBJ - OAM Rotation/Scaling Parameters, LCD OBJ - VRAM Character (Tile) Mapping,
//! LCD I/O Mosaic Function).

use super::{LayerPixel, Ppu, SCREEN_WIDTH};

const OBJ_COUNT: usize = 128;
const MOSAIC: u32 = 0x04C; // mosaic size: OBJ H-size bits 8-11, OBJ V-size bits 12-15 (minus 1)

const DISPCNT_HBLANK_INTERVAL_FREE: u16 = 1 << 5; // OAM accessible in HBlank, fewer OBJ cycles
const DISPCNT_OBJ_ONE_DIMENSIONAL: u16 = 1 << 6; // OBJ character VRAM mapping

/// OBJ tiles start at 0x10000 in VRAM; 1024 tiles of 32 bytes.
const OBJ_TILE_BASE: usize = 0x10000;
const OBJ_TILE_BYTES: usize = 32;
/// In bitmap modes 3-5 tile numbers below 512 overlap the frame buffer and are not shown.
const FIRST_BITMAP_MODE_TILE: usize = 512;
/// OBJ palette starts at palette entry 256.
const OBJ_PALETTE_START: usize = 256;

/// OBJ rendering cycles available per line (gbatek: LCD OBJ - Overview).
const LINE_CYCLES_DEFAULT: i32 = 1210;
const LINE_CYCLES_HBLANK_FREE: i32 = 954;

/// OBJ Mode (attribute 0 bits 10-11).
const OBJ_MODE_SEMI_TRANSPARENT: u16 = 1;
const OBJ_MODE_WINDOW: u16 = 2;
const OBJ_MODE_PROHIBITED: u16 = 3;

/// One OAM entry, decoded (gbatek: LCD OBJ - OAM Attributes).
struct Sprite {
    /// Top-left of the bounding box on screen; Y already moved above the screen when needed.
    top: i32,
    left: i32,
    /// Sprite size in dots.
    width: i32,
    height: i32,
    /// Bounding box size: doubled for double-size affine sprites.
    box_width: i32,
    box_height: i32,
    /// Affine parameter group 0-31, or None for a normal sprite.
    affine_group: Option<usize>,
    horizontal_flip: bool,
    vertical_flip: bool,
    mode: u16,
    mosaic: bool,
    uses_256_colors: bool,
    tile_number: usize,
    priority: u8,
    palette_bank: usize,
}

/// OBJ size in dots for shape and size fields; shape 3 is prohibited (gbatek: Attribute 1).
fn sprite_dimensions(shape: u16, size: u16) -> Option<(i32, i32)> {
    const SQUARE: [(i32, i32); 4] = [(8, 8), (16, 16), (32, 32), (64, 64)];
    const HORIZONTAL: [(i32, i32); 4] = [(16, 8), (32, 8), (32, 16), (64, 32)];
    const VERTICAL: [(i32, i32); 4] = [(8, 16), (8, 32), (16, 32), (32, 64)];
    let table = match shape {
        0 => &SQUARE,
        1 => &HORIZONTAL,
        2 => &VERTICAL,
        _ => return None,
    };
    Some(table[size as usize])
}

/// OAM halfword at byte offset `offset`.
fn oam_halfword(ppu: &Ppu, offset: usize) -> u16 {
    u16::from_le_bytes([ppu.oam[offset], ppu.oam[offset + 1]])
}

/// Decode OBJ `index`; None if it is disabled or has a prohibited shape/mode.
fn decode_sprite(ppu: &Ppu, index: usize) -> Option<Sprite> {
    let attribute0 = oam_halfword(ppu, index * 8);
    let attribute1 = oam_halfword(ppu, index * 8 + 2);
    let attribute2 = oam_halfword(ppu, index * 8 + 4);

    let is_affine = attribute0 & (1 << 8) != 0;
    let flag_bit9 = attribute0 & (1 << 9) != 0; // double size if affine, else disable
    if !is_affine && flag_bit9 {
        return None;
    }
    let mode = (attribute0 >> 10) & 3;
    if mode == OBJ_MODE_PROHIBITED {
        return None;
    }
    let (width, height) = sprite_dimensions(attribute0 >> 14, attribute1 >> 14)?;
    let double_size = is_affine && flag_bit9;
    let (box_width, box_height) = if double_size { (width * 2, height * 2) } else { (width, height) };

    // Y is 0-255; a box that would run past line 255 starts above the screen instead
    // (gbatek: "located at Y>128 will be treated as at Y>-128").
    let mut top = (attribute0 & 0xFF) as i32;
    if top + box_height > 256 {
        top -= 256;
    }
    // X is 9-bit signed.
    let left = (((attribute1 & 0x1FF) as i32) << 23) >> 23;

    Some(Sprite {
        top,
        left,
        width,
        height,
        box_width,
        box_height,
        affine_group: is_affine.then(|| ((attribute1 >> 9) & 0x1F) as usize),
        horizontal_flip: !is_affine && attribute1 & (1 << 12) != 0,
        vertical_flip: !is_affine && attribute1 & (1 << 13) != 0,
        mode,
        mosaic: attribute0 & (1 << 12) != 0,
        uses_256_colors: attribute0 & (1 << 13) != 0,
        tile_number: (attribute2 & 0x3FF) as usize,
        priority: ((attribute2 >> 10) & 3) as u8,
        palette_bank: (attribute2 >> 12) as usize,
    })
}

/// PA, PB, PC, PD of OBJ affine group `group`, signed 8.8 (gbatek: OAM Rotation/Scaling Parameters).
fn affine_parameters(ppu: &Ppu, group: usize) -> [i32; 4] {
    [0, 1, 2, 3].map(|parameter| oam_halfword(ppu, group * 32 + parameter * 8 + 6) as i16 as i32)
}

/// Rendering cycles a sprite on this line costs (gbatek: LCD OBJ - Overview).
fn sprite_cycle_cost(sprite: &Sprite) -> i32 {
    if sprite.affine_group.is_some() {
        10 + sprite.box_width * 2
    } else {
        sprite.box_width
    }
}

/// Palette index (0 = transparent) of sprite dot (`dot_x`, `dot_y`), both inside the sprite
/// (gbatek: LCD OBJ - VRAM Character (Tile) Mapping).
fn sprite_dot_palette_index(ppu: &Ppu, sprite: &Sprite, one_dimensional: bool, dot_x: usize, dot_y: usize) -> usize {
    let tile_units_per_tile = if sprite.uses_256_colors { 2 } else { 1 };
    let tile_column = dot_x / 8;
    let tile_row = dot_y / 8;
    let row_stride = if one_dimensional { sprite.width as usize / 8 * tile_units_per_tile } else { 32 };
    let base_tile = if sprite.uses_256_colors && !one_dimensional {
        sprite.tile_number & !1 // the low bit is ignored in 2D 256-colour mode
    } else {
        sprite.tile_number
    };
    let tile = (base_tile + tile_row * row_stride + tile_column * tile_units_per_tile) & 0x3FF;
    let tile_offset = OBJ_TILE_BASE + tile * OBJ_TILE_BYTES;
    let (inner_x, inner_y) = (dot_x % 8, dot_y % 8);
    if sprite.uses_256_colors {
        ppu.vram_byte(tile_offset + inner_y * 8 + inner_x) as usize
    } else {
        let byte = ppu.vram_byte(tile_offset + inner_y * 4 + inner_x / 2);
        let nibble = if inner_x & 1 == 0 { byte & 0xF } else { byte >> 4 };
        if nibble == 0 { 0 } else { sprite.palette_bank * 16 + nibble as usize }
    }
}

/// Render one scanline of all sprites into `output` (front-most sprite per pixel, its OBJ
/// priority in `priority`) and mark OBJ-window pixels in `obj_window`.
/// Lower priority value wins; on a tie the lower OAM index wins.
pub fn render_sprite_line(
    ppu: &Ppu,
    line: usize,
    output: &mut [LayerPixel; SCREEN_WIDTH],
    obj_window: &mut [bool; SCREEN_WIDTH],
) {
    output.fill(LayerPixel { is_transparent: true, priority: 3, ..LayerPixel::default() });
    obj_window.fill(false);

    let dispcnt = ppu.dispcnt();
    let one_dimensional = dispcnt & DISPCNT_OBJ_ONE_DIMENSIONAL != 0;
    let is_bitmap_mode = (dispcnt & 7) >= 3;
    let mosaic = ppu.register(MOSAIC);
    let mosaic_width = ((mosaic >> 8) & 0xF) as i32 + 1;
    let mosaic_height = ((mosaic >> 12) & 0xF) as i32 + 1;
    let mut cycles_left =
        if dispcnt & DISPCNT_HBLANK_INTERVAL_FREE != 0 { LINE_CYCLES_HBLANK_FREE } else { LINE_CYCLES_DEFAULT };
    let line = line as i32;

    for index in 0..OBJ_COUNT {
        if cycles_left <= 0 {
            break;
        }
        let Some(sprite) = decode_sprite(ppu, index) else { continue };
        let box_row = line - sprite.top;
        if box_row < 0 || box_row >= sprite.box_height {
            continue;
        }
        // ponytail: budget charged per sprite on the line, left to right, one dot per cycle
        // (two for affine after 10 setup cycles); gbatek gives no finer detail.
        let cost = sprite_cycle_cost(&sprite);
        let drawable_columns = if sprite.affine_group.is_some() {
            ((cycles_left - 10) / 2).clamp(0, sprite.box_width)
        } else {
            cycles_left.min(sprite.box_width)
        };
        cycles_left -= cost;
        if is_bitmap_mode && sprite.tile_number < FIRST_BITMAP_MODE_TILE {
            continue;
        }
        draw_sprite_row(
            ppu,
            &sprite,
            one_dimensional,
            line,
            drawable_columns,
            (mosaic_width, mosaic_height),
            output,
            obj_window,
        );
    }
}

/// Draw the part of `sprite` that lies on screen line `line` (only its first
/// `drawable_columns` box columns, per the cycle budget).
#[allow(clippy::too_many_arguments)]
fn draw_sprite_row(
    ppu: &Ppu,
    sprite: &Sprite,
    one_dimensional: bool,
    line: i32,
    drawable_columns: i32,
    (mosaic_width, mosaic_height): (i32, i32),
    output: &mut [LayerPixel; SCREEN_WIDTH],
    obj_window: &mut [bool; SCREEN_WIDTH],
) {
    // Mosaic snaps screen coordinates to a grid; the sampled point may fall outside the box.
    // ponytail: grid is screen-relative (as for BGs); gbatek does not say for OBJs.
    let sample_line = if sprite.mosaic { line - line % mosaic_height } else { line };
    let box_row = sample_line - sprite.top;
    if box_row < 0 {
        return;
    }
    let affine = sprite.affine_group.map(|group| affine_parameters(ppu, group));
    let first_column = (-sprite.left).max(0);
    let last_column = drawable_columns.min(SCREEN_WIDTH as i32 - sprite.left);

    for box_column in first_column..last_column {
        let screen_x = sprite.left + box_column;
        let sample_x = if sprite.mosaic { screen_x - screen_x % mosaic_width } else { screen_x };
        let sample_column = sample_x - sprite.left;
        if sample_column < 0 {
            continue;
        }
        let (dot_x, dot_y) = match affine {
            Some([parameter_a, parameter_b, parameter_c, parameter_d]) => {
                // Rotate around the box centre, then map into the sprite (gbatek: same as BG).
                let offset_x = sample_column - sprite.box_width / 2;
                let offset_y = box_row - sprite.box_height / 2;
                let dot_x = ((parameter_a * offset_x + parameter_b * offset_y) >> 8) + sprite.width / 2;
                let dot_y = ((parameter_c * offset_x + parameter_d * offset_y) >> 8) + sprite.height / 2;
                if dot_x < 0 || dot_x >= sprite.width || dot_y < 0 || dot_y >= sprite.height {
                    continue;
                }
                (dot_x, dot_y)
            }
            None => {
                let dot_x = if sprite.horizontal_flip { sprite.width - 1 - sample_column } else { sample_column };
                let dot_y = if sprite.vertical_flip { sprite.height - 1 - box_row } else { box_row };
                (dot_x, dot_y)
            }
        };
        let palette_index = sprite_dot_palette_index(ppu, sprite, one_dimensional, dot_x as usize, dot_y as usize);
        if palette_index == 0 {
            continue;
        }
        let x = screen_x as usize;
        if sprite.mode == OBJ_MODE_WINDOW {
            obj_window[x] = true;
            continue;
        }
        let current = &mut output[x];
        if current.is_transparent || sprite.priority < current.priority {
            *current = LayerPixel {
                color: ppu.palette_color(OBJ_PALETTE_START + palette_index),
                priority: sprite.priority,
                is_transparent: false,
                is_semi_transparent_sprite: sprite.mode == OBJ_MODE_SEMI_TRANSPARENT,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::Bus;

    const RED: u16 = 0x001F;
    const GREEN: u16 = 0x03E0;

    /// Write OBJ `index` attributes 0-2.
    fn set_sprite(bus: &mut Bus, index: u32, attributes: [u16; 3]) {
        for (slot, value) in attributes.iter().enumerate() {
            bus.write16(0x0700_0000 + index * 8 + slot as u32 * 2, *value);
        }
    }

    fn render(bus: &Bus, line: usize) -> ([LayerPixel; SCREEN_WIDTH], [bool; SCREEN_WIDTH]) {
        let mut output = [LayerPixel::default(); SCREEN_WIDTH];
        let mut window = [false; SCREEN_WIDTH];
        render_sprite_line(&bus.ppu, line, &mut output, &mut window);
        (output, window)
    }

    /// Bus with every OAM entry disabled, OBJ palette 1 = red, 2 = green, and 4bpp tile 1
    /// filled with colour 1 and tile 2 filled with colour 2.
    fn setup() -> Bus {
        let mut bus = Bus::new(&[]);
        for index in 0..128 {
            set_sprite(&mut bus, index, [1 << 9, 0, 0]);
        }
        bus.write16(0x0500_0200 + 2, RED);
        bus.write16(0x0500_0200 + 4, GREEN);
        for offset in (0..32).step_by(2) {
            bus.write16(0x0601_0000 + 32 + offset, 0x1111);
            bus.write16(0x0601_0000 + 64 + offset, 0x2222);
        }
        bus
    }

    #[test]
    fn normal_sprite_position_priority_and_wrap() {
        let mut bus = setup();
        set_sprite(&mut bus, 0, [10, 20, 1 | (2 << 10)]); // 8x8 at (20,10), tile 1, priority 2
        set_sprite(&mut bus, 1, [10, 24, 2 | (1 << 10)]); // overlapping, priority 1 wins
        let (output, _) = render(&bus, 10);
        assert!(output[19].is_transparent);
        assert_eq!((output[20].color, output[20].priority), (RED, 2));
        assert_eq!((output[24].color, output[24].priority), (GREEN, 1));
        assert!(render(&bus, 18).0[20].is_transparent);
        // X 0x1FC is -4, Y 252 is -4: dots 4-7 show at x 0-3 on lines 0-3.
        set_sprite(&mut bus, 0, [252, 0x1FC, 1]);
        let (output, _) = render(&bus, 3);
        assert_eq!(output[3].color, RED);
        assert!(output[4].is_transparent);
    }

    #[test]
    fn obj_window_and_semi_transparent_modes() {
        let mut bus = setup();
        set_sprite(&mut bus, 0, [(2 << 10), 0, 1]);
        set_sprite(&mut bus, 1, [(1 << 10), 8, 1]);
        let (output, window) = render(&bus, 0);
        assert!(window[0] && !window[8]);
        assert!(output[0].is_transparent);
        assert!(output[8].is_semi_transparent_sprite);
    }

    #[test]
    fn affine_identity_double_size_centres_sprite() {
        let mut bus = setup();
        set_sprite(&mut bus, 0, [(1 << 8) | (1 << 9), 0, 1]); // 8x8 double size: 16x16 box
        bus.write16(0x0700_0006, 0x100); // PA = 1.0
        bus.write16(0x0700_001E, 0x100); // PD = 1.0
        let (output, _) = render(&bus, 4);
        assert!(output[3].is_transparent);
        assert_eq!(output[4].color, RED);
        assert_eq!(output[11].color, RED);
        assert!(output[12].is_transparent);
        assert!(render(&bus, 3).0[4].is_transparent);
    }

    #[test]
    fn one_dimensional_mapping_and_flip() {
        let mut bus = setup();
        bus.write16(0x0400_0000, 1 << 6);
        set_sprite(&mut bus, 0, [1 << 14, 1 << 12, 1]); // 16x8, tiles 1,2, horizontal flip
        let (output, _) = render(&bus, 0);
        assert_eq!(output[0].color, GREEN);
        assert_eq!(output[8].color, RED);
    }

    #[test]
    fn cycle_budget_limits_sprites_per_line() {
        let mut bus = setup();
        // 64x64 sprites cost 64 cycles each: 1210 / 64 = 18 full sprites, the 19th partly.
        for index in 0..20 {
            set_sprite(&mut bus, index, [0, 3 << 14, 1]);
        }
        set_sprite(&mut bus, 19, [0, 100 | (3 << 14), 2 | (1 << 10)]);
        assert!(render(&bus, 0).0[100].is_transparent, "20th sprite is over budget");
        set_sprite(&mut bus, 1, [1 << 9, 0, 0]); // one fewer sprite frees the budget
        assert_eq!(render(&bus, 0).0[100].color, GREEN);
    }
}
