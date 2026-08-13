//! A monospace bitmap-font metrics model (P5, Wave 1).
//!
//! Scope note: this module implements ONLY the frozen [`Metrics`] seam —
//! advances and vertical metrics derived from a fixed cell geometry. Glyph
//! bitmaps, the atlas, and rasterization are deliberately deferred to the
//! fb-backend packet (P9), which is where pixels actually get consumed; the
//! frozen `Metrics` trait has no glyph-bitmap method, so none is added here.
//! Proportional (non-monospace) fonts are also deferred — v0 is monospace,
//! matching the tty cell grid and the brief's §5 lean toward a bitmap atlas.
//!
//! ## Cell geometry and baseline split
//!
//! A [`BitmapFont`] is defined by a fixed cell box in font design units
//! (`cell_width` × `cell_height`), plus a baseline split of that height into
//! `ascent_units` (above baseline) and `descent_units` (below baseline, used
//! for descenders/underline). All four are held as `f32` design units and
//! scaled to a requested `size_px` by the ratio `size_px / cell_height`.
//!
//! [`BitmapFont::vga_8x16`] models the classic VGA text-mode 8×16 bitmap
//! font (e.g. IBM code-page fonts): an 8-wide, 16-tall cell with the
//! baseline at design row 12 (0-indexed) — i.e. a 12:4 (3:1) ascent:descent
//! split, leaving 4 design rows below the baseline for descenders and the
//! underline row. [`BitmapFont::with_cell`] generalizes this to any cell
//! size using the same 3:1 ratio, for tests/other fixed grids.
//!
//! Because `ascent_units + descent_units == cell_height` by construction,
//! `line_height(size_px)` is exactly `size_px` — baseline-to-baseline
//! advance equals the requested font size, matching a terminal-style cell
//! grid where line spacing has no extra leading.

use super::Metrics;

/// Monospace bitmap-font metrics: a fixed cell box scaled to `size_px`.
///
/// Never stores or looks up glyph bitmaps — see the module docs for the
/// (deliberate) scope boundary with the fb-backend atlas work (P9).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BitmapFont {
    /// Cell width, in font design units (e.g. 8 for an 8×16 VGA cell).
    cell_width: f32,
    /// Cell height, in font design units (e.g. 16 for an 8×16 VGA cell).
    cell_height: f32,
    /// Baseline-to-top distance, in the same design units as `cell_height`.
    ascent_units: f32,
    /// Baseline-to-bottom distance, in the same design units as `cell_height`.
    descent_units: f32,
}

impl BitmapFont {
    /// Build a bitmap font from an explicit cell box, using the same 3:1
    /// (12:4-at-16) ascent:descent baseline split as [`Self::vga_8x16`],
    /// scaled to `cell_height`.
    ///
    /// `cell_width` and `cell_height` must be finite and positive; non-finite
    /// or non-positive inputs are clamped to `0.0` so every method on the
    /// resulting font stays total (finite, never panics) rather than
    /// producing NaN/infinite metrics.
    pub fn with_cell(cell_width: f32, cell_height: f32) -> Self {
        let cell_width = sanitize(cell_width);
        let cell_height = sanitize(cell_height);
        let ascent_units = cell_height * 0.75;
        let descent_units = cell_height - ascent_units;
        Self { cell_width, cell_height, ascent_units, descent_units }
    }

    /// The classic VGA text-mode bitmap font geometry: an 8×16 cell, baseline
    /// at design row 12 (12 rows ascent, 4 rows descent/underline).
    pub fn vga_8x16() -> Self {
        Self::with_cell(8.0, 16.0)
    }

    /// Scale factor from design units to pixels at `size_px`. Total: returns
    /// `0.0` (never NaN/infinite, never panics) for non-finite, zero, or
    /// negative `size_px`, or for a degenerate (zero-height) cell.
    fn scale(&self, size_px: f32) -> f32 {
        if size_px.is_finite() && size_px > 0.0 && self.cell_height > 0.0 {
            size_px / self.cell_height
        } else {
            0.0
        }
    }
}

fn sanitize(v: f32) -> f32 {
    if v.is_finite() && v > 0.0 {
        v
    } else {
        0.0
    }
}

impl Metrics for BitmapFont {
    fn ascent(&self, size_px: f32) -> f32 {
        self.ascent_units * self.scale(size_px)
    }

    fn descent(&self, size_px: f32) -> f32 {
        self.descent_units * self.scale(size_px)
    }

    fn line_height(&self, size_px: f32) -> f32 {
        self.cell_height * self.scale(size_px)
    }

    /// Monospace: every character — ASCII, Latin-1, CJK/emoji, control
    /// chars, unassigned scalars — gets the same cell-width advance. `ch` is
    /// deliberately never inspected, so this can never fail to "find" a
    /// glyph and is total over all of `char`.
    fn advance(&self, _ch: char, size_px: f32) -> f32 {
        self.cell_width * self.scale(size_px)
    }

    // `measure` keeps the trait default (sum of per-char advances via
    // `s.chars()`, i.e. by Unicode scalar value count, not byte count) —
    // for a monospace font that is already exactly `n_chars * advance`, so
    // overriding would just duplicate it.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advance_is_constant_across_ascii() {
        let f = BitmapFont::vga_8x16();
        let a = f.advance('A', 16.0);
        for ch in ['a', 'Z', '0', '9', ' ', '~', '!'] {
            assert_eq!(f.advance(ch, 16.0), a);
        }
    }

    #[test]
    fn advance_is_constant_across_latin1() {
        let f = BitmapFont::vga_8x16();
        let a = f.advance('A', 16.0);
        for ch in ['é', 'ñ', 'ü', 'ß', '£', '©'] {
            assert_eq!(f.advance(ch, 16.0), a);
        }
    }

    #[test]
    fn advance_is_constant_for_cjk_and_emoji() {
        let f = BitmapFont::vga_8x16();
        let a = f.advance('A', 16.0);
        for ch in ['日', '本', '語', '😀', '🦀'] {
            assert_eq!(f.advance(ch, 16.0), a);
        }
    }

    #[test]
    fn advance_is_constant_for_control_chars() {
        let f = BitmapFont::vga_8x16();
        let a = f.advance('A', 16.0);
        for ch in ['\u{0}', '\t', '\n', '\r', '\u{7f}', '\u{1b}'] {
            assert_eq!(f.advance(ch, 16.0), a);
        }
    }

    #[test]
    fn vertical_metrics_scale_linearly_with_size() {
        let f = BitmapFont::vga_8x16();
        let a16 = f.ascent(16.0);
        let d16 = f.descent(16.0);
        let lh16 = f.line_height(16.0);

        let a32 = f.ascent(32.0);
        let d32 = f.descent(32.0);
        let lh32 = f.line_height(32.0);

        assert_eq!(a32, a16 * 2.0);
        assert_eq!(d32, d16 * 2.0);
        assert_eq!(lh32, lh16 * 2.0);

        let a8 = f.ascent(8.0);
        assert_eq!(a8, a16 * 0.5);
    }

    #[test]
    fn ascent_descent_baseline_split_is_3_to_1() {
        let f = BitmapFont::vga_8x16();
        // At size_px == cell_height (16), design units map 1:1 to pixels.
        assert_eq!(f.ascent(16.0), 12.0);
        assert_eq!(f.descent(16.0), 4.0);
        assert_eq!(f.line_height(16.0), 16.0);
    }

    #[test]
    fn line_height_equals_size_px() {
        let f = BitmapFont::vga_8x16();
        for size in [1.0, 8.0, 16.0, 24.0, 32.0, 100.0] {
            assert_eq!(f.line_height(size), size);
        }
    }

    #[test]
    fn measure_abc_is_three_times_advance() {
        let f = BitmapFont::vga_8x16();
        let a = f.advance('a', 16.0);
        assert_eq!(f.measure("abc", 16.0), 3.0 * a);
    }

    #[test]
    fn measure_empty_string_is_zero() {
        let f = BitmapFont::vga_8x16();
        assert_eq!(f.measure("", 16.0), 0.0);
    }

    #[test]
    fn measure_counts_chars_not_bytes() {
        let f = BitmapFont::vga_8x16();
        let a = f.advance('a', 16.0);
        // "é日" is 2 Unicode scalars but 2+3 = 5 UTF-8 bytes.
        let s = "é日";
        assert_eq!(s.len(), 5);
        assert_eq!(s.chars().count(), 2);
        assert_eq!(f.measure(s, 16.0), 2.0 * a);
    }

    #[test]
    fn totality_on_unusual_scalars_no_panic() {
        let f = BitmapFont::vga_8x16();
        for ch in ['\u{0}', '\u{10FFFF}', '\u{D7FF}', '\u{E000}', char::REPLACEMENT_CHARACTER]
        {
            let adv = f.advance(ch, 16.0);
            assert!(adv.is_finite());
        }
    }

    #[test]
    fn totality_on_unusual_size_px_no_panic() {
        // Per-call metrics stay finite even at the extremes of f32's range:
        // each is at most a couple of multiplications away from `size_px`.
        let f = BitmapFont::vga_8x16();
        for size in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MIN, f32::MAX] {
            assert!(f.advance('A', size).is_finite());
            assert!(f.ascent(size).is_finite());
            assert!(f.descent(size).is_finite());
            assert!(f.line_height(size).is_finite());
        }

        // `measure` sums per-char advances (trait default): finite at any
        // sane size_px, including a size many orders above any real font
        // size. Not exercised at f32::MAX/MIN here — summing several
        // f32::MAX-scale advances legitimately overflows f32 range, which
        // is a property of floating-point summation in general, not a
        // BitmapFont defect (each individual advance above stays finite).
        for size in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.0e30] {
            assert!(f.measure("hello", size).is_finite());
        }
    }

    #[test]
    fn metrics_at_16_and_32px() {
        let f = BitmapFont::vga_8x16();
        assert_eq!(f.advance('A', 16.0), 8.0);
        assert_eq!(f.advance('A', 32.0), 16.0);
        assert_eq!(f.ascent(32.0), 24.0);
        assert_eq!(f.descent(32.0), 8.0);
    }

    #[test]
    fn with_cell_generalizes_the_3_to_1_split() {
        let f = BitmapFont::with_cell(6.0, 12.0);
        assert_eq!(f.ascent(12.0), 9.0);
        assert_eq!(f.descent(12.0), 3.0);
        assert_eq!(f.advance('x', 12.0), 6.0);
    }

    #[test]
    fn degenerate_cell_geometry_stays_finite() {
        // A tiny-but-positive cell_height (or width) must not slip past
        // sanitize()'s finite/positive check and blow scale() up to
        // infinity: scale(size_px) = size_px / cell_height, so a
        // near-zero cell_height alone is enough to overflow. Every
        // BitmapFont, however constructed, must keep the totality
        // guarantee its own doc comments promise.
        let tiny_height = BitmapFont::with_cell(6.0, 1e-38);
        let adv = tiny_height.advance('x', 16.0);
        assert!(adv.is_finite(), "advance was {adv}");
        assert!(adv > 0.0, "advance should still be a real, nonzero cell width");
        let m = tiny_height.measure("hi", 16.0);
        assert!(m.is_finite(), "measure was {m}");
        assert!(m > 0.0);

        let tiny_width = BitmapFont::with_cell(1e-38, 16.0);
        assert!(tiny_width.advance('x', 16.0).is_finite());
        assert!(tiny_width.ascent(16.0).is_finite());
        assert!(tiny_width.descent(16.0).is_finite());
        assert!(tiny_width.line_height(16.0).is_finite());

        let both_tiny = BitmapFont::with_cell(1e-40, 1e-40);
        assert!(both_tiny.advance('x', 16.0).is_finite());
        assert!(both_tiny.line_height(16.0).is_finite());

        let subnormal = BitmapFont::with_cell(f32::MIN_POSITIVE, f32::MIN_POSITIVE);
        assert!(subnormal.advance('x', 200.0).is_finite());
        assert!(subnormal.ascent(200.0).is_finite());
        assert!(subnormal.descent(200.0).is_finite());
        assert!(subnormal.line_height(200.0).is_finite());
    }
}
