//! Text metrics: the seam between the font layer and the inline engine.
//!
//! P5 (Wave 1) implements [`Metrics`] two ways and keeps the winner: a fontdue
//! glue path and an embedded bitmap-atlas path (likely the better fit on a 486).
//! Shaping-free: Latin-1/UTF-8 advance widths, no complex-script shaping — the
//! inline engine (P6) breaks lines given these advances.

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
