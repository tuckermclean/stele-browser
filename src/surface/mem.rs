//! An in-memory RGBA surface — the substrate for pixel-exact golden tests
//! (brief §7). Needs no display, so CI and the fixture suite never touch fbdev
//! or X. P7 blesses renders produced through this; P9 shares its pixel ops.

use super::{Color, Rect, Surface, TextRun};
use crate::text::Metrics;

/// A flat RGBA8 framebuffer in memory.
#[derive(Debug, Clone)]
pub struct MemSurface {
    width: u32,
    height: u32,
    /// `width * height * 4` bytes, row-major, RGBA.
    pixels: Vec<u8>,
}

impl MemSurface {
    /// A new surface filled with `bg`.
    pub fn new(width: u32, height: u32, bg: Color) -> Self {
        let mut s = MemSurface {
            width,
            height,
            pixels: vec![0; (width as usize) * (height as usize) * 4],
        };
        s.fill_rect(
            Rect { x: 0, y: 0, w: width, h: height },
            bg,
        );
        s
    }

    /// The raw RGBA8 bytes (for encoding a golden PNG).
    pub fn bytes(&self) -> &[u8] {
        &self.pixels
    }

    #[inline]
    fn in_bounds(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && (x as u32) < self.width && (y as u32) < self.height
    }

    #[inline]
    fn index(&self, x: i32, y: i32) -> usize {
        ((y as usize) * (self.width as usize) + (x as usize)) * 4
    }
}

impl Surface for MemSurface {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn put_pixel(&mut self, x: i32, y: i32, color: Color) {
        if !self.in_bounds(x, y) {
            return;
        }
        let i = self.index(x, y);
        // Straight-alpha source-over. Opaque is the common path; blend the rest.
        if color.a == 255 {
            self.pixels[i] = color.r;
            self.pixels[i + 1] = color.g;
            self.pixels[i + 2] = color.b;
            self.pixels[i + 3] = 255;
        } else if color.a != 0 {
            let sa = color.a as u32;
            let ia = 255 - sa;
            let blend = |s: u8, d: u8| ((s as u32 * sa + d as u32 * ia) / 255) as u8;
            self.pixels[i] = blend(color.r, self.pixels[i]);
            self.pixels[i + 1] = blend(color.g, self.pixels[i + 1]);
            self.pixels[i + 2] = blend(color.b, self.pixels[i + 2]);
            self.pixels[i + 3] = self.pixels[i + 3].max(color.a);
        }
    }

    fn fill_rect(&mut self, rect: Rect, color: Color) {
        let x0 = rect.x.max(0);
        let y0 = rect.y.max(0);
        let x1 = (rect.x + rect.w as i32).min(self.width as i32);
        let y1 = (rect.y + rect.h as i32).min(self.height as i32);
        for y in y0..y1 {
            for x in x0..x1 {
                self.put_pixel(x, y, color);
            }
        }
    }

    fn blit(&mut self, _at: Rect, _image: &crate::img::RgbaImage) {
        todo!("P9: image blit into the mem surface")
    }

    /// Rasterize `run` glyph-by-glyph via the embedded `text::glyphs` atlas
    /// (M4, pixel foundation Part 2). See module docs above `draw_glyph` for
    /// the placement/scaling rules pinned by the `draw_text_*` tests below.
    fn draw_text(&mut self, run: &TextRun) {
        if run.text.is_empty() || run.color.a == 0 {
            return;
        }
        let font = crate::text::BitmapFont::vga_8x16();
        let scale = font.glyph_scale(run.size_px);
        if scale <= 0.0 {
            // Degenerate/non-finite size_px: BitmapFont::glyph_scale is
            // total and returns 0.0 here rather than propagating NaN/inf —
            // nothing legible to paint at zero-or-negative effective size.
            return;
        }
        // Monospace: every char advances by the same cell width (any char
        // works as the probe; `advance` ignores it — see BitmapFont's docs).
        let advance = font.advance(' ', run.size_px);
        if !advance.is_finite() {
            return;
        }

        let mut cursor_x = run.x as f32;
        let baseline = run.baseline as f32;
        for ch in run.text.chars() {
            self.draw_glyph(font.glyph(ch), cursor_x, baseline, scale, run.color);
            cursor_x += advance;
            if !cursor_x.is_finite() {
                break; // pathologically long run at a pathological advance: stop rather than loop on inf/NaN math.
            }
        }
    }
}

/// Native pixel dimensions of one `text::glyphs` glyph — see that module's
/// doc comment. Fixed regardless of the `BitmapFont` cell geometry in play;
/// only `BitmapFont::vga_8x16` (an 8-wide cell) is ever used to rasterize a
/// real document, so this lines up with `MemSurface::draw_text`'s only
/// caller by construction.
const GLYPH_W: usize = 8;
const GLYPH_H: usize = 8;

/// Hard cap on one glyph's rasterized pixel footprint (width or height),
/// independent of the requested `size_px`. `size_px` is ultimately
/// document-controlled (an author stylesheet's `font-size`), so an
/// unbounded/hostile value must not blow `draw_glyph`'s per-pixel scan up
/// into an effectively-unbounded `O(w * h)` loop. `1024` is already far
/// larger than any real heading (`fixtures/basic.html`'s largest is a 32px
/// `h1`, a 16x16 glyph box) while keeping the worst case (`1024 * 1024` ==
/// ~1M pixel writes, each already bounds-checked/clipped by `put_pixel`) a
/// bounded, fast constant.
const MAX_GLYPH_PX: f32 = 1024.0;

impl MemSurface {
    /// Paint one glyph's lit pixels in `color`, nearest-neighbor-scaled by
    /// `scale`, with its bottom row sitting exactly on `baseline` — i.e. the
    /// glyph's `GLYPH_H`-pixel-tall bounding box spans
    /// `[baseline - GLYPH_H * scale, baseline)` vertically and
    /// `[x0, x0 + GLYPH_W * scale)` horizontally. This is the documented
    /// placement choice for embedding an 8-tall source glyph inside
    /// `BitmapFont::vga_8x16`'s taller 16-design-unit cell (12 ascent / 4
    /// descent, see `text::bitmap`'s docs): rather than pinning the glyph to
    /// the cell's top or geometric center, sitting its bottom edge on the
    /// baseline is what every real font rasterizer does for a
    /// non-descending glyph, and — as a bonus here — 8 is exactly half of
    /// vga_8x16's 16-unit cell_height, so at the font's native size_px
    /// (16.0, scale 1.0) the glyph occupies design rows `[4, 12)`: centered
    /// in the cell AND baseline-bottom-aligned at once, no tradeoff to make.
    ///
    /// `scale` is guaranteed `> 0.0` and finite by the only caller
    /// (`draw_text`, via `BitmapFont::glyph_scale`'s totality contract).
    /// Nearest-neighbor sampling: for each OUTPUT pixel in the glyph's
    /// scaled bounding box, the matching SOURCE pixel is `floor(offset /
    /// scale)`, clamped into `0..GLYPH_W`/`0..GLYPH_H` — this (rather than
    /// iterating source pixels and filling a variable-sized band) handles
    /// non-integer `scale` (e.g. a 24px heading, scale 1.5) with no gaps or
    /// overlaps in the output, for upscale AND downscale alike.
    fn draw_glyph(&mut self, bitmap: [u8; GLYPH_H], x0: f32, baseline: f32, scale: f32, color: Color) {
        let w_px = ((GLYPH_W as f32 * scale).round().clamp(0.0, MAX_GLYPH_PX)) as i32;
        let h_px = ((GLYPH_H as f32 * scale).round().clamp(0.0, MAX_GLYPH_PX)) as i32;
        if w_px <= 0 || h_px <= 0 {
            return;
        }
        let y0 = baseline - h_px as f32;
        if !x0.is_finite() || !y0.is_finite() {
            return;
        }

        for py in 0..h_px {
            let src_row = ((py as f32 / scale) as usize).min(GLYPH_H - 1);
            let row_bits = bitmap[src_row];
            for px in 0..w_px {
                let src_col = ((px as f32 / scale) as usize).min(GLYPH_W - 1);
                if row_bits & (1u8 << src_col) != 0 {
                    let x = x0 + px as f32;
                    let y = y0 + py as f32;
                    // `put_pixel` is itself the clip/OOB guard (frozen
                    // Surface contract) — no separate bounds check needed
                    // here for totality on off-surface glyph placement.
                    self.put_pixel(x.floor() as i32, y.floor() as i32, color);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_and_readback_is_opaque() {
        let mut s = MemSurface::new(2, 2, Color::WHITE);
        s.fill_rect(Rect { x: 0, y: 0, w: 1, h: 1 }, Color::BLACK);
        // top-left black, its neighbor still white
        assert_eq!(&s.bytes()[0..4], &[0, 0, 0, 255]);
        assert_eq!(&s.bytes()[4..8], &[255, 255, 255, 255]);
    }

    #[test]
    fn out_of_bounds_pixels_are_ignored() {
        let mut s = MemSurface::new(1, 1, Color::WHITE);
        s.put_pixel(-1, 0, Color::BLACK);
        s.put_pixel(0, 5, Color::BLACK);
        assert_eq!(&s.bytes()[0..4], &[255, 255, 255, 255]);
    }

    // --------------------------------------------------------------- draw_text

    /// Read back the RGBA pixel at `(x, y)` as an `(r, g, b, a)` tuple.
    fn px(s: &MemSurface, x: i32, y: i32) -> (u8, u8, u8, u8) {
        let i = ((y as usize) * (s.width as usize) + (x as usize)) * 4;
        (s.pixels[i], s.pixels[i + 1], s.pixels[i + 2], s.pixels[i + 3])
    }

    /// `true` at every `(x, y)` where the embedded 'A' glyph
    /// (`text::glyphs`'s `0x0C, 0x1E, 0x33, 0x33, 0x3F, 0x33, 0x33, 0x00`,
    /// bit 0 = leftmost pixel) is lit, for an 8x8 block whose top-left is
    /// `(x0, y0)`.
    fn a_glyph_lit(x: i32, y: i32, x0: i32, y0: i32) -> bool {
        const A: [u8; 8] = [0x0C, 0x1E, 0x33, 0x33, 0x3F, 0x33, 0x33, 0x00];
        let (dx, dy) = (x - x0, y - y0);
        if !(0..8).contains(&dx) || !(0..8).contains(&dy) {
            return false;
        }
        A[dy as usize] & (1 << dx) != 0
    }

    #[test]
    fn draw_text_paints_a_native_size_glyph_bottom_aligned_to_the_baseline() {
        // size_px == 16 == BitmapFont::vga_8x16's cell_height -> scale 1.0,
        // so the 8x8 glyph paints 1:1. The glyph's own doc-comment placement
        // rule (bitmap.rs / mem.rs draw_text): the glyph's bottom row sits
        // exactly on `baseline`, so its pixel box spans
        // `[baseline - 8, baseline)` vertically and `[x, x + 8)` horizontally.
        let mut s = MemSurface::new(20, 20, Color::WHITE);
        let run = TextRun { text: "A", x: 4, baseline: 12, size_px: 16.0, color: Color::BLACK };
        s.draw_text(&run);

        for y in 0..20 {
            for x in 0..20 {
                let expect_lit = a_glyph_lit(x, y, 4, 12 - 8);
                let got = px(&s, x, y);
                if expect_lit {
                    assert_eq!(got, (0, 0, 0, 255), "expected glyph pixel lit at ({x},{y})");
                } else {
                    assert_eq!(got, (255, 255, 255, 255), "expected background at ({x},{y})");
                }
            }
        }
    }

    #[test]
    fn draw_text_advances_by_the_font_cell_width_between_chars() {
        // "II" at size 16: cell_width 8px advance between the two glyphs'
        // origins. Just assert both glyph columns show *some* black ink
        // (rather than re-deriving the full 'I' bitmap) and that there is a
        // gap of all-background columns is not required (font glyphs may
        // touch their cell edge) — the real assertion is total pixel count
        // roughly doubles versus a single "I".
        let mut one = MemSurface::new(24, 20, Color::WHITE);
        one.draw_text(&TextRun { text: "I", x: 0, baseline: 12, size_px: 16.0, color: Color::BLACK });
        let mut two = MemSurface::new(24, 20, Color::WHITE);
        two.draw_text(&TextRun { text: "II", x: 0, baseline: 12, size_px: 16.0, color: Color::BLACK });

        let count_black = |s: &MemSurface| s.bytes().chunks(4).filter(|p| p == &[0, 0, 0, 255]).count();
        let n1 = count_black(&one);
        let n2 = count_black(&two);
        assert!(n1 > 0, "single glyph should paint some ink");
        assert_eq!(n2, n1 * 2, "two identical glyphs should paint exactly twice the ink of one");
    }

    #[test]
    fn draw_text_scales_the_glyph_nearest_neighbor_at_2x_size() {
        // size_px == 32 -> scale 2.0: each source pixel becomes a 2x2 block.
        // Spot-check a couple of lit/unlit source pixels from the 'A' glyph
        // (row 4 == the crossbar, 0x3F, columns 0..=5 lit; column 6 unlit)
        // rather than the whole 16x16 box.
        let mut s = MemSurface::new(40, 40, Color::WHITE);
        let run = TextRun { text: "A", x: 0, baseline: 24, size_px: 32.0, color: Color::BLACK };
        s.draw_text(&run);

        // Row 4 (crossbar) at scale 2 occupies output rows [8,10) within the
        // glyph box, whose box top is baseline - 16 = 8. So output rows
        // 8+4*2=16..18.
        for y in 16..18 {
            for x in 0..12 {
                // columns 0..5 lit -> scaled x in [0,12)
                assert_eq!(px(&s, x, y), (0, 0, 0, 255), "expected crossbar ink at ({x},{y})");
            }
            // column 6 (0x3F bit6==0) unlit -> scaled x in [12,14)
            assert_eq!(px(&s, 12, y), (255, 255, 255, 255), "expected gap after crossbar at (12,{y})");
        }
    }

    #[test]
    fn draw_text_empty_string_is_a_no_op() {
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        s.draw_text(&TextRun { text: "", x: 0, baseline: 8, size_px: 16.0, color: Color::BLACK });
        for i in (0..s.bytes().len()).step_by(4) {
            assert_eq!(&s.bytes()[i..i + 4], &[255, 255, 255, 255]);
        }
    }

    #[test]
    fn draw_text_degenerate_size_px_is_a_no_op_not_a_panic() {
        for size in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut s = MemSurface::new(10, 10, Color::WHITE);
            s.draw_text(&TextRun { text: "A", x: 0, baseline: 8, size_px: size, color: Color::BLACK });
            for i in (0..s.bytes().len()).step_by(4) {
                assert_eq!(&s.bytes()[i..i + 4], &[255, 255, 255, 255]);
            }
        }
    }

    #[test]
    fn draw_text_off_surface_bounds_never_panics() {
        let mut s = MemSurface::new(4, 4, Color::WHITE);
        let degenerate = [
            (i32::MIN, i32::MIN),
            (i32::MAX, i32::MAX),
            (-1000, -1000),
            (10_000, 10_000),
        ];
        for (x, baseline) in degenerate {
            s.draw_text(&TextRun { text: "hello world", x, baseline, size_px: 16.0, color: Color::BLACK });
        }
        // Must not panic; surface stays a valid 4x4 buffer.
        assert_eq!(s.bytes().len(), 4 * 4 * 4);
    }

    #[test]
    fn draw_text_huge_size_px_is_bounded_not_a_hang_or_panic() {
        let mut s = MemSurface::new(4, 4, Color::WHITE);
        s.draw_text(&TextRun { text: "A", x: 0, baseline: 0, size_px: f32::MAX, color: Color::BLACK });
        assert_eq!(s.bytes().len(), 4 * 4 * 4);
    }

    #[test]
    fn draw_text_respects_run_color() {
        let mut s = MemSurface::new(20, 20, Color::WHITE);
        let red = Color::rgb(200, 10, 10);
        s.draw_text(&TextRun { text: "A", x: 4, baseline: 12, size_px: 16.0, color: red });
        // The 'A' glyph's crossbar row (design row 4) is fully lit
        // columns 0..=5; at x0=4 that's surface columns 4..=9, row
        // baseline-8+4 = 8.
        assert_eq!(px(&s, 4, 8), (200, 10, 10, 255));
    }

    #[test]
    fn draw_text_zero_alpha_color_is_a_no_op() {
        let mut s = MemSurface::new(20, 20, Color::WHITE);
        let transparent_black = Color::rgba(0, 0, 0, 0);
        s.draw_text(&TextRun { text: "A", x: 4, baseline: 12, size_px: 16.0, color: transparent_black });
        for i in (0..s.bytes().len()).step_by(4) {
            assert_eq!(&s.bytes()[i..i + 4], &[255, 255, 255, 255]);
        }
    }
}
