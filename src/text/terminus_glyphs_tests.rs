//! Hand-verified correctness tests for the GENERATED
//! [`super::terminus_glyphs`] module (packet/terminus-font, Task 1). Kept as
//! a SIBLING file, not `#[cfg(test)] mod` inside `terminus_glyphs.rs` itself,
//! because that file is regenerated wholesale by `tools/gen-terminus-glyphs.py`
//! — hand-written tests living there would be silently discarded on the next
//! regeneration. `src/text/mod.rs` wires this file in via `#[path = ...]`.
//!
//! The expected byte values below are derived BY HAND from the raw BDF
//! source (`ter-u16n.bdf`/`ter-u16b.bdf`/`ter-u32n.bdf`'s own `'A'`
//! `BITMAP` blocks, MSB-first, as read directly and cited in the design doc,
//! `docs/superpowers/specs/2026-08-21-terminus-font-design.md`'s "Current
//! state" section) and bit-reversed by hand (or by an independent throwaway
//! script, not `tools/gen-terminus-glyphs.py` itself) into this project's
//! LSB-leftmost convention — this test module's whole point is to catch a
//! bit-order regression in the GENERATOR, so it must not share the
//! generator's own reversal code path.
//!
//! `'A'` happens to be left-right symmetric, so its bit-reversed rows equal
//! the ORIGINAL (un-reversed) BDF bytes for every row here — a real property
//! of this specific glyph, not a mistake; it does NOT independently prove
//! the reversal direction is correct (a mirrored implementation would
//! produce the exact same result for a symmetric glyph). That direction is
//! what the packet's separate ASCII-art visual check (rendering `'e'`,
//! `'g'`, `'Q'` — all left-right ASYMMETRIC) actually verifies; see the
//! packet's implementation report for that rendering.

use super::terminus_glyphs::{self, GlyphRows};
use crate::style::computed::FontWeight;

/// `ter-u16n.bdf`'s `'A'`: `00 00 3C 42 42 42 42 7E 42 42 42 42 00 00 00 00`
/// (MSB-first). Bit-reversing each byte by hand: `0x3C` (`0011 1100`) is a
/// palindrome under bit reversal (reverses to itself), as are `0x42`
/// (`0100 0010`) and `0x7E` (`0111 1110`) — so the expected LSB-leftmost
/// rows are numerically identical to the raw BDF bytes.
#[test]
fn a_at_16px_normal_matches_the_hand_reversed_bdf_bytes() {
    let expected: [u8; 16] =
        [0x00, 0x00, 0x3C, 0x42, 0x42, 0x42, 0x42, 0x7E, 0x42, 0x42, 0x42, 0x42, 0x00, 0x00, 0x00, 0x00];
    let g = terminus_glyphs::lookup(16.0, FontWeight::Normal, 'A');
    assert_eq!(g.cell_w, 8);
    assert_eq!(g.cell_h, 16);
    assert_eq!(g.rows, GlyphRows::Narrow(&expected));
}

/// `ter-u16b.bdf`'s `'A'`: `00 00 7C C6 C6 C6 C6 FE C6 C6 C6 C6 00 00 00 00`
/// (MSB-first, distinct bold bytes). Bit-reversing each byte by hand:
/// `0x7C` (`0111 1100`) -> `0x3E` (`0011 1110`); `0xC6` (`1100 0110`) ->
/// `0x63` (`0110 0011`); `0xFE` (`1111 1110`) -> `0x7F` (`0111 1111`).
#[test]
fn a_at_16px_bold_matches_the_hand_reversed_bdf_bytes() {
    let expected: [u8; 16] =
        [0x00, 0x00, 0x3E, 0x63, 0x63, 0x63, 0x63, 0x7F, 0x63, 0x63, 0x63, 0x63, 0x00, 0x00, 0x00, 0x00];
    let g = terminus_glyphs::lookup(16.0, FontWeight::Bold, 'A');
    assert_eq!(g.cell_w, 8);
    assert_eq!(g.cell_h, 16);
    assert_eq!(g.rows, GlyphRows::Narrow(&expected));

    // Bold really is different data from normal at the same char/size (not
    // an accidental alias) -- distinguishes this test from the (coincidentally
    // palindromic) normal-weight case above.
    let normal = terminus_glyphs::lookup(16.0, FontWeight::Normal, 'A');
    assert_ne!(g.rows, normal.rows, "bold 'A' must differ from normal 'A' at the same size");
}

/// `ter-u32n.bdf`'s `'A'` (16-wide, 2-byte/`u16`-row bucket): raw MSB-first
/// rows `0000,0000,0000,0000,0000,0000,0FF0,1FF8,381C,300C,300C,300C,300C,
/// 300C,300C,300C,3FFC,3FFC,300C,300C,300C,300C,300C,300C,300C,300C,0000,
/// 0000,0000,0000,0000,0000`. Every one of these 16-bit values is ALSO a
/// palindrome under full bit reversal (this glyph is left-right symmetric at
/// every row, at every size) -- hand-verified for the first three non-blank
/// rows (`0x0FF0`, `0x1FF8`, `0x381C`) by writing out all 16 bits and
/// confirming the reversed sequence equals the original. This is the
/// packet's proof that the MIXED-WIDTH (`u16`-row) packing round-trips
/// correctly, independent of the bit-order question `'e'`/`'g'`/`'Q'`
/// settle separately.
#[test]
fn a_at_32px_normal_is_the_wide_u16_row_bucket_and_matches_hand_reversed_bytes() {
    let expected: [u16; 32] = [
        0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0FF0, 0x1FF8, 0x381C, 0x300C, 0x300C, 0x300C, 0x300C,
        0x300C, 0x300C, 0x300C, 0x3FFC, 0x3FFC, 0x300C, 0x300C, 0x300C, 0x300C, 0x300C, 0x300C, 0x300C, 0x300C,
        0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    ];
    let g = terminus_glyphs::lookup(32.0, FontWeight::Normal, 'A');
    assert_eq!(g.cell_w, 16);
    assert_eq!(g.cell_h, 32);
    assert_eq!(g.rows, GlyphRows::Wide(&expected));
}

/// Total over all of `char`: a scalar well outside both embedded ranges
/// (`0x20..=0x7E`, `0xA0..=0xFF`) must not panic, and must resolve to the
/// documented fallback box rather than silently vanishing or aliasing a real
/// glyph.
#[test]
fn glyph_outside_the_subset_returns_the_fallback_box_not_a_panic() {
    for ch in ['日', '😀', char::REPLACEMENT_CHARACTER, '\u{10FFFF}', '\u{0}', '\u{1F}', '\u{7F}'] {
        let g = terminus_glyphs::lookup(16.0, FontWeight::Normal, ch); // must not panic
        assert_eq!(g.cell_w, 8);
        assert_eq!(g.cell_h, 16);
        // The 16px hollow-box fallback, by construction (see this test
        // module's doc comment / the generator's `fallback_rows`): row 0 and
        // the last row blank, rows 1 and 14 a full horizontal bar (0x7E),
        // rows 2..=13 just the two side columns (0x42).
        let mut expected = [0u8; 16];
        expected[1] = 0x7E;
        expected[14] = 0x7E;
        for row in expected.iter_mut().take(14).skip(2) {
            *row = 0x42;
        }
        assert_eq!(g.rows, GlyphRows::Narrow(&expected), "unexpected fallback shape for {ch:?}");
    }
}

/// Same fallback-shape check at a WIDE bucket (12px, `u8`-row still since
/// cell_w=6 <= 8 -- Narrow) to confirm the fallback generator scales its box
/// inset correctly across bucket sizes, not just the one pinned above.
#[test]
fn fallback_box_at_the_12px_bucket_has_the_expected_inset_shape() {
    let g = terminus_glyphs::lookup(12.0, FontWeight::Normal, '日');
    assert_eq!(g.cell_w, 6);
    assert_eq!(g.cell_h, 12);
    let mut expected = [0u8; 12];
    expected[1] = 0x1E; // bits 1..=4 set (cell_w - 2 = 4)
    expected[10] = 0x1E;
    for row in expected.iter_mut().take(10).skip(2) {
        *row = 0x12; // bit1 | bit4
    }
    assert_eq!(g.rows, GlyphRows::Narrow(&expected));
}

/// Structural completeness: every one of the 191 (ASCII + Latin-1) glyphs is
/// present, at every one of the 5 sizes and both weights (`191 * 5 * 2 =
/// 1,910` real table entries) -- catches an off-by-one in the extraction
/// range before it ships silently. "Present" here means: the table array has
/// the right length, AND every printable, non-blank-by-design glyph lights
/// at least one pixel (a blank entry would mean the extractor silently
/// dropped/zeroed a real glyph).
#[test]
fn every_table_entry_is_present_and_non_degenerate() {
    for size_table in terminus_glyphs::TABLES.iter() {
        for weight_table in size_table.iter() {
            assert_eq!(weight_table.ascii.len(), 95, "ASCII table must cover 0x20..=0x7E");
            assert_eq!(weight_table.latin1.len(), 96, "Latin-1 table must cover 0xA0..=0xFF");
        }
    }

    for weight in [FontWeight::Normal, FontWeight::Bold] {
        for size in terminus_glyphs::SIZES {
            // 0x20 (space) and 0xA0 (NBSP) are legitimately blank glyphs --
            // skip those two codepoints, require every other printable
            // ASCII/Latin-1 char to light at least one pixel.
            for code in (0x21u32..=0x7E).chain(0xA1u32..=0xFF) {
                let ch = char::from_u32(code).unwrap();
                let g = terminus_glyphs::lookup(size, weight, ch);
                let any_lit = match g.rows {
                    GlyphRows::Narrow(rows) => rows.iter().any(|&r| r != 0),
                    GlyphRows::Wide(rows) => rows.iter().any(|&r| r != 0),
                };
                assert!(any_lit, "size={size} weight={weight:?} char={ch:?} (U+{code:04X}) has a blank glyph");
            }
        }
    }
}

/// Totality on `size_px`: any input (including values with no matching
/// bucket, non-finite, or wildly out of range) must resolve to SOME bucket
/// and never panic. `terminus_glyphs::lookup`'s own `bucket_index` is a
/// defensive nearest-match, not the canonical snap rule (that's
/// `text::terminus::nearest_terminus_size`, tested exhaustively in Task 2) --
/// this only pins that this lower layer can never be made to panic or return
/// a nonsensical cell size regardless of what's handed to it.
#[test]
fn lookup_is_total_over_unusual_size_px() {
    const VALID_CELL_H: [u8; 5] = [12, 16, 20, 24, 32];
    for size in [0.0, -1.0, 1000.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 18.5] {
        let g = terminus_glyphs::lookup(size, FontWeight::Normal, 'A'); // must not panic
        assert!(VALID_CELL_H.contains(&g.cell_h), "unexpected cell_h {} for size_px={size}", g.cell_h);
        assert!(g.cell_w > 0 && g.cell_w <= 16, "unexpected cell_w {} for size_px={size}", g.cell_w);
    }
}
