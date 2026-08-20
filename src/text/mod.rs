//! Text metrics: the seam between the font layer and the inline engine.
//!
//! P5 (Wave 1) implements [`Metrics`] with a monospace [`bitmap::BitmapFont`]
//! model (std-only, i486-safe; the brief's bitmap-font lean over a TTF).
//! Originally metrics-only — glyph rasterization / the atlas were deferred to
//! the fb-backend packet (M4), where pixels are actually consumed; that
//! packet has now landed [`glyphs`], a compiled-in public-domain 8x8 bitmap
//! atlas, looked up via [`BitmapFont::glyph`]. Shaping-free: Latin-1/UTF-8
//! advance widths, no complex-script shaping — the inline engine (P6) breaks
//! lines given these advances. v0 is monospace (matches tty cells);
//! double-width scripts and proportional fonts are later refinements.
//!
//! [`translit`] (packet t2-glyph-fallback) is the shared fb/tty seam that
//! maps a character the atlas still can't render (after its Latin-1
//! extension — see `glyphs`' own doc comment) to an ASCII stand-in, or
//! drops it with a counted, documented default — see that module's own doc
//! comment for the full resolution order.

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

/// packet/text-min-8x8: the floor every caller must apply to `font_size`
/// BEFORE feeding it to a [`Metrics`] method (or `BitmapFont::glyph_scale`)
/// to actually RASTERIZE/measure text — never `16.0`-clamps `font_size`
/// itself.
///
/// Why: `glyphs` is a compiled-in, native **8×8** bitmap atlas, but it is
/// only ever rasterized through [`BitmapFont::vga_8x16`]'s 8-wide/**16**-
/// tall cell (`glyph_scale(size_px) = size_px / 16.0`). So the native 8×8
/// atlas only paints at its true resolution when `size_px == 16.0` (scale
/// `1.0`); anything smaller downscales the already-tiny 8×8 source below
/// its own native size — the "muddy sub-16px text" bug this floor exists
/// to close. Flooring the RENDER size at `16.0` guarantees every glyph is
/// drawn at native resolution or upscaled, never downscaled, while
/// `font8x8` remains the only font Stele has.
///
/// Scope, sharply: this floors ONLY the pixel size fed to glyph
/// rasterization, advances, and line-height for actual TEXT runs
/// (`layout::inline`'s [`Metrics`] call sites, and the `TextRun::size_px`
/// `backend::raster::paint_text` builds). It must NEVER be applied to
/// `ComputedStyle::font_size` itself, nor to anything `em`/`%`-based that
/// resolves against it (`style::cascade::resolve_font_size`, `raw_to_px`,
/// and everything built on `raw_to_px` — box `width`/`height`/`margin`/
/// `padding`/`border`/grid tracks/…): those describe BOX geometry, which
/// must keep tracking the document's real, unfloored `font-size` (e.g. CSS1
/// Acid1's `dt { width: 10.638% /* of 47em */ }`, `dd { width: 34em }` —
/// flooring the `font_size` those resolve against would blow up the whole
/// box layout, not just crisp up some glyphs).
///
/// Revisit-trigger: once a second, non-`font8x8` font lands (scalable, or
/// just a different fixed-cell atlas), a hardcoded "never below 16px" floor
/// stops being universally correct — this function (and every call site
/// that routes through it) should be reconsidered at that point, not kept
/// as a permanent fixture.
///
/// Total: any non-finite `font_size` (NaN, ±infinity) floors to `16.0`
/// rather than propagating; any finite value — including negative/zero —
/// is clamped up to at least `16.0` via [`f32::max`].
pub fn text_render_px(font_size: f32) -> f32 {
    if font_size.is_finite() {
        font_size.max(16.0)
    } else {
        16.0
    }
}

#[cfg(test)]
mod text_render_px_tests {
    use super::text_render_px;

    #[test]
    fn floors_sub_16px_sizes_to_16() {
        for size in [0.0, 1.0, 8.0, 10.0, 15.999] {
            assert_eq!(text_render_px(size), 16.0, "size_px={size}");
        }
    }

    #[test]
    fn leaves_16px_and_above_untouched() {
        for size in [16.0, 16.001, 20.0, 24.0, 32.0, 100.0] {
            assert_eq!(text_render_px(size), size, "size_px={size}");
        }
    }

    #[test]
    fn non_finite_and_negative_inputs_stay_total() {
        for size in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, -100.0] {
            let out = text_render_px(size);
            assert!(out.is_finite(), "text_render_px({size}) = {out} was not finite");
            assert_eq!(out, 16.0, "size_px={size}");
        }
    }
}
