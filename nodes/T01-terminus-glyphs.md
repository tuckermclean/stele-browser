# T01 — terminus-glyphs

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | S02 | T03 | M |

**Owns:** `src/text/terminus_glyphs.rs`, `src/text/terminus_glyphs_tests.rs`, `tools/gen-terminus-glyphs.py`, `third_party/`

## Purpose

Imported from the code. The embedded font: a 191-glyph subset of Terminus Font 4.49.1 (printable ASCII
`0x20..=0x7E` and the Latin-1 supplement `0xA0..=0xFF`) at five pixel sizes (12, 16, 20, 24, 32) and two
weights (normal, bold), compiled into the binary as `const` tables. It is the only font Stele renders with.

- `tools/gen-terminus-glyphs.py` reads the ten upstream BDF files (`ter-u{12,16,20,24,32}{n,b}.bdf`) from a
  `--bdf-dir` and writes `src/text/terminus_glyphs.rs` (`--out`). Its doc comment pins the release tarball's
  URL and SHA-256; the BDF sources are not committed. It reverses each BDF row's bits (BDF is MSB-first)
  into the project's convention, bit 0 = leftmost pixel, and with `--strict` (the default) aborts if a
  glyph's bounding box is not the font's bounding box or a required glyph is missing. It also emits a
  hollow-box fallback glyph per size (`fallback_rows`: rows 1 and `cell_h - 2` a bar over columns
  `1..=cell_w-2`, the rows between only columns 1 and `cell_w - 2`).
- `src/text/terminus_glyphs.rs` is that generated output (header "GENERATED FILE -- DO NOT EDIT BY HAND"):
  `SIZES`, `GlyphRows` (`Narrow` `u8` rows for the 12 and 16px cells, at most 8 wide; `Wide` `u16` rows for
  20, 24 and 32px), `SizeWeightTable` (`cell_w`, `cell_h`, 95 ASCII and 96 Latin-1 entries), `TABLES`
  indexed by size and weight, `FALLBACKS`, and `lookup(size_px, weight, ch)`: the nearest bucket by
  absolute difference (16px for a non-finite size), the glyph for a covered `ch`, the bucket's fallback box
  for any other `char`. Cells: 6x12, 8x16, 10x20, 12x24, 16x32.
- `src/text/terminus_glyphs_tests.rs` holds the hand-checked tests of the generated table, kept outside it
  so a regeneration does not discard them; `src/text/mod.rs` (T03) includes it with `#[path]`.
- `third_party/terminus-font/` holds `OFL.TXT` (the SIL Open Font License 1.1, verbatim from upstream) and
  `NOTICE.md` (the attribution: version, extraction tool, and why the subset is a Modified Version that
  does not use the Reserved Font Name). `src/fetch/about.rs` embeds `OFL.TXT` with `include_str!` into
  `about:attestations`, so the license travels in the binary.

The canonical size snap is `text::terminus::nearest_terminus_size` (T03); `lookup`'s own nearest match is a
fallback that keeps it total.

## Tests

The sibling module `text::terminus_glyphs_tests` in `src/text/terminus_glyphs_tests.rs` (7 tests: `'A'`
at 16px normal and bold and at 32px against hand-reversed BDF bytes, the fallback box at 16 and 12px,
table completeness, and totality over `size_px`). `fetch::about::tests::attestations_page_embeds_ofl_verbatim`
in `src/fetch/about.rs` checks that `OFL.TXT` reaches the attestations page. Nothing runs the generator:
the BDF sources are not in the repository.

## Changes

none: imported

## Gate

- **T01-AC-1** — At 16px normal weight, `lookup` returns `'A'` in an 8x16 cell with the `u8` rows of `ter-u16n.bdf`'s `'A'`, and at 32px normal in a 16x32 cell with the `u16` rows of `ter-u32n.bdf`'s `'A'`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 text::terminus_glyphs_tests::a_at_16px_normal_matches_the_hand_reversed_bdf_bytes text::terminus_glyphs_tests::a_at_32px_normal_is_the_wide_u16_row_bucket_and_matches_hand_reversed_bytes` exits 0.
- **T01-AC-2** — At 16px bold, `lookup` returns `'A'` with the bit-reversed rows of `ter-u16b.bdf`'s `'A'` (`0x7C` stored as `0x3E`, `0xC6` as `0x63`, `0xFE` as `0x7F`), which differ from the normal weight's. *Verify:* `cargo test --locked --jobs 2 --lib text::terminus_glyphs_tests::a_at_16px_bold_matches_the_hand_reversed_bdf_bytes -- --test-threads=2` exits 0.
- **T01-AC-3** — At 16px, a `char` outside the subset (CJK, an emoji, U+FFFD, U+10FFFF, U+0000, U+001F, U+007F) gets the hollow fallback box in an 8x16 cell: rows 0 and 15 blank, rows 1 and 14 `0x7E`, rows 2 to 13 `0x42`. *Verify:* `cargo test --locked --jobs 2 --lib text::terminus_glyphs_tests::glyph_outside_the_subset_returns_the_fallback_box_not_a_panic -- --test-threads=2` exits 0.
- **T01-AC-4** — At 12px, the fallback box is a 6x12 cell with rows 1 and 10 `0x1E` and rows 2 to 9 `0x12`. *Verify:* `cargo test --locked --jobs 2 --lib text::terminus_glyphs_tests::fallback_box_at_the_12px_bucket_has_the_expected_inset_shape -- --test-threads=2` exits 0.
- **T01-AC-5** — Every size and weight has 95 ASCII and 96 Latin-1 entries, and every glyph but the space and NBSP lights at least one pixel. *Verify:* `cargo test --locked --jobs 2 --lib text::terminus_glyphs_tests::every_table_entry_is_present_and_non_degenerate -- --test-threads=2` exits 0.
- **T01-AC-6** — `lookup` is total over `size_px`: for 0, -1, 1000, NaN, ±infinity and 18.5 it returns one of the five cell heights and a cell width from 1 to 16, without a panic. *Verify:* `cargo test --locked --jobs 2 --lib text::terminus_glyphs_tests::lookup_is_total_over_unusual_size_px -- --test-threads=2` exits 0.
- **T01-AC-7** — The Terminus OFL-1.1 text in `third_party/terminus-font/OFL.TXT` is embedded in the `about:attestations` page. *Verify:* `cargo test --locked --jobs 2 --lib fetch::about::tests::attestations_page_embeds_ofl_verbatim -- --test-threads=2` exits 0.
- **T01-AC-8** — `tools/gen-terminus-glyphs.py` reproduces `src/text/terminus_glyphs.rs` from the pinned 4.49.1 BDF sources, and with `--strict` aborts on a glyph whose bounding box differs from the font's or on a missing required glyph. *Verify:* unpinned (nothing runs the generator; the BDF sources are not committed).
- **T01-AC-9** — The fallback box of the 20, 24 and 32px buckets has the same 1-pixel-inset hollow shape. *Verify:* unpinned.

## Risks

- `src/text/terminus_glyphs.rs` is generated: a hand edit is lost the next time the generator runs, and a
  change of extraction logic goes in the generator. Nothing checks that the committed table is what the
  generator makes from the pinned sources.
- The bold test is the only one whose bytes change under bit reversal (`'A'` normal is left-right
  symmetric at both sizes tested). A mirrored reversal in a regeneration would be caught there and by the
  goldens, not by the normal-weight tests.
- The embedded table is a large share of the binary's size against the floppy; a wider subset (more
  sizes, more ranges) costs directly in the `stele-i486` size.
- Changing the font version means updating the pinned URL and SHA-256 in the generator's doc comment and
  in the generated header, and the version in `NOTICE.md`.
- `OFL.TXT` must stay verbatim (OFL condition 2); `about:attestations` shows it.
