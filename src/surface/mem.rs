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

    /// Copy `image` into `at`, nearest-neighbor-scaled to `at`'s size,
    /// alpha-blending each source pixel over the destination via the same
    /// `put_pixel` blend `fill_rect`/`draw_glyph` already use.
    ///
    /// Total: a zero-sized `image` (either dimension) or zero-sized `at`
    /// (either dimension) is a no-op — no image content to sample / no
    /// destination area to paint, and either would otherwise divide by
    /// zero in the scale math. Coordinates are widened to `i64` before any
    /// arithmetic so a huge/degenerate `at` (`u32::MAX` width, an `i32`
    /// origin near a bound) can't overflow computing its far edge; the
    /// destination rect is then clipped to the surface bounds up front, so
    /// the pixel loop below only ever visits on-surface, in-bounds
    /// destination pixels (never relying on `put_pixel`'s own clip as the
    /// only guard, unlike `draw_glyph`, since the loop bounds themselves
    /// must stay a small, finite range regardless of how huge `at` is).
    fn blit(&mut self, at: Rect, image: &crate::img::RgbaImage) {
        if image.width == 0 || image.height == 0 || at.w == 0 || at.h == 0 {
            return;
        }

        let dst_x0 = at.x as i64;
        let dst_y0 = at.y as i64;
        let dst_x1 = dst_x0 + at.w as i64;
        let dst_y1 = dst_y0 + at.h as i64;

        let clip_x0 = dst_x0.max(0);
        let clip_y0 = dst_y0.max(0);
        let clip_x1 = dst_x1.min(self.width as i64);
        let clip_y1 = dst_y1.min(self.height as i64);
        if clip_x1 <= clip_x0 || clip_y1 <= clip_y0 {
            return; // `at` doesn't intersect the surface at all.
        }

        let (img_w, img_h) = (image.width as u64, image.height as u64);
        let (at_w, at_h) = (at.w as u64, at.h as u64);

        for y in clip_y0..clip_y1 {
            let rel_y = (y - dst_y0) as u64;
            let src_y = ((rel_y * img_h) / at_h).min(img_h - 1) as u32;
            for x in clip_x0..clip_x1 {
                let rel_x = (x - dst_x0) as u64;
                let src_x = ((rel_x * img_w) / at_w).min(img_w - 1) as u32;
                let idx = ((src_y as usize) * (image.width as usize) + (src_x as usize)) * 4;
                let Some(src_px) = image.pixels.get(idx..idx + 4) else { continue };
                let color = Color { r: src_px[0], g: src_px[1], b: src_px[2], a: src_px[3] };
                self.put_pixel(x as i32, y as i32, color);
            }
        }
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
    ///
    /// Review fix (Important #1): `MAX_GLYPH_PX` alone bounds ONE glyph's
    /// pixel loop to a fixed worst case (~1024*1024 iterations at a
    /// saturating `scale`), but does nothing about a glyph placed entirely
    /// off-canvas — every iteration's `put_pixel` would still run, just to
    /// be silently clipped. A long document with a huge author `font-size`
    /// (reachable input: the response body cap is 64MiB, plenty of room for
    /// a lot of off-screen text) turns that into `O(chars * 1024^2)` wasted
    /// work regardless of how little (or nothing) is actually visible — an
    /// aggregate CPU-hang vector, not just a per-glyph one. So: compute the
    /// glyph's screen-space bounding box up front and bail out in O(1) if it
    /// doesn't intersect the surface at all, before ever entering the pixel
    /// loop below.
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

        // O(1) screen-bbox-vs-surface intersection check: an entirely
        // off-canvas glyph (past the right/bottom edge, or past the
        // left/top edge) returns here instead of paying for the pixel loop
        // at all. `put_pixel` would have clipped every write anyway; this
        // just stops doing `w_px * h_px` wasted work to get there.
        let (surface_w, surface_h) = (self.width as f32, self.height as f32);
        let x1 = x0 + w_px as f32;
        let y1 = y0 + h_px as f32;
        if x1 <= 0.0 || y1 <= 0.0 || x0 >= surface_w || y0 >= surface_h {
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

    /// Review fix (Important #1): `MAX_GLYPH_PX` bounds ONE glyph's pixel
    /// loop, but with no screen-bbox-vs-surface intersection check, a glyph
    /// placed entirely off-canvas still ran the full `O(w_px * h_px)` loop
    /// (up to ~1024*1024 iterations at a saturating `size_px`) only to have
    /// every `put_pixel` silently clip it. Many off-screen characters at a
    /// saturating `size_px` (a real, reachable shape: a long document with a
    /// huge author `font-size`, well within the 64MiB response-body cap) is
    /// an aggregate CPU-hang vector. Assert this completes promptly — before
    /// the fix, 512 chars * ~1M wasted iterations each is measurably slow;
    /// after it, an off-screen glyph is an O(1) bbox check regardless of
    /// character count.
    #[test]
    fn draw_text_skips_off_screen_glyphs_without_paying_their_full_pixel_cost() {
        let mut s = MemSurface::new(50, 50, Color::WHITE);
        let text: String = std::iter::repeat('A').take(512).collect();
        // size_px 4096 -> scale 256 -> w_px/h_px both saturate MAX_GLYPH_PX
        // (1024), so each glyph's *unfixed* inner loop is ~1024*1024 == ~1M
        // iterations. Placed far past the 50x50 surface on both axes, so
        // NONE of the 512 glyphs are visible.
        let run = TextRun { text: &text, x: 1_000_000, baseline: 1_000_000, size_px: 4096.0, color: Color::BLACK };

        let start = std::time::Instant::now();
        s.draw_text(&run);
        let elapsed = start.elapsed();

        assert!(
            elapsed.as_secs_f64() < 1.0,
            "512 off-screen glyphs at a saturating size_px took {elapsed:?} -- \
             should be an O(1) bbox-skip per glyph, not O(pixels) per glyph"
        );
        for i in (0..s.bytes().len()).step_by(4) {
            assert_eq!(&s.bytes()[i..i + 4], &[255, 255, 255, 255], "off-screen text must not paint any pixel");
        }
    }

    #[test]
    fn off_screen_glyph_writes_zero_pixels_while_an_on_screen_one_still_renders() {
        let mut s = MemSurface::new(20, 20, Color::WHITE);
        s.draw_text(&TextRun { text: "A", x: 10_000, baseline: 10_012, size_px: 16.0, color: Color::BLACK });
        for i in (0..s.bytes().len()).step_by(4) {
            assert_eq!(&s.bytes()[i..i + 4], &[255, 255, 255, 255], "off-screen glyph must write zero pixels");
        }

        s.draw_text(&TextRun { text: "A", x: 4, baseline: 12, size_px: 16.0, color: Color::BLACK });
        let count_black = s.bytes().chunks(4).filter(|p| p == &[0, 0, 0, 255]).count();
        assert!(count_black > 0, "on-screen glyph should still render after an off-screen one was skipped");
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

    // ------------------------------------------------------------------ blit

    use crate::img::RgbaImage;

    /// A tiny `RgbaImage` built from a flat list of `(r,g,b,a)` tuples,
    /// row-major, `w * h` long.
    fn image(w: u32, h: u32, px: &[(u8, u8, u8, u8)]) -> RgbaImage {
        assert_eq!(px.len(), (w * h) as usize);
        let mut pixels = Vec::with_capacity(px.len() * 4);
        for &(r, g, b, a) in px {
            pixels.extend_from_slice(&[r, g, b, a]);
        }
        RgbaImage { width: w, height: h, pixels }
    }

    #[test]
    fn blit_scales_the_image_nearest_neighbor_to_fill_the_target_rect() {
        // A 2x1 image (red | blue) blitted into a 4x2 target: nearest-
        // neighbor scaling should split the target exactly in half (columns
        // 0-1 red, columns 2-3 blue), replicated down both rows.
        let img = image(2, 1, &[(255, 0, 0, 255), (0, 0, 255, 255)]);
        let mut s = MemSurface::new(4, 2, Color::WHITE);
        s.blit(Rect { x: 0, y: 0, w: 4, h: 2 }, &img);

        for y in 0..2 {
            assert_eq!(px(&s, 0, y), (255, 0, 0, 255));
            assert_eq!(px(&s, 1, y), (255, 0, 0, 255));
            assert_eq!(px(&s, 2, y), (0, 0, 255, 255));
            assert_eq!(px(&s, 3, y), (0, 0, 255, 255));
        }
    }

    #[test]
    fn blit_alpha_blends_a_semi_transparent_source_over_the_background() {
        let img = image(1, 1, &[(0, 0, 0, 128)]); // half-opaque black
        let mut s = MemSurface::new(1, 1, Color::WHITE);
        s.blit(Rect { x: 0, y: 0, w: 1, h: 1 }, &img);
        // Same source-over blend `put_pixel` already implements: expect a
        // roughly 50/50 mix of black over white, not pure black or white.
        let (r, g, b, a) = px(&s, 0, 0);
        assert!(r > 0 && r < 255, "expected a blended value, got r={r}");
        assert_eq!((r, g, b), (r, r, r), "grayscale blend stays grayscale");
        assert_eq!(a, 255);
    }

    #[test]
    fn blit_with_zero_width_image_is_a_no_op() {
        let img = RgbaImage { width: 0, height: 3, pixels: Vec::new() };
        let mut s = MemSurface::new(4, 4, Color::WHITE);
        s.blit(Rect { x: 0, y: 0, w: 4, h: 4 }, &img);
        for i in (0..s.bytes().len()).step_by(4) {
            assert_eq!(&s.bytes()[i..i + 4], &[255, 255, 255, 255]);
        }
    }

    #[test]
    fn blit_with_zero_height_image_is_a_no_op() {
        let img = RgbaImage { width: 3, height: 0, pixels: Vec::new() };
        let mut s = MemSurface::new(4, 4, Color::WHITE);
        s.blit(Rect { x: 0, y: 0, w: 4, h: 4 }, &img);
        for i in (0..s.bytes().len()).step_by(4) {
            assert_eq!(&s.bytes()[i..i + 4], &[255, 255, 255, 255]);
        }
    }

    #[test]
    fn blit_with_zero_size_target_rect_is_a_no_op() {
        let img = image(1, 1, &[(0, 0, 0, 255)]);
        let mut s = MemSurface::new(4, 4, Color::WHITE);
        s.blit(Rect { x: 0, y: 0, w: 0, h: 0 }, &img);
        for i in (0..s.bytes().len()).step_by(4) {
            assert_eq!(&s.bytes()[i..i + 4], &[255, 255, 255, 255]);
        }
    }

    #[test]
    fn blit_clips_to_surface_bounds_when_the_target_rect_overhangs() {
        // A solid black 2x2 image blitted at (2,2) on a 3x3 surface: the
        // target rect (2,2)-(4,4) hangs off the right/bottom edge, so only
        // the top-left pixel of the target rect actually lands on-surface.
        let img = image(2, 2, &[(0, 0, 0, 255); 4]);
        let mut s = MemSurface::new(3, 3, Color::WHITE);
        s.blit(Rect { x: 2, y: 2, w: 2, h: 2 }, &img);
        assert_eq!(px(&s, 2, 2), (0, 0, 0, 255), "on-surface corner should be painted");
        // Every other pixel (nothing else is in-bounds for this rect) stays background.
        for y in 0..3 {
            for x in 0..3 {
                if (x, y) != (2, 2) {
                    assert_eq!(px(&s, x, y), (255, 255, 255, 255), "unexpected paint at ({x},{y})");
                }
            }
        }
    }

    #[test]
    fn blit_entirely_off_surface_never_panics_and_paints_nothing() {
        let img = image(1, 1, &[(0, 0, 0, 255)]);
        let mut s = MemSurface::new(4, 4, Color::WHITE);
        s.blit(Rect { x: -100, y: -100, w: 5, h: 5 }, &img);
        s.blit(Rect { x: 1000, y: 1000, w: 5, h: 5 }, &img);
        for i in (0..s.bytes().len()).step_by(4) {
            assert_eq!(&s.bytes()[i..i + 4], &[255, 255, 255, 255]);
        }
    }

    #[test]
    fn blit_with_huge_target_rect_clips_rather_than_panicking() {
        let img = image(1, 1, &[(0, 0, 0, 255)]);
        let mut s = MemSurface::new(4, 4, Color::WHITE);
        s.blit(Rect { x: i32::MIN / 2, y: i32::MIN / 2, w: u32::MAX, h: u32::MAX }, &img);
        // Must not panic; surface stays valid. (Whether it paints anything
        // is incidental — the huge rect may or may not intersect the
        // surface depending on where its origin lands; the point is totality.)
        assert_eq!(s.bytes().len(), 4 * 4 * 4);
    }

    /// Review finding (Minor, quick test gap): `blit`'s `image.pixels.get(idx
    /// ..idx + 4)` guard against a `RgbaImage` whose `pixels` buffer is
    /// shorter than `width * height * 4` implies (a malformed/truncated
    /// decode result reaching `blit` -- unreachable via the real P4
    /// decoders/`images::collect_images` today, but `blit` is a `Surface`
    /// trait method or a hostile future caller could still construct one)
    /// was correct but untested. Pins: out-of-range source pixels are
    /// silently skipped (destination stays background), never a panic.
    #[test]
    fn blit_with_a_truncated_pixel_buffer_skips_out_of_range_pixels_without_panicking() {
        // A 2x2 image claims 16 bytes of pixels (2*2*4) but only carries 4
        // (one real pixel, opaque black, at index 0). Blit 1:1 into a 2x2
        // surface: only the (0,0) source pixel is in-bounds; the other
        // three destination pixels have no backing source data and must
        // stay background rather than reading past the buffer's end.
        let img = crate::img::RgbaImage { width: 2, height: 2, pixels: vec![0, 0, 0, 255] };
        let mut s = MemSurface::new(2, 2, Color::WHITE);
        s.blit(Rect { x: 0, y: 0, w: 2, h: 2 }, &img); // must not panic
        assert_eq!(px(&s, 0, 0), (0, 0, 0, 255), "the one in-bounds source pixel should still paint");
        assert_eq!(px(&s, 1, 0), (255, 255, 255, 255), "out-of-range source pixel must be skipped, not read OOB");
        assert_eq!(px(&s, 0, 1), (255, 255, 255, 255), "out-of-range source pixel must be skipped, not read OOB");
        assert_eq!(px(&s, 1, 1), (255, 255, 255, 255), "out-of-range source pixel must be skipped, not read OOB");
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
