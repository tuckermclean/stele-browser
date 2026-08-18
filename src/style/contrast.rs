//! WCAG-inspired contrast repair (packet T1c): the "contrast covenant" —
//! Stele has no way to guarantee an arbitrary author-declared foreground/
//! background color PAIR stays legible (no gradient renderer — see
//! `style::value`'s gradient -> representative-solid fallback, this
//! packet's OTHER half — and a `var()` substitution, packet T1a, can hand a
//! property a value this engine can't fully honor), so rather than trust
//! the page, every text run's foreground color is checked against its
//! EFFECTIVE background (`backend::raster::effective_background`) and
//! REPAIRED to black or white — whichever actually reads — when it falls
//! below a floor ratio.
//!
//! Pure `Color` arithmetic only (no `Fragment`/layout knowledge here — see
//! `backend::raster::effective_background` for the fragment-slice-walking
//! half of this packet, which is NOT pure the same way this module is);
//! every function here is total over any two 8-bit RGBA colors.

use crate::surface::Color;

/// The contrast-repair floor. Real WCAG 2.x defines TWO normal-text floors
/// -- AA (4.5:1) and the stricter AAA (7:1) -- but neither is what this
/// packet enforces: `CONTRAST_MIN` is a deliberately LOOSER "is this even
/// readable at all" backstop (close to WCAG's own "graphical objects and
/// UI components" 3:1 floor, matching the brief's own "WCAG-ish" framing),
/// picked because this is a defense-of-last-resort repair over pages this
/// engine can't fully honor (a dropped gradient, an unhandled `var()`
/// chain, ...), not a full accessibility auditor -- demanding full AA
/// compliance would repair (and thus visibly recolor) far more author-
/// intended text than this packet's actual bug (fully invisible text)
/// requires fixing. A future packet can raise this if a stricter floor is
/// ever wanted.
pub const CONTRAST_MIN: f32 = 3.0;

/// One sRGB 8-bit channel -> its WCAG-linearized `[0, 1]` contribution, per
/// the WCAG 2.x relative luminance formula.
fn linearize_channel(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.03928 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG 2.x relative luminance of `color`, ignoring alpha -- `repair_fg`/
/// `contrast_ratio` are only ever asked about the OPAQUE colors two boxes
/// paint over each other with (`backend::raster::effective_background`
/// only ever returns a `background_color` whose `a != 0`, or the surface's
/// own always-opaque canvas fill). `L = 0.2126 R + 0.7152 G + 0.0722 B`
/// over the linearized channels.
pub fn relative_luminance(color: Color) -> f32 {
    0.2126 * linearize_channel(color.r) + 0.7152 * linearize_channel(color.g) + 0.0722 * linearize_channel(color.b)
}

/// WCAG 2.x contrast ratio between two colors: `(L_lighter + 0.05) /
/// (L_darker + 0.05)`, always `>= 1.0` regardless of argument order (the
/// brighter color is picked out by comparing luminance, not by which
/// argument came first) -- black vs. white is the canonical 21:1 maximum.
pub fn contrast_ratio(a: Color, b: Color) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// The RUN-level rung of the contrast covenant (packet T1c; the BOX-level
/// rung is `style::value`'s gradient -> representative-solid fallback,
/// which repairs what actually gets PAINTED as a background before this
/// function ever sees it -- see that module's own doc comment): if `fg`
/// already clears `CONTRAST_MIN` against `effective_bg`, return it
/// unchanged (the overwhelmingly common case -- every existing black-on-
/// white fixture takes this path, byte-for-byte unchanged renders).
/// Otherwise return whichever of `Color::BLACK`/`Color::WHITE` reads
/// better against `effective_bg` -- never a third color, so this can't
/// introduce some new, potentially-also-illegible hue.
///
/// This never fails to repair down to something legible: for ANY
/// `effective_bg`, at least one of black/white clears `CONTRAST_MIN` --
/// the worst case is a background whose luminance sits at black's and
/// white's crossover point (`contrast_ratio(BLACK, bg) == contrast_ratio
/// (WHITE, bg)`), which still clears roughly 4.6:1, comfortably above this
/// module's 3.0 floor (`repair_always_clears_the_floor_across_a_spread_
/// of_gray_backgrounds` sweeps this by hand). So a REPAIRED color that
/// still fails the floor is a bug in this function or in `backend::
/// raster::effective_background`'s own resolution, never a legitimately
/// unrepairable input -- `main.rs`'s `--audit-contrast` is exactly that
/// regression gate, checking this invariant against real rendered pages.
pub fn repair_fg(fg: Color, effective_bg: Color) -> Color {
    if contrast_ratio(fg, effective_bg) >= CONTRAST_MIN {
        return fg;
    }
    if contrast_ratio(Color::BLACK, effective_bg) >= contrast_ratio(Color::WHITE, effective_bg) {
        Color::BLACK
    } else {
        Color::WHITE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_on_white_repairs_to_black() {
        assert_eq!(repair_fg(Color::WHITE, Color::WHITE), Color::BLACK);
    }

    #[test]
    fn black_on_black_repairs_to_white() {
        assert_eq!(repair_fg(Color::BLACK, Color::BLACK), Color::WHITE);
    }

    #[test]
    fn black_on_white_is_unchanged() {
        assert_eq!(repair_fg(Color::BLACK, Color::WHITE), Color::BLACK);
    }

    #[test]
    fn a_compliant_foreground_is_returned_unchanged_even_if_not_black_or_white() {
        // red (#ff0000) on white clears CONTRAST_MIN (~4.0:1) -- must pass
        // through untouched, not get quantized to black/white regardless.
        let red = Color::rgb(255, 0, 0);
        assert_eq!(repair_fg(red, Color::WHITE), red);
    }

    #[test]
    fn mid_gray_background_picks_black_over_white() {
        // Gray-on-gray fails CONTRAST_MIN (ratio 1.0) -- repaired to
        // whichever of black/white contrasts better against THIS
        // background. Verified by hand (WCAG relative-luminance formula):
        // black-vs-#808080 is ~5.32:1, white-vs-#808080 is ~3.95:1, so
        // black wins.
        let gray = Color::rgb(128, 128, 128);
        assert_eq!(repair_fg(gray, gray), Color::BLACK);
    }

    #[test]
    fn black_white_contrast_ratio_is_the_canonical_21_to_1() {
        assert!((contrast_ratio(Color::BLACK, Color::WHITE) - 21.0).abs() < 0.01);
    }

    #[test]
    fn contrast_ratio_is_order_independent() {
        let a = Color::rgb(30, 60, 90);
        let b = Color::rgb(200, 210, 220);
        assert_eq!(contrast_ratio(a, b), contrast_ratio(b, a));
    }

    #[test]
    fn contrast_ratio_of_a_color_with_itself_is_1() {
        let c = Color::rgb(77, 88, 99);
        assert!((contrast_ratio(c, c) - 1.0).abs() < 0.0001);
    }

    #[test]
    fn relative_luminance_of_black_is_zero_and_white_is_one() {
        assert_eq!(relative_luminance(Color::BLACK), 0.0);
        assert!((relative_luminance(Color::WHITE) - 1.0).abs() < 0.0001);
    }

    #[test]
    fn repair_always_clears_the_floor_across_a_spread_of_gray_backgrounds() {
        // Defense-in-depth sanity sweep (mirrors --audit-contrast's own
        // invariant, main.rs): repair_fg must never hand back a color that
        // itself fails CONTRAST_MIN against the background it was repaired
        // for, for ANY background -- swept across the full gray ramp.
        for v in 0..=255u8 {
            let bg = Color::rgb(v, v, v);
            // A foreground that's ALWAYS going to be too close to `bg` to
            // clear the floor on its own (differs by only 1 of 255 levels),
            // forcing `repair_fg` to actually engage its black/white choice
            // on every iteration.
            let fg = Color::rgb(v.wrapping_add(1), v, v);
            let repaired = repair_fg(fg, bg);
            assert!(
                contrast_ratio(repaired, bg) >= CONTRAST_MIN - 0.001,
                "repair_fg output failed the floor against gray background {v}"
            );
        }
    }
}
