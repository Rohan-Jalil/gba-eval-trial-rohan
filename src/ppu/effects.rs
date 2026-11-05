//! Line compositor: windows, BG mosaic, priority, alpha blending and brightness
//! (gbatek: LCD I/O Window Feature, LCD I/O Mosaic Function, LCD I/O Color Special Effects,
//! LCD I/O BG Control).

use super::{bgr555_to_abgr8888, LineLayers, Ppu, SCREEN_HEIGHT, SCREEN_WIDTH};

const WIN0H: u32 = 0x040; // window 0 horizontal: X1 (high byte), X2 (low byte, exclusive)
const WIN0V: u32 = 0x044; // window 0 vertical: Y1 (high byte), Y2 (low byte, exclusive)
const WININ: u32 = 0x048; // layer/effect enables inside window 0 (low byte) and 1 (high byte)
const WINOUT: u32 = 0x04A; // enables outside all windows (low byte) and inside the OBJ window (high)
const MOSAIC: u32 = 0x04C; // mosaic sizes minus 1: BG H (bits 0-3), BG V (bits 4-7)
const BLDCNT: u32 = 0x050; // special effect selection: 1st targets, mode, 2nd targets
const BLDALPHA: u32 = 0x052; // alpha coefficients EVA (bits 0-4), EVB (bits 8-12)
const BLDY: u32 = 0x054; // brightness coefficient EVY (bits 0-4)

const DISPCNT_WINDOW0_ENABLE: u16 = 1 << 13;
const DISPCNT_WINDOW1_ENABLE: u16 = 1 << 14;
const DISPCNT_OBJ_WINDOW_ENABLE: u16 = 1 << 15;
const BGCNT_MOSAIC: u16 = 1 << 6;

/// Layer ids, also the bit position in window controls and BLDCNT target masks.
const OBJ_LAYER: usize = 4;
const BACKDROP_LAYER: usize = 5;
/// Window control bit 5: colour special effects allowed.
const EFFECTS_ENABLE: u8 = 1 << 5;
/// Window control when no window is on: every layer and effects enabled.
const EVERYTHING_ENABLED: u8 = 0x3F;

/// One window's rectangle for the current line (gbatek: LCD I/O Window Feature).
struct WindowSpan {
    is_on_this_line: bool,
    left: usize,
    right: usize,
}

impl WindowSpan {
    /// Decode WINxH/WINxV; garbage X2>240 or X1>X2 means X2=240, same for Y with 160.
    fn new(horizontal: u16, vertical: u16, line: usize) -> Self {
        let (left, right) = clamp_window_range(horizontal, SCREEN_WIDTH);
        let (top, bottom) = clamp_window_range(vertical, SCREEN_HEIGHT);
        WindowSpan { is_on_this_line: (top..bottom).contains(&line), left, right }
    }

    fn contains(&self, x: usize) -> bool {
        self.is_on_this_line && x >= self.left && x < self.right
    }
}

/// (start, end) from a window register: start in the high byte, exclusive end in the low byte.
fn clamp_window_range(register: u16, screen_size: usize) -> (usize, usize) {
    let start = (register >> 8) as usize;
    let end = (register & 0xFF) as usize;
    let end = if end > screen_size || start > end { screen_size } else { end };
    (start, end)
}

/// Colour special effect mode, BLDCNT bits 6-7.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EffectMode {
    None,
    AlphaBlend,
    BrightnessIncrease,
    BrightnessDecrease,
}

/// BLDCNT/BLDALPHA/BLDY decoded once per line (gbatek: LCD I/O Color Special Effects).
struct BlendSettings {
    mode: EffectMode,
    first_targets: u8,
    second_targets: u8,
    eva: u32,
    evb: u32,
    evy: u32,
}

impl BlendSettings {
    fn new(bldcnt: u16, bldalpha: u16, bldy: u16) -> Self {
        // Coefficients 17-31 act as 16/16.
        let coefficient = |raw: u16| ((raw & 0x1F) as u32).min(16);
        let mode = match (bldcnt >> 6) & 3 {
            0 => EffectMode::None,
            1 => EffectMode::AlphaBlend,
            2 => EffectMode::BrightnessIncrease,
            _ => EffectMode::BrightnessDecrease,
        };
        BlendSettings {
            mode,
            first_targets: (bldcnt & 0x3F) as u8,
            second_targets: ((bldcnt >> 8) & 0x3F) as u8,
            eva: coefficient(bldalpha),
            evb: coefficient(bldalpha >> 8),
            evy: coefficient(bldy),
        }
    }
}

/// Split BGR555 into (red, green, blue) 5-bit intensities.
fn channels(color: u16) -> [u32; 3] {
    [(color & 0x1F) as u32, ((color >> 5) & 0x1F) as u32, ((color >> 10) & 0x1F) as u32]
}

fn pack_channels([red, green, blue]: [u32; 3]) -> u16 {
    (red | green << 5 | blue << 10) as u16
}

/// I = MIN(31, I1st*EVA + I2nd*EVB), coefficients in 1/16 units.
fn alpha_blend(first: u16, second: u16, eva: u32, evb: u32) -> u16 {
    let (first, second) = (channels(first), channels(second));
    pack_channels([0, 1, 2].map(|channel| ((first[channel] * eva + second[channel] * evb) >> 4).min(31)))
}

/// I = I + (31-I)*EVY.
fn brighten(color: u16, evy: u32) -> u16 {
    pack_channels(channels(color).map(|intensity| intensity + (((31 - intensity) * evy) >> 4)))
}

/// I = I - I*EVY.
fn darken(color: u16, evy: u32) -> u16 {
    pack_channels(channels(color).map(|intensity| intensity - ((intensity * evy) >> 4)))
}

/// A candidate for the top two visible pixels at one x.
#[derive(Clone, Copy)]
struct VisiblePixel {
    layer: usize,
    color: u16,
    /// priority * 8 + order (OBJ 0, BG0-3 1-4); smaller is in front.
    depth: u8,
    is_semi_transparent_sprite: bool,
}

impl VisiblePixel {
    fn backdrop(color: u16) -> Self {
        VisiblePixel { layer: BACKDROP_LAYER, color, depth: u8::MAX, is_semi_transparent_sprite: false }
    }
}

/// Keeps the front-most and second front-most pixel seen so far.
fn insert_pixel(top: &mut VisiblePixel, second: &mut VisiblePixel, candidate: VisiblePixel) {
    if candidate.depth < top.depth {
        *second = *top;
        *top = candidate;
    } else if candidate.depth < second.depth {
        *second = candidate;
    }
}

/// Final colour of one pixel from its top two layers (gbatek: LCD I/O Color Special Effects,
/// including "Semi-Transparent OBJs" which blend whenever the pixel below is a 2nd target).
fn apply_effects(top: VisiblePixel, second: VisiblePixel, blend: &BlendSettings) -> u16 {
    let second_is_target = blend.second_targets & (1 << second.layer) != 0;
    if top.is_semi_transparent_sprite && second_is_target {
        return alpha_blend(top.color, second.color, blend.eva, blend.evb);
    }
    if blend.first_targets & (1 << top.layer) == 0 {
        return top.color;
    }
    match blend.mode {
        EffectMode::AlphaBlend if second_is_target => alpha_blend(top.color, second.color, blend.eva, blend.evb),
        EffectMode::BrightnessIncrease => brighten(top.color, blend.evy),
        EffectMode::BrightnessDecrease => darken(top.color, blend.evy),
        _ => top.color,
    }
}

impl Ppu {
    /// Merge the line's layers over the backdrop into the framebuffer: windows pick enabled
    /// layers, BG mosaic resamples, priority picks the top two pixels, then colour effects.
    /// Priority: lower value wins, OBJ beats a BG of equal priority, BG0 beats BG3 on ties
    /// (gbatek: LCD I/O BG Control; LCD OBJ - OAM Attributes).
    pub(super) fn compose_line(&mut self, line: usize, layers: &LineLayers) {
        let dispcnt = self.dispcnt();
        let backdrop = VisiblePixel::backdrop(self.palette_color(0));
        let blend = BlendSettings::new(self.register(BLDCNT), self.register(BLDALPHA), self.register(BLDY));
        let window0 = WindowSpan::new(self.register(WIN0H), self.register(WIN0V), line);
        let window1 = WindowSpan::new(self.register(WIN0H + 2), self.register(WIN0V + 2), line);
        let (winin, winout) = (self.register(WININ), self.register(WINOUT));
        let any_window = dispcnt & (DISPCNT_WINDOW0_ENABLE | DISPCNT_WINDOW1_ENABLE | DISPCNT_OBJ_WINDOW_ENABLE) != 0;

        let window_control_at = |x: usize| -> u8 {
            if !any_window {
                EVERYTHING_ENABLED
            } else if dispcnt & DISPCNT_WINDOW0_ENABLE != 0 && window0.contains(x) {
                winin as u8
            } else if dispcnt & DISPCNT_WINDOW1_ENABLE != 0 && window1.contains(x) {
                (winin >> 8) as u8
            } else if dispcnt & DISPCNT_OBJ_WINDOW_ENABLE != 0 && layers.obj_window[x] {
                (winout >> 8) as u8
            } else {
                winout as u8
            }
        };

        let mosaic = self.register(MOSAIC);
        let mosaic_width = (mosaic & 0xF) as usize + 1;
        let mosaic_height = ((mosaic >> 4) & 0xF) as usize + 1;
        let mut uses_mosaic = [false; 4];
        for (background_index, uses) in uses_mosaic.iter_mut().enumerate() {
            *uses = layers.background_enabled[background_index]
                && self.background_control(background_index) & BGCNT_MOSAIC != 0;
            // Vertical mosaic: the first line of each block is kept and repeated below it.
            // ponytail: the block's first line is reused as rendered, so scroll changes inside a
            // block are ignored; sample the block's first line in the renderers if that matters.
            if *uses && line % mosaic_height == 0 {
                self.mosaic_source_lines[background_index] = layers.backgrounds[background_index];
            }
        }

        let start = line * SCREEN_WIDTH;
        for x in 0..SCREEN_WIDTH {
            let control = window_control_at(x);
            let (mut top, mut second) = (backdrop, backdrop);
            let sprite = layers.sprites[x];
            if layers.sprites_enabled && control & (1 << OBJ_LAYER) != 0 && !sprite.is_transparent {
                let candidate = VisiblePixel {
                    layer: OBJ_LAYER,
                    color: sprite.color,
                    depth: sprite.priority * 8,
                    is_semi_transparent_sprite: sprite.is_semi_transparent_sprite,
                };
                insert_pixel(&mut top, &mut second, candidate);
            }
            for background_index in 0..4 {
                if !layers.background_enabled[background_index] || control & (1 << background_index) == 0 {
                    continue;
                }
                let pixel = if uses_mosaic[background_index] {
                    self.mosaic_source_lines[background_index][x - x % mosaic_width]
                } else {
                    layers.backgrounds[background_index][x]
                };
                if pixel.is_transparent {
                    continue;
                }
                let candidate = VisiblePixel {
                    layer: background_index,
                    color: pixel.color,
                    depth: pixel.priority * 8 + 1 + background_index as u8,
                    is_semi_transparent_sprite: false,
                };
                insert_pixel(&mut top, &mut second, candidate);
            }
            let color = if control & EFFECTS_ENABLE != 0 { apply_effects(top, second, &blend) } else { top.color };
            self.framebuffer[start + x] = bgr555_to_abgr8888(color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ppu::LayerPixel;
    use crate::bus::Bus;

    const RED: u16 = 0x001F;
    const BLUE: u16 = 0x7C00;

    #[test]
    fn blend_math_matches_gbatek_formulas() {
        assert_eq!(alpha_blend(RED, BLUE, 8, 8), 15 | 15 << 10);
        assert_eq!(alpha_blend(0x7FFF, 0x7FFF, 16, 16), 0x7FFF, "clamped to 31");
        assert_eq!(brighten(0, 16), 0x7FFF);
        assert_eq!(brighten(0, 8), 15 | 15 << 5 | 15 << 10);
        assert_eq!(darken(0x7FFF, 16), 0);
        assert_eq!(BlendSettings::new(0, 0x1F1F, 31).eva, 16);
    }

    #[test]
    fn garbage_window_end_reads_as_screen_edge() {
        assert_eq!(clamp_window_range(0x10_20, 240), (0x10, 0x20));
        assert_eq!(clamp_window_range(0x30_20, 240), (0x30, 240));
        assert_eq!(clamp_window_range(0x00_F5, 240), (0, 240));
    }

    fn opaque(color: u16, priority: u8) -> LayerPixel {
        LayerPixel { color, priority, is_transparent: false, is_semi_transparent_sprite: false }
    }

    fn compose(bus: &mut Bus, layers: &LineLayers) -> u32 {
        bus.ppu.compose_line(0, layers);
        bus.ppu.framebuffer[0]
    }

    #[test]
    fn priority_windows_and_semi_transparent_sprites() {
        let mut bus = Bus::new(&[]);
        let mut layers = LineLayers::new();
        layers.background_enabled = [true, false, false, true];
        layers.backgrounds[0][0] = opaque(RED, 1);
        layers.backgrounds[3][0] = opaque(BLUE, 1);
        assert_eq!(compose(&mut bus, &layers), bgr555_to_abgr8888(RED), "BG0 wins tie");

        layers.sprites_enabled = true;
        layers.sprites[0] = opaque(0x03E0, 1);
        assert_eq!(compose(&mut bus, &layers), bgr555_to_abgr8888(0x03E0), "OBJ wins tie");

        // Semi-transparent sprite blends with BG0 below it even with BLDCNT mode 0.
        layers.sprites[0].is_semi_transparent_sprite = true;
        bus.write16(0x0400_0050, 1 << 8); // BG0 2nd target
        bus.write16(0x0400_0052, 0x0808); // EVA = EVB = 8/16
        assert_eq!(compose(&mut bus, &layers), bgr555_to_abgr8888(15 | 15 << 5));

        // Window 0 covering x=0 shows only BG3 there.
        bus.write16(0x0400_0040, 0x0001);
        bus.write16(0x0400_0044, 0x0001);
        bus.write16(0x0400_0048, 1 << 3);
        bus.write16(0x0400_0000, 1 << 13);
        assert_eq!(compose(&mut bus, &layers), bgr555_to_abgr8888(BLUE));
    }

    #[test]
    fn horizontal_mosaic_repeats_block_origin() {
        let mut bus = Bus::new(&[]);
        let mut layers = LineLayers::new();
        layers.background_enabled[0] = true;
        layers.backgrounds[0][0] = opaque(RED, 0);
        bus.write16(0x0400_0008, BGCNT_MOSAIC);
        bus.write16(0x0400_004C, 0x0003); // 4 pixels wide
        bus.ppu.compose_line(0, &layers);
        assert_eq!(bus.ppu.framebuffer[3], bgr555_to_abgr8888(RED));
        assert_eq!(bus.ppu.framebuffer[4], bgr555_to_abgr8888(0));
    }
}
