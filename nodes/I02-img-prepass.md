# I02 — img-prepass

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | D01, I01 | I03 | S |

**Owns:** `src/images.rs`

## Purpose

Imported from the code. `stele::images` is the driver-level pre-pass that loads the images of `<img>` and
`<object>` elements before layout. It does real I/O, so it sits outside `img` (I01, pure decoding) and
outside layout, which only consumes decoded images. The pixel render paths in `main.rs` call
`collect_images(dom, base)` and hand its `NodeId -> Rc<RgbaImage>` map to `layout::box_tree::build_box_tree`;
the tty dump passes an empty map instead.

`collect_images` walks the DOM from its root:

- An `img` element's `src` attribute, or an `object` element's `data` attribute, is resolved against the
  document's base `Url`, fetched through `fetch::fetch` (a GET), and decoded by `img::decode_bytes` with the
  response's `Content-Type` as the hint. The first frame's image is kept (an animated GIF renders its first
  frame). A node whose image does not load (fetch error, unsupported scheme, unrecognised or broken bytes,
  no frame) is left out of the map; layout then draws the placeholder box, or for `object` its fallback
  children.
- **Dedup**: results are cached by the resolved URL string, failures included, so a URL is fetched and
  decoded once per call and every element that names it shares the same `Rc`.
- **Bounds**: at most `MAX_IMAGES` (256) distinct URLs are attempted. A running total of decoded pixel bytes
  is kept against `MAX_TOTAL_IMAGE_BYTES` (256 MiB); the first decode that would exceed it is dropped and no
  further unseen URL is attempted, while cached URLs still resolve. The walk stops descending at depth
  `DEPTH_CAP` (100). `collect_images_bounded`, private, takes the two bounds as parameters for the tests.
- An empty `Dom` yields an empty map.

`MAX_TOTAL_IMAGE_BYTES` is public and shared with I03.

## Tests

The inline module `images::tests` in `src/images.rs` (10 tests: absolute and relative `src`, a missing file,
broken bytes, elements without an image, an empty document, dedup, both bounds and deep nesting; they write
small PNGs to the temp directory with `backend::raster::encode_png`). Through the pipeline:
`tests/images_golden.rs` (`fixtures/images.html` against `goldens/images.png`, and its six `<img>`s all
decoding), `tests/local_img_loading.rs` (relative and absolute `file://` `src` in a temp directory), and
`accept.sh`'s `A5q`, `A5r` and `A5s` (`fixtures/object-*.html` against `goldens/object-*.png`).

## Changes

none: imported

## Gate

- **I02-AC-1** — An `<img>` with an absolute `file://` `src` of a PNG is fetched and decoded, and its `NodeId` maps to the decoded pixels. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 images::tests::resolves_fetches_and_decodes_an_img_src` exits 0.
- **I02-AC-2** — A relative `src` is resolved against the document's base URL. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 images::tests::resolves_a_relative_src_against_the_document_base` exits 0.
- **I02-AC-3** — An `<img>` whose file does not exist, or whose bytes are not an image, is left out of the map without a panic. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 images::tests::a_missing_file_is_skipped_not_a_panic images::tests::malformed_image_bytes_are_skipped_not_a_panic` exits 0.
- **I02-AC-4** — Elements other than `img` and `object`, and an `<img>` without `src`, add no entry: `<div><span>hello</span><img></div>`, and a new `Dom` holding only its root, yield an empty map. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 images::tests::non_img_elements_and_missing_src_never_produce_entries images::tests::empty_document_yields_an_empty_map` exits 0.
- **I02-AC-5** — The same `src` on more than `MAX_IMAGES` elements resolves for every one of them, all sharing one decoded `Rc`. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 images::tests::repeated_src_decodes_once_and_is_shared_across_every_referencing_img` exits 0.
- **I02-AC-6** — With a bound of 2 distinct URLs, only 2 of 5 distinct images are decoded. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 images::tests::distinct_images_beyond_max_images_are_skipped` exits 0.
- **I02-AC-7** — A decode that would take the decoded pixel bytes over the budget is skipped, while a later element naming an already-cached URL still gets the cached `Rc`. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 images::tests::distinct_images_beyond_the_aggregate_byte_budget_are_skipped_and_cached_hits_still_work` exits 0.
- **I02-AC-8** — A document nested 3,000 elements deep returns without aborting. *Verify:* `cargo test --locked --jobs 2 --lib -- --exact --test-threads=2 images::tests::deeply_nested_dom_does_not_abort_the_process` exits 0.
- **I02-AC-9** — The walk stops descending at depth 100, and the bounds are exactly 256 distinct URLs and 256 MiB of decoded pixels. *Verify:* unpinned (the 3,000-deep test also passes without the depth cap).
- **I02-AC-10** — All six `<img>`s of `fixtures/images.html` (PNG, JPEG, GIF, animated GIF, floated and inline) decode. *Verify:* `cargo test --locked --jobs 2 --test images_golden all_images_actually_decode_not_fallen_back_to_placeholders -- --test-threads=2` exits 0.
- **I02-AC-11** — An animated GIF is drawn with its first frame: `fixtures/images.html` renders to the pixels of `goldens/images.png`. *Verify:* `cargo test --locked --jobs 2 --test images_golden images_fixture_png_matches_golden_pixels -- --test-threads=2` exits 0.
- **I02-AC-12** — An `<img>` without size attributes in an arbitrary directory, with a relative or an absolute `file://` `src`, decodes and paints. *Verify:* `cargo test --locked --jobs 2 --test local_img_loading -- --test-threads=2 relative_img_src_with_no_size_attrs_decodes_and_paints_at_an_arbitrary_directory absolute_file_url_img_src_with_no_size_attrs_decodes_and_paints` exits 0.
- **I02-AC-13** — An `<object>`'s `data` is loaded like an `<img>`'s `src`: `fixtures/object-image.html`, whose `data` is a `data:` PNG, renders to `goldens/object-image.png`, and `fixtures/object-nested.html` to `goldens/object-nested.png`. *Verify:* `CARGO_BUILD_JOBS=2 ./accept.sh --tty-only` prints `PASS A5q` and `PASS A5s`.
- **I02-AC-14** — The response's `Content-Type` is passed to `decode_bytes` as the hint, and a failed load is cached so it is not retried in the same call. *Verify:* unpinned.
- **I02-AC-15** — After the first decode over the byte budget, no further unseen URL is attempted, even one that would fit; an empty `Dom` returns before the walk. *Verify:* unpinned (the budget test's images are all the same size, and `Dom::new()` is not empty).

## Risks

- `fetch_and_decode` decodes every frame of an animated GIF and keeps only the first; the others are built
  and dropped, and their size does not count against the byte budget.
- The byte budget counts decoded pixels only after a decode, so a single image up to the I01 pixel cap
  (about 244 MiB) is decoded in full before it is weighed.
- `fetch_and_decode` and `fetch_response` are copied, on purpose, in I03 (and a similar helper in
  `frames.rs`); a change to one is not a change to the others.
- `DEPTH_CAP` repeats the depth caps of `layout::box_tree`, `layout::block` and `dom_util` by hand: an image
  deeper than 100 levels is never loaded, even if layout would draw its box.
- The pre-pass keeps only frame 0 and has no cache across calls; issue #119 (animated GIFs in `--x11`) needs
  the frames and a cache kept by the session.
