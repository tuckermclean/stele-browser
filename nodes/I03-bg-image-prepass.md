# I03 — bg-image-prepass

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | S02, I01, I02 | none | S |

**Owns:** `src/bg_images.rs`

## Purpose

Imported from the code. `stele::bg_images` is the driver-level pre-pass that loads CSS background images for
the pixel render paths. `collect_bg_images(styles, base)` takes the cascaded `ComputedStyle`s (one per node,
as `style::cascade::cascade` returns them) and the document's base `Url`, and returns a map from the **raw**
`background_image` string, as `ComputedStyle` carries it, to the decoded first frame. `backend::raster`'s
paint looks a box's own `style.background_image` up in it; a missing entry means "background colour only".
The tty backend never calls it.

For every style with a `background_image`:

- A raw string already in the map is skipped.
- Otherwise it is resolved against `base`, and the fetch and decode work is cached by the **resolved** URL
  (failures included), so two raw strings naming the same resource load it once and share one `Rc`.
- An unseen resolved URL is fetched through `fetch::fetch` and decoded by `img::decode_bytes` with the
  response's `Content-Type` as the hint; the first frame is kept. A URL that does not load is left out.
- **Bounds**: at most `MAX_BG_IMAGES` (32) distinct resolved URLs are attempted, and the decoded pixel bytes
  are held to I02's `MAX_TOTAL_IMAGE_BYTES` (256 MiB) the same way I02 does: the first decode over the
  budget is dropped and no further unseen URL is attempted. `collect_bg_images_bounded`, private, takes the
  bounds as parameters for the tests.

## Tests

The inline module `bg_images::tests` in `src/bg_images.rs` (10 tests: absolute and relative URLs, no
background image, a missing file, broken bytes, an empty slice, a repeated URL, both bounds, and 500
unreachable URLs). Through the pipeline, `tests/bg_image_golden.rs` (`fixtures/bg-image.html` against
`goldens/bg-image.png`, the image-free render differing from it, and its one background image decoding).

## Changes

none: imported

## Gate

- **I03-AC-1** — A `background_image` that is an absolute `file://` URL of a PNG is fetched and decoded, keyed by that string. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 bg_images::tests::fetches_and_decodes_a_file_url_background_image` exits 0.
- **I03-AC-2** — A relative `background_image` is resolved against the base URL, and the map is keyed by the raw relative string. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 bg_images::tests::resolves_a_relative_url_against_the_document_base` exits 0.
- **I03-AC-3** — A style without `background_image`, and an empty slice, yield an empty map. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 bg_images::tests::no_background_image_declared_yields_no_entry_not_a_panic bg_images::tests::empty_styles_slice_yields_an_empty_map` exits 0.
- **I03-AC-4** — A URL whose file does not exist, or whose bytes are not an image, is left out of the map without a panic, and 500 distinct unreachable URLs yield an empty map. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 bg_images::tests::a_missing_file_is_skipped_not_a_panic bg_images::tests::malformed_image_bytes_are_skipped_not_a_panic bg_images::tests::hostile_many_distinct_bg_image_styles_do_not_panic_and_stay_bounded` exits 0.
- **I03-AC-5** — The same raw URL on more than `MAX_BG_IMAGES` styles gives one map entry. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 bg_images::tests::repeated_raw_url_decodes_once_and_is_shared_by_rc` exits 0.
- **I03-AC-6** — With a bound of 2 distinct URLs, only 2 of 5 distinct images are decoded. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 bg_images::tests::distinct_urls_beyond_max_images_are_skipped` exits 0.
- **I03-AC-7** — A decode that would take the decoded pixel bytes over the budget is skipped. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 bg_images::tests::distinct_urls_beyond_the_aggregate_byte_budget_are_skipped_and_cached_hits_still_work` exits 0.
- **I03-AC-8** — Two raw strings that resolve to the same URL load it once and share one `Rc`; after the first decode over the budget no further unseen URL is attempted, even one that would fit; a cached URL still resolves after that. *Verify:* unpinned (no test has two raw strings for one URL; the budget test's images are all the same size, and the test named `..._cached_hits_still_work` has no cached hit).
- **I03-AC-9** — The bounds are exactly 32 distinct URLs and I02's 256 MiB; the `Content-Type` is passed as the decode hint. *Verify:* unpinned.
- **I03-AC-10** — `fixtures/bg-image.html` declares one distinct background image, and it decodes. *Verify:* `cargo test --locked --jobs 2 --test bg_image_golden the_bg_image_actually_decodes_not_fallen_back_to_color_only -- --test-threads=2` exits 0.
- **I03-AC-11** — `fixtures/bg-image.html` renders to the pixels of `goldens/bg-image.png`. *Verify:* `cargo test --locked --jobs 2 --test bg_image_golden bg_image_fixture_png_matches_golden_pixels -- --test-threads=2` exits 0.

## Risks

- The map is keyed by the raw string while the work is cached by the resolved URL: two raw strings for the
  same resource get two map entries pointing at one `Rc`. The raster backend must look up exactly the string
  the cascade produced; any normalisation of `background_image` on one side and not the other loses the
  image.
- `fetch_and_decode`, `fetch_response` and `Budget` are copies of I02's, kept apart on purpose; a fix to one
  is not a fix to the other.
- The test `distinct_urls_beyond_the_aggregate_byte_budget_are_skipped_and_cached_hits_still_work` checks no
  cached hit, unlike its I02 twin; its name claims more than it asserts.
- The base URL is resolved twice for each unseen URL (once for the cache key, once for the fetch).
- Nothing is cached across calls: in `--x11` every resize re-runs this pre-pass and re-fetches every
  background image (issue #121).
