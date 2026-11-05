//! Tiled (text) backgrounds for modes 0-1 (gbatek: LCD I/O BG Control, LCD I/O BG Scrolling,
//! LCD VRAM Character Data, LCD VRAM BG Screen Data Format).

use super::{LayerPixel, Ppu, SCREEN_WIDTH};

const CHARACTER_BLOCK_SIZE: usize = 0x4000; // 16 KiB per character base block
const SCREEN_BLOCK_SIZE: usize = 0x800; // 2 KiB per 32x32-entry screen block
const TILES_PER_BLOCK_SIDE: usize = 32;

const MAP_TILE_NUMBER_MASK: u16 = 0x3FF;
const MAP_HORIZONTAL_FLIP: u16 = 1 << 10;
const MAP_VERTICAL_FLIP: u16 = 1 << 11;

/// Fields of BGxCNT that matter for a text background.
struct TextBackgroundControl {
    priority: u8,
    character_base: usize,
    uses_256_colors: bool,
    screen_base: usize,
    width_pixels: usize,
    height_pixels: usize,
}

/// Decode BGxCNT (gbatek: LCD I/O BG Control; text sizes 256x256, 512x256, 256x512, 512x512).
fn decode_control(control: u16) -> TextBackgroundControl {
    let (width_pixels, height_pixels) = match control >> 14 {
        0 => (256, 256),
        1 => (512, 256),
        2 => (256, 512),
        _ => (512, 512),
    };
    TextBackgroundControl {
        priority: (control & 3) as u8,
        character_base: ((control >> 2) & 3) as usize * CHARACTER_BLOCK_SIZE,
        uses_256_colors: control & (1 << 7) != 0,
        screen_base: ((control >> 8) & 0x1F) as usize * SCREEN_BLOCK_SIZE,
        width_pixels,
        height_pixels,
    }
}

/// VRAM offset of the map entry covering background pixel (`background_x`, `background_y`).
/// Screen blocks SC0..SC3 are laid out left to right, then top to bottom.
fn map_entry_offset(control: &TextBackgroundControl, background_x: usize, background_y: usize) -> usize {
    let tile_x = background_x / 8;
    let tile_y = background_y / 8;
    let blocks_per_row = control.width_pixels / 256;
    let block_index = tile_x / TILES_PER_BLOCK_SIDE + (tile_y / TILES_PER_BLOCK_SIDE) * blocks_per_row;
    let entry_in_block = (tile_y % TILES_PER_BLOCK_SIDE) * TILES_PER_BLOCK_SIDE + tile_x % TILES_PER_BLOCK_SIDE;
    control.screen_base + block_index * SCREEN_BLOCK_SIZE + entry_in_block * 2
}

/// Palette index of one tile dot, 0 meaning transparent (gbatek: LCD VRAM Character Data).
fn tile_dot_palette_index(ppu: &Ppu, control: &TextBackgroundControl, map_entry: u16, dot_x: usize, dot_y: usize) -> usize {
    let tile_number = (map_entry & MAP_TILE_NUMBER_MASK) as usize;
    let dot_x = if map_entry & MAP_HORIZONTAL_FLIP != 0 { 7 - dot_x } else { dot_x };
    let dot_y = if map_entry & MAP_VERTICAL_FLIP != 0 { 7 - dot_y } else { dot_y };
    if control.uses_256_colors {
        let offset = control.character_base + tile_number * 64 + dot_y * 8 + dot_x;
        ppu.vram_byte(offset) as usize
    } else {
        let offset = control.character_base + tile_number * 32 + dot_y * 4 + dot_x / 2;
        let byte = ppu.vram_byte(offset);
        let color_index = if dot_x % 2 == 0 { byte & 0xF } else { byte >> 4 } as usize;
        if color_index == 0 {
            return 0;
        }
        let palette_bank = (map_entry >> 12) as usize;
        palette_bank * 16 + color_index
    }
}

/// Render one scanline of a text background into `output`.
// ponytail: per-pixel map lookup; fetch once per tile if the fuel limit gets tight.
pub fn render_text_background_line(
    ppu: &Ppu,
    background_index: usize,
    line: usize,
    output: &mut [LayerPixel; SCREEN_WIDTH],
) {
    let control = decode_control(ppu.background_control(background_index));
    let (horizontal_offset, vertical_offset) = ppu.background_scroll(background_index);
    let background_y = (line + vertical_offset as usize) % control.height_pixels;
    for (screen_x, pixel) in output.iter_mut().enumerate() {
        let background_x = (screen_x + horizontal_offset as usize) % control.width_pixels;
        let map_entry = ppu.vram_halfword(map_entry_offset(&control, background_x, background_y));
        let palette_index = tile_dot_palette_index(ppu, &control, map_entry, background_x % 8, background_y % 8);
        *pixel = LayerPixel {
            color: ppu.palette_color(palette_index),
            priority: control.priority,
            is_transparent: palette_index == 0,
            is_semi_transparent_sprite: false,
        };
    }
}
