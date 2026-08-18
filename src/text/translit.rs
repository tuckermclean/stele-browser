// SPDX-License-Identifier: GPL-3.0-or-later

//! Transliteration fallback for characters the glyph atlas still can't
//! render after the Latin-1 extension (packet t2-glyph-fallback, closing
//! the D2 bug report's second half — see `text::glyphs`' own doc comment
//! for the atlas half).
//!
//! ## The bug
//!
//! httpforever.com and most real prose lean on non-ASCII PUNCTUATION — em
//! dash `—`, en dash `–`, curly quotes `‘ ’ “ ”`, ellipsis `…`, bullet `•`,
//! the multiplication sign `×`, a right arrow `→` — none of which live in
//! Latin-1 (`U+00A0..=U+00FF`, `text::glyphs`' newly-extended range); most
//! of this punctuation is General Punctuation (`U+2000..=U+206F`), a block
//! this project's compiled-in `font8x8`-derived atlas has never shipped
//! glyphs for and has no public-domain source to embed one from either. So
//! rather than paint the atlas's own hollow "tofu" box for every one of
//! these (`text::glyphs::FALLBACK_GLYPH`) — which is what a document with
//! this punctuation looked like before this packet — this module maps each
//! one to a plain-ASCII stand-in that the atlas CAN always render.
//!
//! ## Resolution order (pins the whole packet — read before touching either
//! backend's text path)
//!
//! For every `char` a `Text` fragment carries, in order:
//!
//! 1. **Atlas has a real glyph** (`text::glyphs::has_glyph`, ASCII or the
//!    Latin-1 supplement) → render `ch` verbatim. This covers the ORDINARY
//!    case (English prose) and, since packet t2-glyph-fallback's atlas
//!    extension, NBSP and `×` too — both are ALSO listed in
//!    [`TRANSLIT_TABLE`] below (the packet brief names them explicitly), but
//!    in practice this atlas check wins for both before transliteration is
//!    ever consulted; the table entries are a documented backstop (still
//!    independently contract-tested against [`transliterate`] directly), not
//!    dead weight — a future change that narrows the atlas's own coverage
//!    would fall through to them automatically.
//! 2. **No atlas glyph, but [`transliterate`] maps it** → render the ASCII
//!    replacement string instead (itself always atlas-renderable — every
//!    [`TRANSLIT_TABLE`] replacement is plain ASCII, contract-tested in this
//!    module's own tests).
//! 3. **Neither** → skip the character entirely (emit nothing — NOT the
//!    atlas's tofu box) and increment the process's missing-glyph counter
//!    (see "The `--stats` counter" below). CJK, emoji, and other genuinely
//!    unrepresentable scripts land here.
//!
//! [`resolve`] is the one function that walks all three steps over a whole
//! string; both backends call ONLY it (never `transliterate`/`glyphs::
//! has_glyph` directly from their own text paths) so fb and tty can never
//! silently drift apart on what a given document's punctuation renders as:
//!
//! - `backend::tty::write_marker` sanitizes each `Text`/`Image`-placeholder
//!   fragment's string through [`resolve`] before writing chars into grid
//!   cells. The tty backend has no glyph RASTERIZER of its own — a grid cell
//!   just stores a `char` — but the atlas is still the single source of
//!   truth for "can Stele actually represent this character," so a tty text
//!   dump and a PNG screenshot of the SAME document always agree on which
//!   characters survive and which get transliterated/dropped.
//! - `backend::raster::paint_text` sanitizes `text` through [`resolve`]
//!   before building the `surface::TextRun` handed to `Surface::draw_text` —
//!   by the time `MemSurface::draw_text` (`surface/mem.rs`) rasterizes glyph
//!   bitmaps, every character it sees is already atlas-renderable by
//!   construction, so that module needed NO changes at all for this packet.
//!
//! ## The `--stats` missing-glyph counter
//!
//! [`resolve`] increments a thread-local counter (reset via
//! [`reset_missing_glyph_count`], read via [`missing_glyph_count`]) once per
//! character that falls through to step 3 above. Thread-local, not a global
//! atomic: `cargo test` gives each `#[test]` its own OS thread by default,
//! so a thread-local counter is naturally isolated per test — no
//! inter-test races to guard against — while `main`'s single-threaded CLI
//! driver (`main.rs::count_missing_glyphs`) gets exactly the reset-then-read
//! pairing it needs around one render pass. `main.rs` wires this into
//! `--stats`'s existing STDERR line (see that module's own "count what we
//! refuse" section) — feeding a future Provenance pane, per the packet
//! brief.

use std::cell::Cell;

use super::glyphs;

/// One (source char, ASCII replacement) pair. `pub(crate)` visibility isn't
/// needed — nothing outside this module reads the table directly; every
/// caller goes through [`transliterate`] or [`resolve`].
type TranslitEntry = (char, &'static str);

/// The transliteration table (packet t2-glyph-fallback's brief, verbatim):
/// every entry maps ONE non-atlas-covered `char` to a plain-ASCII string.
/// Pure data — no logic here, just the mapping [`transliterate`] searches.
///
/// `\u{00A0}` (NBSP) and `\u{00D7}` (multiplication sign) are BOTH also now
/// real atlas glyphs (`text::glyphs`' Latin-1 extension) — see this module's
/// own "Resolution order" doc section for why they're still listed here
/// (a documented backstop, exercised directly by this table's own contract
/// tests even though the real render pipeline's atlas check normally wins
/// first).
const TRANSLIT_TABLE: &[TranslitEntry] = &[
    ('\u{2013}', "-"),    // – en dash
    ('\u{2014}', "-"),    // — em dash
    ('\u{2018}', "'"),    // ‘ left single quotation mark
    ('\u{2019}', "'"),    // ’ right single quotation mark
    ('\u{201C}', "\""),   // “ left double quotation mark
    ('\u{201D}', "\""),   // ” right double quotation mark
    ('\u{2026}', "..."),  // … horizontal ellipsis
    ('\u{2022}', "*"),    // • bullet
    ('\u{00A0}', " "),    // non-breaking space (documented backstop — see module doc)
    ('\u{00D7}', "x"),    // × multiplication sign (documented backstop — see module doc)
    ('\u{2192}', "->"),   // → rightwards arrow
];

/// Look up `ch`'s ASCII transliteration, or `None` if this table has no
/// mapping for it. Pure: no side effects, no counter, nothing but a table
/// scan — [`resolve`] is the (impure, counter-incrementing) orchestrator
/// built on top of this. Total over all of `char`: an unmapped scalar simply
/// returns `None` rather than panicking.
pub fn transliterate(ch: char) -> Option<&'static str> {
    TRANSLIT_TABLE.iter().find_map(|&(c, replacement)| if c == ch { Some(replacement) } else { None })
}

thread_local! {
    /// Process-wide (per-thread) count of characters [`resolve`] has had to
    /// SKIP (step 3 of the module's own resolution order — no atlas glyph,
    /// no transliteration mapping) since the last [`reset_missing_glyph_count`].
    /// See the module doc's "`--stats` missing-glyph counter" section for why
    /// this is thread-local rather than a global atomic.
    static MISSING_GLYPHS: Cell<u32> = Cell::new(0);
}

/// Zero this thread's missing-glyph counter — called once before a counted
/// render pass (`main.rs::count_missing_glyphs`), so an unrelated earlier
/// call (a previous document, a previous test) can never leak into the next
/// count.
pub fn reset_missing_glyph_count() {
    MISSING_GLYPHS.with(|c| c.set(0));
}

/// Read this thread's missing-glyph counter as of right now.
pub fn missing_glyph_count() -> u32 {
    MISSING_GLYPHS.with(|c| c.get())
}

/// `pub(self)`: only [`resolve`] ever increments the counter — reading and
/// resetting are the only externally-exposed operations, keeping the
/// counter a write-only instrumentation channel from the render path's own
/// point of view.
fn record_missing_glyph() {
    MISSING_GLYPHS.with(|c| c.set(c.get().saturating_add(1)));
}

/// Resolve `text` for rendering: walk every `char` through this module's own
/// documented resolution order (module doc, "Resolution order") and return
/// the sanitized, ALWAYS-atlas-renderable replacement string. Shared by
/// `backend::tty::write_marker` and `backend::raster::paint_text` — see the
/// module doc's "Resolution order" section for why calling this (rather than
/// `glyphs::has_glyph`/`transliterate` directly) is what keeps fb and tty in
/// agreement.
///
/// Total over any `&str` — every Unicode scalar (including 4-byte UTF-8
/// ones like emoji) is handled by the same three-step resolution order, and
/// an empty string returns an empty string. Never panics. `text.len()` is
/// used as the initial capacity estimate (most replacements are the same
/// length as or shorter than their source char's UTF-8 encoding), but this
/// is just a starting point, not a hard cap — `String::push`/`push_str`
/// grow past it for the rare longer case (e.g. `→`, 3 UTF-8 bytes, becomes
/// the 2-byte `"->"`, shorter; `…`, 3 bytes, becomes the 3-byte `"..."`,
/// equal — either direction is fine, `String` handles both).
pub fn resolve(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if glyphs::has_glyph(ch) {
            out.push(ch);
        } else if let Some(replacement) = transliterate(ch) {
            out.push_str(replacement);
        } else {
            record_missing_glyph();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------- transliterate (pure)

    #[test]
    fn en_dash_and_em_dash_map_to_a_hyphen() {
        assert_eq!(transliterate('\u{2013}'), Some("-"));
        assert_eq!(transliterate('\u{2014}'), Some("-"));
    }

    #[test]
    fn curly_single_quotes_map_to_a_straight_apostrophe() {
        assert_eq!(transliterate('\u{2018}'), Some("'"));
        assert_eq!(transliterate('\u{2019}'), Some("'"));
    }

    #[test]
    fn curly_double_quotes_map_to_a_straight_double_quote() {
        assert_eq!(transliterate('\u{201C}'), Some("\""));
        assert_eq!(transliterate('\u{201D}'), Some("\""));
    }

    #[test]
    fn ellipsis_maps_to_three_dots() {
        assert_eq!(transliterate('\u{2026}'), Some("..."));
    }

    #[test]
    fn bullet_maps_to_an_asterisk() {
        assert_eq!(transliterate('\u{2022}'), Some("*"));
    }

    #[test]
    fn nbsp_maps_to_an_ordinary_space() {
        assert_eq!(transliterate('\u{00A0}'), Some(" "));
    }

    #[test]
    fn multiplication_sign_maps_to_a_lowercase_x() {
        assert_eq!(transliterate('\u{00D7}'), Some("x"));
    }

    #[test]
    fn right_arrow_maps_to_ascii_arrow() {
        assert_eq!(transliterate('\u{2192}'), Some("->"));
    }

    #[test]
    fn unmapped_characters_return_none() {
        for ch in ['日', '本', '😀', '🦀', 'A', ' ', 'é', char::REPLACEMENT_CHARACTER] {
            assert_eq!(transliterate(ch), None, "{ch:?} should have no transliteration mapping");
        }
    }

    #[test]
    fn every_replacement_is_plain_ascii() {
        // Every entry's own replacement must itself be atlas-renderable
        // (module doc: "always atlas-renderable") -- plain ASCII is a
        // sufficient, easy-to-verify proxy for that.
        for &(ch, replacement) in TRANSLIT_TABLE {
            assert!(replacement.is_ascii(), "{ch:?}'s replacement {replacement:?} is not plain ASCII");
        }
    }

    // ---------------------------------------------------------- resolve

    #[test]
    fn resolve_passes_ascii_through_unchanged() {
        assert_eq!(resolve("hello, world!"), "hello, world!");
    }

    #[test]
    fn resolve_passes_latin1_supplement_through_unchanged() {
        assert_eq!(resolve("café"), "café");
        assert_eq!(resolve("naïve"), "naïve");
    }

    #[test]
    fn resolve_transliterates_em_dash_and_curly_quotes() {
        assert_eq!(resolve("a \u{2014} b"), "a - b");
        assert_eq!(resolve("\u{2018}quoted\u{2019}"), "'quoted'");
        assert_eq!(resolve("\u{201C}quoted\u{201D}"), "\"quoted\"");
    }

    #[test]
    fn resolve_transliterates_ellipsis_bullet_and_arrow() {
        assert_eq!(resolve("wait\u{2026}"), "wait...");
        assert_eq!(resolve("\u{2022} item"), "* item");
        assert_eq!(resolve("a \u{2192} b"), "a -> b");
    }

    #[test]
    fn resolve_skips_unmappable_characters_and_counts_them() {
        reset_missing_glyph_count();
        let out = resolve("emoji: \u{1F600} cjk: \u{65E5}\u{672C}");
        assert_eq!(out, "emoji:  cjk: ", "unmappable chars should be dropped, not tofu");
        assert_eq!(missing_glyph_count(), 3, "one emoji + two CJK chars = 3 missing glyphs");
    }

    #[test]
    fn resolve_never_emits_the_atlas_tofu_box_character() {
        // The atlas's own FALLBACK_GLYPH is a PIXEL pattern, not a `char` --
        // this test's real point is simpler: resolve() must never pass an
        // unmappable char through verbatim (which a naive "else push ch"
        // bug would do), since that's exactly the tofu-producing path this
        // packet closes.
        let out = resolve("\u{1F600}");
        assert!(out.is_empty(), "an unmappable char must be dropped, not passed through");
    }

    #[test]
    fn resolve_on_an_empty_string_is_a_total_no_op() {
        reset_missing_glyph_count();
        assert_eq!(resolve(""), "");
        assert_eq!(missing_glyph_count(), 0);
    }

    #[test]
    fn reset_missing_glyph_count_zeroes_the_counter() {
        reset_missing_glyph_count();
        let _ = resolve("\u{1F600}\u{1F601}");
        assert_eq!(missing_glyph_count(), 2);
        reset_missing_glyph_count();
        assert_eq!(missing_glyph_count(), 0, "reset must zero the counter");
    }

    #[test]
    fn missing_glyph_count_accumulates_across_multiple_resolve_calls() {
        reset_missing_glyph_count();
        let _ = resolve("\u{1F600}");
        let _ = resolve("\u{1F601}\u{1F602}");
        assert_eq!(missing_glyph_count(), 3, "counts should accumulate until the next reset");
    }

    #[test]
    fn resolve_does_not_count_transliterated_or_atlas_covered_characters() {
        reset_missing_glyph_count();
        let _ = resolve("café \u{2014} na\u{00EF}ve \u{2026}");
        assert_eq!(missing_glyph_count(), 0, "atlas hits and transliteration hits must not count as missing");
    }
}
