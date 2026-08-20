//! Text metrics: the seam between the font layer and the inline engine.
//!
//! P5 (Wave 1) implemented [`Metrics`] with a monospace [`bitmap::BitmapFont`]
//! model (std-only, i486-safe; the brief's bitmap-font lean over a TTF), and
//! M4 added the compiled-in [`glyphs`] font8x8 atlas as its glyph source.
//! packet/terminus-font retired both: [`terminus::TerminusFont`] is now the
//! sole `Metrics` implementer and glyph source everywhere Stele renders text
//! (tty, fb/raster, chrome, `--dump-png`) — a real, embedded, 191-glyph
//! subset of Terminus Font (OFL-1.1), 5 pixel sizes x 2 weights, generated
//! from upstream BDF sources by `tools/gen-terminus-glyphs.py` into
//! [`terminus_glyphs`]. Shaping-free: Latin-1/UTF-8 advance widths, no
//! complex-script shaping — the inline engine (P6) breaks lines given these
//! advances. Still monospace (matches tty cells); proportional fonts remain
//! a later refinement.
//!
//! [`translit`] (packet t2-glyph-fallback) is the shared fb/tty seam that
//! maps a character the embedded subset still can't render to an ASCII
//! stand-in, or drops it with a counted, documented default — see that
//! module's own doc comment for the full resolution order.

pub mod bitmap;
pub(crate) mod glyphs;
pub mod terminus;
pub(crate) mod terminus_glyphs;
#[cfg(test)]
#[path = "terminus_glyphs_tests.rs"]
mod terminus_glyphs_tests;
pub mod translit;

pub use bitmap::BitmapFont;
pub use terminus::TerminusFont;

/// Per-font, size-parameterized metrics. All returns are in pixels at `size_px`.
pub trait Metrics {
    /// Distance from baseline up to the top of typical glyphs.
    fn ascent(&self, size_px: f32) -> f32;

    /// Distance from baseline down to the bottom of typical glyphs (positive).
    fn descent(&self, size_px: f32) -> f32;

    /// Default line-to-line advance (baseline to baseline).
    fn line_height(&self, size_px: f32) -> f32;

    /// Horizontal advance of a single character.
    fn advance(&self, ch: char, size_px: f32) -> f32;

    /// Advance of a whole string — sum of per-char advances by default; a
    /// backend with kerning may override.
    fn measure(&self, s: &str, size_px: f32) -> f32 {
        s.chars().map(|c| self.advance(c, size_px)).sum()
    }
}

/// packet/terminus-font, Task 4: the snap every caller must apply to
/// `font_size` BEFORE feeding it to a [`Metrics`] method (or
/// `TerminusFont::glyph`) to actually RASTERIZE/measure text — never
/// touches `font_size` itself.
///
/// This function's own doc comment (packet/text-min-8x8, its original
/// author) predicted exactly this moment: *"once a second, non-`font8x8`
/// font lands (scalable, or just a different fixed-cell atlas), a hardcoded
/// 'never below 16px' floor stops being universally correct — this function
/// ... should be reconsidered at that point, not kept as a permanent
/// fixture."* That trigger has now fired. The body below no longer floors
/// at a single hardcoded `16.0`; it delegates to
/// [`terminus::nearest_terminus_size`] (the SAME canonical snap
/// [`TerminusFont`]'s own `Metrics` impl uses internally), so this function
/// and `TerminusFont` can never disagree on which of the 5 embedded buckets
/// (`12.0, 16.0, 20.0, 24.0, 32.0`) a given `size_px` resolves to.
///
/// Scope, sharply (unchanged from the original floor): this snaps ONLY the
/// pixel size fed to glyph rasterization, advances, and line-height for
/// actual TEXT runs (`layout::inline`'s [`Metrics`] call sites, and the
/// `TextRun::size_px` `backend::raster::paint_text` builds). It must NEVER
/// be applied to `ComputedStyle::font_size` itself, nor to anything
/// `em`/`%`-based that resolves against it (`style::cascade::
/// resolve_font_size`, `raw_to_px`, and everything built on `raw_to_px` —
/// box `width`/`height`/`margin`/`padding`/`border`/grid tracks/…): those
/// describe BOX geometry, which must keep tracking the document's real,
/// un-snapped `font-size` (e.g. CSS1 Acid1's `dt { width: 10.638% /* of
/// 47em */ }`, `dd { width: 34em }` — snapping the `font_size` those
/// resolve against would blow up the whole box layout, not just change some
/// glyph shapes).
///
/// Deliberate contract change from the original flat `16.0` floor (stated
/// explicitly, not an oversight): any `font_size` below the 12/16 midpoint
/// (`14.0`) now snaps DOWN to the 12px bucket instead of UP to 16px — e.g.
/// the UA stylesheet's `h6`/`h5` (`≈10.72px`/`≈13.28px` off a 16px parent)
/// now render at the 12px bucket, visibly smaller than before. Symmetrically,
/// any `font_size` above 32px now clamps DOWN to the 32px bucket instead of
/// scaling up arbitrarily. See the design doc
/// (`docs/superpowers/specs/2026-08-21-terminus-font-design.md` §4) for the
/// full blast-radius accounting.
///
/// Total: any non-finite `font_size` (NaN, ±infinity) degrades to the 16px
/// bucket (same as before); every finite value — including negative/zero —
/// resolves to one of the 5 embedded buckets via [`terminus::nearest_terminus_size`].
pub fn text_render_px(font_size: f32) -> f32 {
    terminus::nearest_terminus_size(font_size)
}

#[cfg(test)]
mod text_render_px_tests {
    use super::text_render_px;

    /// packet/terminus-font: REPLACES the old flat-16px-floor tests
    /// (`floors_sub_16px_sizes_to_16`, `leaves_16px_and_above_untouched`,
    /// `non_finite_and_negative_inputs_stay_total`) -- those asserted the
    /// OLD contract and are now actively WRONG under the nearest-of-5 snap,
    /// not merely out of date. This is the function's own long-standing
    /// "revisit-trigger" doc comment firing, not an oversight (see
    /// `text_render_px`'s doc comment above). The assertions below are the
    /// same list `text::terminus::nearest_terminus_size`'s own test module
    /// pins, ported to this public wrapper to prove the two agree on every
    /// input.
    #[test]
    fn exact_bucket_values_map_to_themselves() {
        for size in [12.0, 16.0, 20.0, 24.0, 32.0] {
            assert_eq!(text_render_px(size), size, "size_px={size}");
        }
    }

    #[test]
    fn exact_midpoints_round_up_to_the_larger_bucket() {
        assert_eq!(text_render_px(14.0), 16.0);
        assert_eq!(text_render_px(18.0), 20.0);
        assert_eq!(text_render_px(22.0), 24.0);
        assert_eq!(text_render_px(28.0), 32.0);
    }

    #[test]
    fn interior_values_snap_to_the_nearer_bucket() {
        assert_eq!(text_render_px(13.0), 12.0);
        assert_eq!(text_render_px(15.0), 16.0);
        assert_eq!(text_render_px(19.0), 20.0);
        assert_eq!(text_render_px(30.0), 32.0);
    }

    #[test]
    fn below_min_and_above_max_clamp_to_the_nearest_bucket() {
        assert_eq!(text_render_px(0.0), 12.0);
        assert_eq!(text_render_px(11.9), 12.0);
        assert_eq!(text_render_px(32.1), 32.0);
        assert_eq!(text_render_px(1000.0), 32.0);
    }

    #[test]
    fn non_finite_and_negative_inputs_stay_total() {
        for size in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let out = text_render_px(size);
            assert!(out.is_finite(), "text_render_px({size}) = {out} was not finite");
            assert_eq!(out, 16.0, "size_px={size}");
        }
        for size in [-1.0, -100.0] {
            let out = text_render_px(size);
            assert!(out.is_finite(), "text_render_px({size}) = {out} was not finite");
            assert_eq!(out, 12.0, "size_px={size}, negative sizes clamp to the smallest bucket");
        }
    }
}
