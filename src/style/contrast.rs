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
