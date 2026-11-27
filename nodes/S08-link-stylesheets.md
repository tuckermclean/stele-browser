# S08 — link-stylesheets

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | D01, S05, S07 | none | M |

**Owns:** `src/stylesheets.rs`

## Purpose

Imported from the code. `stele::stylesheets` is the driver-level pre-pass that gathers every author sheet of a
page, `<style>` blocks and fetched `<link rel="stylesheet">` sheets alike, in document order. It lives in the
library crate, outside `style`, because it does I/O: `main.rs`'s render paths and `frames.rs` call it.

- **`collect_all_author_sheets(dom, base, width, scheme) -> Vec<Stylesheet>`** walks the DOM (D01) once, in
  document order, with an explicit heap stack (any depth is safe). A `style` element's text and a fetched
  `<link>` sheet each go through `style::parse_and_flatten` (S07), so they reach the cascade the same way and a
  later one wins a tie, whichever kind it is.
- **Which `<link>`s.** One whose `rel` holds the token `stylesheet` in any case, among others too
  (`alternate stylesheet` counts, a simplification); no `rel`, or `icon`, `preload` and the like, is never
  fetched. A non-empty `media` attribute that does not match the viewport (`style::media_attr_matches`, S07)
  skips the whole sheet before any fetch; an absent or empty one matches.
- **The fetch.** The `href`, trimmed, is resolved against the document's `base` URL and fetched through
  `crate::fetch::fetch`, whichever schemes it supports. A missing or empty `href`, a fetch error, an
  unsupported scheme, or a response whose `Content-Type` is present and does not contain `css` skips that one
  sheet; the rest of the page's CSS still applies. The body is decoded as UTF-8, lossily. `@import` inside a
  fetched sheet is not followed (S05 skips it as an at-rule).
- **The bound.** `MAX_LINKS` (32) caps the fetch attempts of one call; a `<link>` that the `rel` or `media`
  gate skips costs nothing, and `<style>` blocks are never capped.

## Tests

The inline module `stylesheets::tests` in `src/stylesheets.rs` (20 tests, fetching temporary `file://` CSS
files: order between `<link>` and `<style>`, relative `href`, `media`, `rel`, missing and bad `href`s, the
cap, `@import`, a deep DOM). `tests/frames_golden.rs` and the golden tests reach it through the render paths.

## Changes

none: imported

## Gate

- **S08-AC-1** — A `<link rel="stylesheet">` to a CSS file is fetched and its rules style the page. *Verify:* `cargo test --locked --jobs 2 --lib stylesheets::tests::external_link_stylesheet_applies_through_a_real_file_fetch -- --test-threads=2` exits 0.
- **S08-AC-2** — A relative `href` resolves against the document's base URL. *Verify:* `cargo test --locked --jobs 2 --lib stylesheets::tests::external_link_href_resolves_relative_to_the_document_base -- --test-threads=2` exits 0.
- **S08-AC-3** — `<link>` and `<style>` sheets keep document order: a later `<style>` beats an earlier `<link>`, and a later `<link>` an earlier `<style>`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 stylesheets::tests::a_later_style_block_overrides_an_earlier_link_stylesheet stylesheets::tests::a_later_link_stylesheet_overrides_an_earlier_style_block` exits 0.
- **S08-AC-4** — A `<link media>` that does not match the viewport skips the whole sheet, unfetched; at a matching width it applies. *Verify:* `cargo test --locked --jobs 2 --lib stylesheets::tests::link_media_attribute_gates_the_whole_sheet_by_viewport -- --test-threads=2` exits 0.
- **S08-AC-5** — A `<link>` with no `media` attribute applies at every width. *Verify:* `cargo test --locked --jobs 2 --lib stylesheets::tests::link_with_no_media_attribute_always_applies -- --test-threads=2` exits 0.
- **S08-AC-6** — `rel` matches `stylesheet` in any case and among other tokens (`Alternate STYLESHEET`). *Verify:* `cargo test --locked --jobs 2 --lib stylesheets::tests::link_rel_stylesheet_matches_case_insensitively_and_among_multiple_tokens -- --test-threads=2` exits 0.
- **S08-AC-7** — A `<link rel="icon">` is never applied as a stylesheet. *Verify:* `cargo test --locked --jobs 2 --lib stylesheets::tests::link_rel_icon_is_never_fetched_as_a_stylesheet -- --test-threads=2` exits 0.
- **S08-AC-8** — A `<link>` with no `rel` is never fetched. *Verify:* `cargo test --locked --jobs 2 --lib stylesheets::tests::link_rel_missing_is_never_fetched -- --test-threads=2` exits 0.
- **S08-AC-9** — A `<link>` whose fetch fails (a missing file, an unsupported scheme) is skipped, and the page's other CSS still applies. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 stylesheets::tests::a_missing_href_file_is_skipped_not_a_panic stylesheets::tests::an_unsupported_scheme_href_is_skipped_not_a_panic` exits 0.
- **S08-AC-10** — A `<link>` with an empty `href` yields no sheet. *Verify:* `cargo test --locked --jobs 2 --lib stylesheets::tests::an_empty_href_is_skipped_not_a_panic -- --test-threads=2` exits 0.
- **S08-AC-11** — A `<link>` with no `href` yields no sheet. *Verify:* `cargo test --locked --jobs 2 --lib stylesheets::tests::a_link_with_no_href_at_all_is_skipped_not_a_panic -- --test-threads=2` exits 0.
- **S08-AC-12** — At most `MAX_LINKS` stylesheet links are fetched in one call. *Verify:* `cargo test --locked --jobs 2 --lib stylesheets::tests::max_links_caps_the_number_of_link_fetches -- --test-threads=2` exits 0.
- **S08-AC-13** — The link cap does not block `<style>` blocks. *Verify:* `cargo test --locked --jobs 2 --lib stylesheets::tests::max_links_does_not_block_style_blocks -- --test-threads=2` exits 0.
- **S08-AC-14** — A response whose `Content-Type` is present and does not contain `css` is skipped. *Verify:* unpinned.
- **S08-AC-15** — An empty `media` attribute matches like an absent one. *Verify:* unpinned.
- **S08-AC-16** — A body that is not UTF-8 is decoded lossily rather than skipped. *Verify:* unpinned.

## Risks

- `alternate stylesheet` links apply like ordinary ones: a page that offers alternate themes gets all of them
  folded together.
- Each `<link>` is fetched again on every call, with no cache: an `--x11` resize re-fetches every sheet (issue #121
  asks for a session cache).
- The `Content-Type` gate is a substring test (`css`) and needs a header: a `file://` sheet, or a server that
  sends none, is parsed whatever its body is (the parse is total, so this is safe but can apply junk).
- `MAX_LINKS` counts attempts, not successes: 32 failing links use up the budget before a working one.
- A fetched sheet's `url()`s keep their raw text, and the background-image stage resolves them against the
  document's base URL, not the sheet's: a sheet in another directory points its relative images at the wrong
  place.
