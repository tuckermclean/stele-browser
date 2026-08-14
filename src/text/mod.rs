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

pub mod bitmap;
pub(crate) mod glyphs;

pub use bitmap::BitmapFont;

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
