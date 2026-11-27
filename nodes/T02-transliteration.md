# T02 — transliteration

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | none | T03 | S |

**Owns:** `src/text/translit.rs`

## Purpose

Imported from the code. `stele::text::translit` decides what each character of a text run becomes before
it is drawn, so that the tty and pixel backends agree. `backend::tty::write_marker` and
`backend::raster::paint_text` both pass their text through `resolve`; `main.rs`'s `--stats` reads its
counter.

`resolve(text)` walks every `char` in order:

1. **The embedded font has a glyph** (`has_embedded_glyph`: `0x20..=0x7E` or `0xA0..=0xFF`, the range T01
   embeds, by its own range check): the `char` is kept.
2. **Otherwise, `transliterate` maps it**: the ASCII replacement is written instead. The table
   (`TRANSLIT_TABLE`): en and em dash to `-`, curly single quotes to `'`, curly double quotes to `"`,
   ellipsis to `...`, bullet to `*`, rightwards arrow to `->`, and NBSP to a space and `×` to `x` (these two
   are inside step 1's range, so in `resolve` step 1 wins; they are kept as a backstop).
3. **Otherwise** the `char` is dropped (no fallback box is drawn) and a thread-local counter is
   incremented. Control characters (`0x00..=0x1F`), DEL, CJK and emoji land here.

`reset_missing_glyph_count` zeroes the counter and `missing_glyph_count` reads it; `main.rs` resets it
before a counted render pass and reports it as `--stats`'s "missing glyphs" field. The counter is per
thread, so parallel tests do not share it.

## Tests

The inline module `text::translit::tests` in `src/text/translit.rs` (23 tests: each mapping, unmapped
characters, ASCII-only replacements, the coverage check, control characters, and `resolve` with its
counter). `tests/stats_cli.rs` checks the count through the binary's `--stats`, and `tests/entities_golden.rs`
checks a transliterated em dash in the tty dump.

## Changes

none: imported

## Gate

- **T02-AC-1** — `transliterate` maps en and em dash to `-`, curly single quotes to `'`, curly double quotes to `"`, the ellipsis to `...`, the bullet to `*` and the rightwards arrow to `->`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 text::translit::tests::en_dash_and_em_dash_map_to_a_hyphen text::translit::tests::curly_single_quotes_map_to_a_straight_apostrophe text::translit::tests::curly_double_quotes_map_to_a_straight_double_quote text::translit::tests::ellipsis_maps_to_three_dots text::translit::tests::bullet_maps_to_an_asterisk text::translit::tests::right_arrow_maps_to_ascii_arrow` exits 0.
- **T02-AC-2** — `transliterate` maps NBSP to a space and `×` to `x`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 text::translit::tests::nbsp_maps_to_an_ordinary_space text::translit::tests::multiplication_sign_maps_to_a_lowercase_x` exits 0.
- **T02-AC-3** — `transliterate` returns `None` for a character the table does not map (CJK, emoji, `A`, a space, `é`, U+FFFD). *Verify:* `cargo test --locked --jobs 2 --lib text::translit::tests::unmapped_characters_return_none -- --test-threads=2` exits 0.
- **T02-AC-4** — Every replacement in the table is plain ASCII. *Verify:* `cargo test --locked --jobs 2 --lib text::translit::tests::every_replacement_is_plain_ascii -- --test-threads=2` exits 0.
- **T02-AC-5** — The embedded-glyph check covers every `char` of `0x20..=0x7E` and `0xA0..=0xFF`. *Verify:* `cargo test --locked --jobs 2 --lib text::translit::tests::has_embedded_glyph_covers_printable_ascii_and_latin1 -- --test-threads=2` exits 0.
- **T02-AC-6** — Control characters and DEL are outside the embedded range and unmapped: `resolve` drops them and counts each one. *Verify:* `cargo test --locked --jobs 2 --lib text::translit::tests::control_chars_and_del_are_outside_embedded_coverage_and_fall_through_to_step_3 -- --test-threads=2` exits 0.
- **T02-AC-7** — `resolve` keeps ASCII and Latin-1 text unchanged. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 text::translit::tests::resolve_passes_ascii_through_unchanged text::translit::tests::resolve_passes_latin1_supplement_through_unchanged` exits 0.
- **T02-AC-8** — `resolve` replaces a mapped character with its transliteration (em dash, curly quotes, ellipsis, bullet, arrow). *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 text::translit::tests::resolve_transliterates_em_dash_and_curly_quotes text::translit::tests::resolve_transliterates_ellipsis_bullet_and_arrow` exits 0.
- **T02-AC-9** — `resolve` drops a character that is neither covered nor mapped instead of passing it through. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 text::translit::tests::resolve_never_emits_the_atlas_tofu_box_character text::translit::tests::resolve_skips_unmappable_characters_and_counts_them` exits 0.
- **T02-AC-10** — `resolve` counts each dropped character once. *Verify:* `cargo test --locked --jobs 2 --lib text::translit::tests::resolve_skips_unmappable_characters_and_counts_them -- --test-threads=2` exits 0.
- **T02-AC-11** — `resolve` of an empty string returns an empty string and counts nothing. *Verify:* `cargo test --locked --jobs 2 --lib text::translit::tests::resolve_on_an_empty_string_is_a_total_no_op -- --test-threads=2` exits 0.
- **T02-AC-12** — `reset_missing_glyph_count` sets the counter to 0. *Verify:* `cargo test --locked --jobs 2 --lib text::translit::tests::reset_missing_glyph_count_zeroes_the_counter -- --test-threads=2` exits 0.
- **T02-AC-13** — The counter accumulates across `resolve` calls until it is reset. *Verify:* `cargo test --locked --jobs 2 --lib text::translit::tests::missing_glyph_count_accumulates_across_multiple_resolve_calls -- --test-threads=2` exits 0.
- **T02-AC-14** — A kept or transliterated character is not counted. *Verify:* `cargo test --locked --jobs 2 --lib text::translit::tests::resolve_does_not_count_transliterated_or_atlas_covered_characters -- --test-threads=2` exits 0.
- **T02-AC-15** — `stele --headless --dump-text --stats` on a page with one emoji and two CJK characters reports `3 missing glyphs`. *Verify:* `cargo test --locked --jobs 2 --test stats_cli stats_flag_counts_missing_glyphs_for_unmappable_characters -- --test-threads=2` exits 0.
- **T02-AC-16** — The tty dump shows `&mdash;` as `-` and never the raw em dash. *Verify:* `cargo test --locked --jobs 2 --test entities_golden named_symbol_entities_decode_to_their_real_unicode_characters -- --test-threads=2` exits 0.
- **T02-AC-17** — The counter is per thread: a `resolve` on one thread does not change another thread's count. *Verify:* unpinned.

## Risks

- `has_embedded_glyph` repeats T01's range by hand instead of asking the table. If the embedded subset
  changes, this check and the table can disagree: a covered `char` would be dropped, or an uncovered one
  drawn as the fallback box.
- Dropping a character changes the text silently except for the `--stats` count: CJK or emoji prose
  renders with gaps.
- The table is small and fixed; any other punctuation outside Latin-1 (other dashes, quotes, arrows) is
  dropped. Adding a mapping changes what Stele renders, so it is a dialect question (SPEC.md principle 6).
- NBSP is kept by step 1, so it is not turned into an ordinary space by `resolve` (SPEC.md lists `&nbsp;`
  as a non-collapsing space among the fidelity nits).
