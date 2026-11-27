# S07 — author-sheets

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | D01, S01, S02, S03, S04, S05, S06 | S08 | S |

**Owns:** `src/style/mod.rs`, `src/style/author.rs`

## Purpose

Imported from the code. The `style` module's root and the step that reads a page's own `<style>` CSS out of the
DOM (D01) so the cascade (S06) can be fed it.

- **`src/style/mod.rs`** declares the public modules `author`, `cascade`, `computed`, `contrast` and `parser`
  and the crate-private `media`, `selector`, `tokenizer`, `ua` and `value`, and re-exports
  `collect_author_sheets`, `collect_author_sheets_for_viewport`, `media_attr_matches`, `parse_and_flatten`,
  `ComputedStyle`, `ColorScheme` and `Stylesheet`, so the rest of the crate and the integration tests write
  `stele::style::{self, cascade}` and `style::collect_author_sheets(&dom)`.
- **`collect_author_sheets(dom)`** walks the DOM in document order with an explicit heap stack (any depth is
  safe) and parses every `style` element's text (its direct text children, concatenated) into one
  `Stylesheet` (S05), in document order, so a later `<style>` wins ties. An empty or malformed block still
  gives a sheet, never a panic. It reads no `media` attribute of a `<style>`, and fetches nothing: `<link>` is
  S08's.
- **`collect_author_sheets_for_viewport(dom, width, scheme)`** is the same walk with each sheet's `@media`
  blocks flattened for the viewport width and colour scheme.
- **`parse_and_flatten(css, width, scheme)`** is `parse` then `flatten_media`, for a fetched sheet's text.
- **`media_attr_matches(attr, width, scheme)`** evaluates a `media="..."` attribute value as a media query;
  empty or unparseable fails closed (S08 treats an empty attribute as no gate before calling it).

## Tests

The inline module `style::author::tests` in `src/style/author.rs` (18 tests: no or empty DOM, one and several
`<style>` blocks, empty and malformed blocks, the viewport variant, an end-to-end media query, huge sheets
with a deep DOM, `parse_and_flatten` and `media_attr_matches`). `tests/author_css_golden.rs` and the other
golden tests feed `style::collect_author_sheets` into the cascade; the integration tests import the
re-exports.

## Changes

none: imported

## Gate

- **S07-AC-1** — A DOM with no `style` element yields no sheets. *Verify:* `cargo test --locked --jobs 2 --lib style::author::tests::no_style_element_yields_no_sheets -- --test-threads=2` exits 0.
- **S07-AC-2** — A `<style>` block's CSS is collected as one sheet, and its rule styles the page through the cascade. *Verify:* `cargo test --locked --jobs 2 --lib style::author::tests::single_style_block_is_collected_and_makes_p_red_via_cascade -- --test-threads=2` exits 0.
- **S07-AC-3** — Several `<style>` blocks are collected in document order, so the later one wins a tie. *Verify:* `cargo test --locked --jobs 2 --lib style::author::tests::multiple_style_blocks_apply_in_document_order -- --test-threads=2` exits 0.
- **S07-AC-4** — An empty `<style>` yields an empty sheet, and a malformed one yields a sheet, not a panic. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::author::tests::empty_style_block_yields_an_empty_stylesheet style::author::tests::malformed_style_block_yields_a_stylesheet_not_a_panic` exits 0.
- **S07-AC-5** — `collect_author_sheets_for_viewport` applies a `<style>`'s `@media` rule at a matching width and not at another. *Verify:* `cargo test --locked --jobs 2 --lib style::author::tests::end_to_end_media_query_produces_a_responsive_result -- --test-threads=2` exits 0.
- **S07-AC-6** — `parse_and_flatten` folds a matching `@media` block into the rules and drops one that does not match. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::author::tests::parse_and_flatten_folds_a_matching_media_block_into_rules style::author::tests::parse_and_flatten_drops_a_non_matching_media_block` exits 0.
- **S07-AC-7** — `media_attr_matches` evaluates the attribute against the given width: `(max-width: 500px)` matches at 320px and not at 640px, `screen and (min-width: 800px)` at 1024px and not at 320px. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 style::author::tests::media_attr_matches_narrow_query_at_narrow_viewport_only style::author::tests::media_attr_matches_screen_and_feature` exits 0.
- **S07-AC-8** — `ComputedStyle` is re-exported at the module root, as `stele::style::ComputedStyle`. *Verify:* `cargo test --locked --jobs 2 --test layout_block -- --test-threads=2` exits 0.

## Risks

- A `<style media="print">` block applies at every width: only `<link>` sheets are gated by their `media`
  attribute (S08), and only `@media` blocks inside a sheet are flattened.
- A `<style>` anywhere in the document counts, inside `<body>` or a `<noscript>` too; its position in the
  document sets its cascade order.
- Every `style` element's text is parsed again on each call: nothing is cached between calls.
