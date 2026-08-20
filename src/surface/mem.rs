//! An in-memory RGBA surface — the substrate for pixel-exact golden tests
//! (brief §7). Needs no display, so CI and the fixture suite never touch fbdev
//! or X. P7 blesses renders produced through this; P9 shares its pixel ops.

use super::{Color, Rect, Surface, TextRun};
use crate::text::terminus::TerminusFont;

/// A flat RGBA8 framebuffer in memory.
#[derive(Debug, Clone)]
pub struct MemSurface {
    width: u32,
    height: u32,
    /// `width * height * 4` bytes, row-major, RGBA.
    pixels: Vec<u8>,
    /// The current clip rectangle (Acid2 Packet 5, Task 2), in the same
    /// pixel-`Rect` space as `fill_rect`/`blit` -- `None` means unclipped.
    /// Set per-fragment by `backend::raster::paint_at` via `set_clip`;
    /// checked in `put_pixel`, the choke point every draw op (`fill_rect`,
    /// `blit`, `draw_glyph`) already routes through, so one check there
    /// clips all of them uniformly (see this module's own top-of-file doc
    /// comment and the brief's "key design insight").
    clip: Option<Rect>,
}

impl MemSurface {
    /// A new surface filled with `bg`.
    pub fn new(width: u32, height: u32, bg: Color) -> Self {
        let mut s = MemSurface {
            width,
            height,
            pixels: vec![0; (width as usize) * (height as usize) * 4],
            clip: None,
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
        // Acid2 Packet 5, Task 2: `overflow:hidden` paint clipping. Every
        // draw op (`fill_rect`'s loop, `blit`'s per-pixel blend, `draw_text`/
        // `draw_glyph`) funnels through `put_pixel`, so this one check clips
        // all of them uniformly -- see `clip`'s own field doc comment.
        if let Some(c) = self.clip {
            if x < c.x || y < c.y || x >= c.x + c.w as i32 || y >= c.y + c.h as i32 {
                return;
            }
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

    /// Rasterize `run` glyph-by-glyph via the embedded Terminus subset
    /// (packet/terminus-font, replacing the earlier font8x8-via-`BitmapFont`
    /// atlas). See module docs above `draw_glyph` for the placement rules
    /// pinned by the `draw_text_*` tests below.
    ///
    /// `run.size_px` snaps to the nearest of Terminus's 5 embedded buckets
    /// (`TerminusFont::glyph`, internally) — unlike the old font8x8 path,
    /// there is no continuous up/downscale left to do, so every glyph in a
    /// run paints at its bucket's real, native pixel size. Total: ANY
    /// `size_px` (including `0.0`, negative, `NaN`, `+-infinity`) resolves
    /// to a real, legible bucket rather than a no-op — see
    /// `text::terminus::nearest_terminus_size`'s own doc comment for why
    /// this is a deliberate contract (a generalization of the project's
    /// long-standing "never render illegibly small" floor, not a
    /// regression); `draw_text_degenerate_size_px_snaps_to_a_legible_bucket_not_a_panic`
    /// pins this explicitly.
    fn draw_text(&mut self, run: &TextRun) {
        if run.text.is_empty() || run.color.a == 0 {
            return;
        }
        let font = TerminusFont::new();
        // Monospace: every glyph at a given (weight, snapped size) shares
        // the same cell width — probing with a fixed char is safe and
        // avoids computing the snap twice per char.
        let advance = font.glyph(' ', run.weight, run.size_px).cell_w as f32;
        if advance <= 0.0 {
            return; // unreachable given TerminusFont's totality, but defensive.
        }

        let mut cursor_x = run.x as f32;
        let baseline = run.baseline as f32;
        for ch in run.text.chars() {
            let glyph = font.glyph(ch, run.weight, run.size_px);
            self.draw_glyph(&glyph, cursor_x, baseline, run.color);
            cursor_x += advance;
            if !cursor_x.is_finite() {
                break; // pathologically long run at a pathological advance: stop rather than loop on inf/NaN math.
            }
        }
    }

    fn set_clip(&mut self, clip: Option<Rect>) {
        self.clip = clip;
    }
}

/// Real upper bounds on any embedded Terminus glyph's cell size (the 32px
/// bucket's 16x32 cell is the largest of the 5 embedded buckets —
/// `text::terminus::TerminusFont`). `draw_glyph` clamps `cell_w`/`cell_h`
/// against these defensively so an (unreachable via any real call path, but
/// hypothetically hostile) out-of-range glyph can never overflow the `u16`
/// row bit-shift below or walk past `Glyph::rows`'s fixed-size backing
/// array, rather than trusting the caller's cell size unconditionally.
const MAX_CELL_W: i32 = 16;
const MAX_CELL_H: i32 = 32;

impl MemSurface {
    /// Paint one glyph's lit pixels in `color`, with row `ascent` (the
    /// bucket's pinned ascent, carried on the glyph itself — see
    /// `terminus::Glyph::ascent`'s doc comment) sitting exactly on
    /// `baseline` — i.e. the glyph's `cell_h`-pixel-tall bounding box spans
    /// `[baseline - ascent, baseline - ascent + cell_h)` vertically and
    /// `[x0, x0 + cell_w)` horizontally. Terminus's embedded bitmaps are
    /// already at their real, native target size once `TerminusFont::glyph`
    /// has snapped `size_px` to a bucket — unlike font8x8 (always an 8x8
    /// source, nearest-neighbor-scaled to fit `size_px`), there is no
    /// upscale/downscale left to do here: this is a straight 1:1 copy of
    /// `glyph.rows`.
    ///
    /// Review fix (code review on packet/terminus-font, the blocker):
    /// placing the glyph at `baseline - cell_h` (the old font8x8-era
    /// formula) only worked for font8x8 because its 8-row glyph atlas was
    /// smaller than its 16-row cell. Terminus's bitmaps encode a real
    /// ascent/descent split INSIDE `cell_h` (`ascent + descent == cell_h`
    /// at every bucket) — anchoring on `cell_h` instead of `ascent` shifted
    /// every glyph up by `descent` px, so descenders (g, y, p, q, j) never
    /// dipped below the baseline and clipped contexts lost their top
    /// `descent` rows.
    ///
    /// Review fix (Important #1, carried forward from the font8x8-era
    /// implementation): compute the glyph's screen-space bounding box up
    /// front and bail out in O(1) if it doesn't intersect the surface at
    /// all, before ever entering the pixel loop below — a long document
    /// with many off-screen glyphs must stay O(1) per glyph, not
    /// O(cell_w * cell_h) per glyph, regardless of character count.
    fn draw_glyph(&mut self, glyph: &crate::text::terminus::Glyph, x0: f32, baseline: f32, color: Color) {
        let w_px = (glyph.cell_w as i32).clamp(0, MAX_CELL_W);
        let h_px = (glyph.cell_h as i32).clamp(0, MAX_CELL_H);
        if w_px <= 0 || h_px <= 0 {
            return;
        }
        let y0 = baseline - glyph.ascent as f32;
        if !x0.is_finite() || !y0.is_finite() {
            return;
        }

        // O(1) screen-bbox-vs-surface intersection check (see this
        // function's doc comment).
        let (surface_w, surface_h) = (self.width as f32, self.height as f32);
        let x1 = x0 + w_px as f32;
        let y1 = y0 + h_px as f32;
        if x1 <= 0.0 || y1 <= 0.0 || x0 >= surface_w || y0 >= surface_h {
            return;
        }

        for py in 0..h_px {
            let row_bits = glyph.rows[py as usize];
            for px in 0..w_px {
                if row_bits & (1u16 << px) != 0 {
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

    /// Acid2 Packet 5, Task 2: `set_clip` scissors every draw op that routes
    /// through `put_pixel` -- `fill_rect`'s loop is the simplest probe (a
    /// large fill against a small clip should only paint the overlap).
    #[test]
    fn set_clip_restricts_fill_rect_to_the_clip_rectangle() {
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        s.set_clip(Some(Rect { x: 2, y: 2, w: 3, h: 3 })); // covers pixels [2,5) x [2,5)
        s.fill_rect(Rect { x: 0, y: 0, w: 10, h: 10 }, Color::BLACK);

        // Inside the clip: painted.
        assert_eq!(&s.bytes()[((3 * 10 + 3) * 4)..((3 * 10 + 3) * 4 + 4)], &[0, 0, 0, 255], "inside the clip must paint");
        // Outside the clip (but inside the fill_rect's own bounds): untouched.
        assert_eq!(
            &s.bytes()[((0 * 10 + 0) * 4)..((0 * 10 + 0) * 4 + 4)],
            &[255, 255, 255, 255],
            "outside the clip must stay the background color"
        );
        assert_eq!(
            &s.bytes()[((7 * 10 + 7) * 4)..((7 * 10 + 7) * 4 + 4)],
            &[255, 255, 255, 255],
            "outside the clip (past its far edge) must stay the background color"
        );

        // `set_clip(None)` clears it -- a subsequent fill covers everything again.
        s.set_clip(None);
        s.fill_rect(Rect { x: 0, y: 0, w: 10, h: 10 }, Color::BLACK);
        assert_eq!(&s.bytes()[0..4], &[0, 0, 0, 255], "clearing the clip must restore unclipped painting");
    }

    // --------------------------------------------------------------- draw_text

    use crate::style::computed::FontWeight;

    /// Read back the RGBA pixel at `(x, y)` as an `(r, g, b, a)` tuple.
    fn px(s: &MemSurface, x: i32, y: i32) -> (u8, u8, u8, u8) {
        let i = ((y as usize) * (s.width as usize) + (x as usize)) * 4;
        (s.pixels[i], s.pixels[i + 1], s.pixels[i + 2], s.pixels[i + 3])
    }

    /// `true` at every `(x, y)` where the embedded Terminus 16px
    /// normal-weight 'A' glyph (`0x00, 0x00, 0x3C, 0x42, 0x42, 0x42, 0x42,
    /// 0x7E, 0x42, 0x42, 0x42, 0x42, 0x00, 0x00, 0x00, 0x00`, bit 0 =
    /// leftmost pixel — hand-verified in `src/text/terminus_glyphs_tests.rs`)
    /// is lit, for an 8x16 block whose top-left is `(x0, y0)`.
    fn a_glyph_lit(x: i32, y: i32, x0: i32, y0: i32) -> bool {
        const A: [u8; 16] =
            [0x00, 0x00, 0x3C, 0x42, 0x42, 0x42, 0x42, 0x7E, 0x42, 0x42, 0x42, 0x42, 0x00, 0x00, 0x00, 0x00];
        let (dx, dy) = (x - x0, y - y0);
        if !(0..8).contains(&dx) || !(0..16).contains(&dy) {
            return false;
        }
        A[dy as usize] & (1 << dx) != 0
    }

    #[test]
    fn draw_text_paints_a_native_size_glyph_bottom_aligned_to_the_baseline() {
        // size_px == 16.0 snaps to Terminus's 16px bucket (an 8x16 cell) --
        // the WHOLE cell is the glyph's own native size now (unlike
        // font8x8's 8x8-source-inside-a-16-tall-cell), so this is a
        // straight 1:1 copy, no scaling. The glyph's row `ascent` (NOT its
        // bottom row) sits exactly on `baseline` -- code review fix on
        // packet/terminus-font: `terminus::METRICS[1]` (the 16px bucket) is
        // `(ascent=12.0, descent=4.0, ...)`, so the box's top-left is
        // `baseline - ascent = 16 - 12 = 4`, not `baseline - cell_h = 0`
        // (the old, wrong font8x8-era formula this test used to assert).
        // Its pixel box spans `[4, 4 + 16) = [4, 20)` vertically and
        // `[x, x + 8)` horizontally.
        let mut s = MemSurface::new(20, 20, Color::WHITE);
        let run = TextRun { text: "A", x: 4, baseline: 16, size_px: 16.0, color: Color::BLACK, weight: FontWeight::Normal };
        s.draw_text(&run);

        for y in 0..20 {
            for x in 0..20 {
                let expect_lit = a_glyph_lit(x, y, 4, 16 - 12);
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
        one.draw_text(&TextRun { text: "I", x: 0, baseline: 16, size_px: 16.0, color: Color::BLACK, weight: FontWeight::Normal });
        let mut two = MemSurface::new(24, 20, Color::WHITE);
        two.draw_text(&TextRun { text: "II", x: 0, baseline: 16, size_px: 16.0, color: Color::BLACK, weight: FontWeight::Normal });

        let count_black = |s: &MemSurface| s.bytes().chunks(4).filter(|p| p == &[0, 0, 0, 255]).count();
        let n1 = count_black(&one);
        let n2 = count_black(&two);
        assert!(n1 > 0, "single glyph should paint some ink");
        assert_eq!(n2, n1 * 2, "two identical glyphs should paint exactly twice the ink of one");
    }

    /// packet/terminus-font, Task 3 step 4: a 32px run paints at the WIDE
    /// (`u16`-row, 16x32 cell) bucket correctly -- every column up to 15
    /// must light up as the generated table says, not just the low 8 bits a
    /// naive `u8`-only implementation would have truncated to. Replaces the
    /// old font8x8-era `draw_text_scales_the_glyph_nearest_neighbor_at_2x_size`
    /// test: Terminus doesn't scale a 16px source up to 32px, it paints the
    /// REAL, separately-embedded 32px bitmap 1:1 -- there is no
    /// nearest-neighbor scaling left in this codepath to test.
    #[test]
    fn draw_text_at_the_32px_bucket_paints_the_native_wide_glyph_without_truncation() {
        // Terminus's 32px normal 'A' (hand-verified in
        // src/text/terminus_glyphs_tests.rs), 16 columns wide, LSB-leftmost.
        const A32: [u16; 32] = [
            0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0FF0, 0x1FF8, 0x381C, 0x300C, 0x300C, 0x300C,
            0x300C, 0x300C, 0x300C, 0x300C, 0x3FFC, 0x3FFC, 0x300C, 0x300C, 0x300C, 0x300C, 0x300C, 0x300C,
            0x300C, 0x300C, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
        ];
        let mut s = MemSurface::new(24, 40, Color::WHITE);
        let run = TextRun { text: "A", x: 2, baseline: 32, size_px: 32.0, color: Color::BLACK, weight: FontWeight::Normal };
        s.draw_text(&run);

        // Code review fix on packet/terminus-font: the box's top-left (y0)
        // is `baseline - ascent`, not `baseline - cell_h`. The 32px
        // bucket's ascent is 26.0 (`terminus::METRICS[4]`), so
        // y0 = 32 - 26 = 6 (not 0, the old wrong-formula value) -- glyph
        // row `y` (0-indexed into `A32`) lands at surface row `6 + y`.
        const Y0: i32 = 6;
        for (y, &row) in A32.iter().enumerate() {
            for x in 0..16u32 {
                let expect_lit = (row >> x) & 1 != 0;
                let got = px(&s, 2 + x as i32, Y0 + y as i32);
                if expect_lit {
                    assert_eq!(got, (0, 0, 0, 255), "expected lit pixel at glyph-relative ({x},{y})");
                } else {
                    assert_eq!(got, (255, 255, 255, 255), "expected background at glyph-relative ({x},{y})");
                }
            }
        }
    }

    #[test]
    fn draw_text_empty_string_is_a_no_op() {
        let mut s = MemSurface::new(10, 10, Color::WHITE);
        s.draw_text(&TextRun { text: "", x: 0, baseline: 8, size_px: 16.0, color: Color::BLACK, weight: FontWeight::Normal });
        for i in (0..s.bytes().len()).step_by(4) {
            assert_eq!(&s.bytes()[i..i + 4], &[255, 255, 255, 255]);
        }
    }

    /// packet/terminus-font: this REPLACES the old font8x8-era
    /// `draw_text_degenerate_size_px_is_a_no_op_not_a_panic` test, whose
    /// assertion (nothing paints) is now actively WRONG, not just
    /// out-of-date. `TerminusFont`'s nearest-size snap
    /// (`text::terminus::nearest_terminus_size`) is TOTAL and always
    /// resolves to a real, legible bucket: `0.0`/`-1.0` clamp to the 12px
    /// bucket (a generalization of the project's long-standing "never
    /// render illegibly small" floor, per the design doc §2 -- not a
    /// regression), `NaN`/`+-infinity` default to the 16px bucket. So every
    /// one of these inputs now legitimately PAINTS something; the only
    /// invariant that survives unchanged from the old test is "never
    /// panics."
    #[test]
    fn draw_text_degenerate_size_px_snaps_to_a_legible_bucket_not_a_panic() {
        for size in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut s = MemSurface::new(20, 40, Color::WHITE);
            s.draw_text(&TextRun { text: "A", x: 0, baseline: 32, size_px: size, color: Color::BLACK, weight: FontWeight::Normal }); // must not panic
            let count_black = s.bytes().chunks(4).filter(|p| p == &[0, 0, 0, 255]).count();
            assert!(count_black > 0, "size_px={size} should snap to a real bucket and paint some ink, not be a no-op");
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
            s.draw_text(&TextRun { text: "hello world", x, baseline, size_px: 16.0, color: Color::BLACK, weight: FontWeight::Normal });
        }
        // Must not panic; surface stays a valid 4x4 buffer.
        assert_eq!(s.bytes().len(), 4 * 4 * 4);
    }

    #[test]
    fn draw_text_huge_size_px_is_bounded_not_a_hang_or_panic() {
        let mut s = MemSurface::new(4, 4, Color::WHITE);
        s.draw_text(&TextRun { text: "A", x: 0, baseline: 0, size_px: f32::MAX, color: Color::BLACK, weight: FontWeight::Normal });
        assert_eq!(s.bytes().len(), 4 * 4 * 4);
    }

    /// Review fix (Important #1, carried forward from the font8x8 era): a
    /// glyph placed entirely off-canvas must be an O(1) bbox-skip, not a
    /// full per-pixel scan. Terminus's embedded buckets already bound any
    /// single glyph to at most a 16x32 cell regardless of `size_px` (no
    /// more upscale blowup like font8x8's old `scale`-driven 1024x1024 worst
    /// case), so the raw per-glyph cost is small either way now -- this test
    /// still pins that many off-screen glyphs at a saturating `size_px`
    /// complete promptly and paint nothing, as a regression anchor for the
    /// bbox check itself.
    #[test]
    fn draw_text_skips_off_screen_glyphs_without_paying_their_full_pixel_cost() {
        let mut s = MemSurface::new(50, 50, Color::WHITE);
        let text: String = std::iter::repeat('A').take(512).collect();
        // size_px 4096 -> snaps to the 32px bucket (16x32 cell). Placed far
        // past the 50x50 surface on both axes, so NONE of the 512 glyphs are
        // visible.
        let run = TextRun { text: &text, x: 1_000_000, baseline: 1_000_000, size_px: 4096.0, color: Color::BLACK, weight: FontWeight::Normal };

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
        s.draw_text(&TextRun { text: "A", x: 10_000, baseline: 10_012, size_px: 16.0, color: Color::BLACK, weight: FontWeight::Normal });
        for i in (0..s.bytes().len()).step_by(4) {
            assert_eq!(&s.bytes()[i..i + 4], &[255, 255, 255, 255], "off-screen glyph must write zero pixels");
        }

        s.draw_text(&TextRun { text: "A", x: 4, baseline: 16, size_px: 16.0, color: Color::BLACK, weight: FontWeight::Normal });
        let count_black = s.bytes().chunks(4).filter(|p| p == &[0, 0, 0, 255]).count();
        assert!(count_black > 0, "on-screen glyph should still render after an off-screen one was skipped");
    }

    #[test]
    fn draw_text_respects_run_color() {
        let mut s = MemSurface::new(20, 20, Color::WHITE);
        let red = Color::rgb(200, 10, 10);
        s.draw_text(&TextRun { text: "A", x: 4, baseline: 16, size_px: 16.0, color: red, weight: FontWeight::Normal });
        // The 'A' glyph's crossbar row (row index 7 of the 16-row cell,
        // 0x7E) lights columns 1..=6. Code review fix on packet/terminus-
        // font: the box's top-left (y0) is `baseline - ascent`, not
        // `baseline - cell_h` -- the 16px bucket's ascent is 12.0
        // (`terminus::METRICS[1]`), so y0 = 16 - 12 = 4, and row 7 lands at
        // surface row 4 + 7 = 11 (not row 7, the old wrong-formula value).
        // At x0=4 that's surface column 4+1=5.
        assert_eq!(px(&s, 5, 11), (200, 10, 10, 255));
    }

    /// packet/terminus-font, Task 3 step 2: bold vs. normal at the same
    /// char/size must produce a DIFFERENT lit-pixel set -- proves `run.weight`
    /// actually reaches `TerminusFont::glyph` and selects a different glyph
    /// table, not just that the field exists on `TextRun`.
    #[test]
    fn draw_text_bold_vs_normal_weight_paints_different_pixels() {
        let paint_with = |weight: FontWeight| {
            let mut s = MemSurface::new(20, 20, Color::WHITE);
            s.draw_text(&TextRun { text: "A", x: 4, baseline: 16, size_px: 16.0, color: Color::BLACK, weight });
            s.bytes().to_vec()
        };
        let normal = paint_with(FontWeight::Normal);
        let bold = paint_with(FontWeight::Bold);
        assert_ne!(normal, bold, "bold and normal 'A' at the same size must paint different pixels");
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
        s.draw_text(&TextRun { text: "A", x: 4, baseline: 12, size_px: 16.0, color: transparent_black, weight: FontWeight::Normal });
        for i in (0..s.bytes().len()).step_by(4) {
            assert_eq!(&s.bytes()[i..i + 4], &[255, 255, 255, 255]);
        }
    }
}
