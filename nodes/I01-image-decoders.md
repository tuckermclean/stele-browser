# I01 — image-decoders

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | none | I02, I03, L01, L02, L04, L05 | M |

**Owns:** `src/img/`

## Purpose

Imported from the code. `stele::img` is the pure image-decoding layer: bytes in, straight-alpha RGBA8 frames
out, with no network or file access. The pre-passes that fetch images (I02 for `<img>` and `<object>`, I03
for `background-image`) call its dispatcher `decode_bytes`; layout and the raster backend only ever see its
`RgbaImage`.

- **The contract** (`img/mod.rs`): `RgbaImage` (`width`, `height`, `width * height * 4` row-major RGBA
  bytes), `Frame` (an image and its `delay_ms`, 0 for a still), the `Decode` trait (`decode(bytes)` returns
  every frame), and `DecodeError`: `NotThisFormat` (the bytes are not this decoder's format), `Malformed`
  (they are, but broken) and `Unsupported` (a known feature is not implemented; the caller falls back to
  alt text).
- **The pixel cap** (`check_pixel_cap`, crate-private): every decoder checks the declared size before it
  allocates pixels. A zero width or height is `Malformed`; more than `MAX_DECODE_PIXELS` (64,000,000) pixels
  is `Unsupported`.
- **The dispatcher**: `sniff_format` recognises GIF (`GIF87a`/`GIF89a`), JPEG (`FF D8 FF`) and PNG (its
  8-byte signature) by magic number. `decode_bytes(bytes, content_type)` first tries the decoder named by a
  `Content-Type` hint (parameters after `;` dropped, case folded; `image/gif`, `image/jpeg`, `image/jpg`,
  `image/pjpeg`, `image/png`). If that decoder answers `NotThisFormat`, or the hint names none of these, it
  sniffs; bytes no format matches are `Malformed`. Any other error of the hinted decoder is returned as is.
- **GIF** (`img/gif.rs`, the `gif` crate, RGBA output): decodes every frame and composites each onto a
  logical-screen-sized canvas, so each returned frame is a full image. Before a frame, the previous frame's
  disposal is applied (`Background` clears its rectangle to transparent, `Previous` restores the snapshot
  taken before it was drawn, `Keep`/`Any` leave the canvas). A source pixel of alpha 0 leaves the canvas
  pixel; any other replaces it. Drawing is clipped to the canvas. The delay is converted from centiseconds to
  milliseconds. The crate's own per-frame memory limit is raised to `MAX_DECODE_PIXELS * 4` bytes so the
  pixel cap is the only size gate. A stream with no frame is `Malformed`.
- **JPEG** (`img/jpeg.rs`, `jpeg-decoder` without default features): baseline and progressive. The frame
  header is read first and the pixel cap checked before `decode` allocates. Grayscale and RGB become opaque
  RGBA; 16-bit samples and CMYK/YCCK are `Unsupported`. One frame, delay 0.
- **PNG** (`img/png.rs`, the `png` crate with `EXPAND | STRIP_16 | ALPHA`): every colour type and bit depth
  normalises to 8-bit RGBA or grey+alpha, and grey+alpha is widened to RGBA. The pixel cap is checked after
  the header. APNG animation chunks are ignored: only the default image, one frame, delay 0.

## Tests

`tests/img_dispatch.rs` (10 tests: sniffing each format, the hint and its fallbacks, errors),
`tests/img_gif.rs` (12: an animated 3-frame fixture built in the test, disposal and delays, errors, the pixel
cap and the raised crate limit), `tests/img_jpeg.rs` (9: the checked-in baseline, progressive and CMYK
fixtures `fixtures/p4-*.jpg`, errors and the cap) and `tests/img_png.rs` (12: round trips of each colour
type built with the `png` encoder, errors and the cap). `tests/fuzz_totality.rs`'s
`fuzz_image_decode_never_panics` feeds mutated image fixtures to `decode_bytes`. The module has no inline
tests.

## Changes

none: imported

## Gate

- **I01-AC-1** — `decode_bytes` with no hint decodes a PNG by its magic number (exact pixels), a GIF (one frame) and the baseline JPEG fixture (16 wide). *Verify:* `cargo test --locked --jobs 2 --test img_dispatch -- --test-threads=2 sniffs_png_by_magic_with_no_hint sniffs_gif_by_magic_with_no_hint sniffs_jpeg_by_magic_with_no_hint` exits 0.
- **I01-AC-2** — `decode_bytes` decodes a PNG given the hint `image/png` or `image/png; charset=binary`. *Verify:* `cargo test --locked --jobs 2 --test img_dispatch -- --test-threads=2 honors_correct_content_type_hint honors_content_type_hint_with_charset_parameter` exits 0.
- **I01-AC-3** — A hint that names the wrong format (`image/gif` on PNG bytes) or no known format (`application/octet-stream`) falls back to sniffing and still decodes. *Verify:* `cargo test --locked --jobs 2 --test img_dispatch -- --test-threads=2 wrong_content_type_hint_falls_back_to_sniffing unknown_content_type_hint_falls_back_to_sniffing` exits 0.
- **I01-AC-4** — `decode_bytes` returns an error, without panicking, for bytes no format matches and for empty bytes, and `Malformed` or `Unsupported` for a PNG signature followed by garbage. *Verify:* `cargo test --locked --jobs 2 --test img_dispatch -- --test-threads=2 unrecognized_bytes_are_an_error_not_a_panic empty_bytes_are_an_error_not_a_panic malformed_bytes_with_valid_magic_are_an_error_not_a_panic` exits 0.
- **I01-AC-5** — `decode_bytes` does not panic on byte-mutated copies of the image fixtures under any of the three hints or none. *Verify:* `cargo test --locked --jobs 2 --test fuzz_totality fuzz_image_decode_never_panics -- --test-threads=2` exits 0.
- **I01-AC-6** — The hint is tried before sniffing, its parameters and case are ignored, and `image/jpg` and `image/pjpeg` name JPEG. *Verify:* unpinned (sniffing reaches the same decoder, so no test sees which path ran).
- **I01-AC-7** — Each decoder answers `NotThisFormat` for empty bytes and for another format's magic. *Verify:* `cargo test --locked --jobs 2 --test img_gif --test img_jpeg --test img_png -- --test-threads=2 wrong_magic_is_not_this_format empty_bytes_are_not_this_format` exits 0.
- **I01-AC-8** — Each decoder returns an error other than `NotThisFormat`, without panicking, for its bare magic number or signature, and an error for a stream cut short and for garbage after the magic. *Verify:* `cargo test --locked --jobs 2 --test img_gif --test img_jpeg --test img_png -- --test-threads=2 truncated_ garbage_` exits 0.
- **I01-AC-9** — An animated GIF decodes to every frame, each the full 4x4 canvas, with its delay converted from centiseconds to milliseconds (5, 7, 9 to 50, 70, 90). *Verify:* `cargo test --locked --jobs 2 --test img_gif decodes_all_frames_with_delays_in_milliseconds -- --test-threads=2` exits 0.
- **I01-AC-10** — A GIF frame's pixels are composited onto the canvas at its position: the first, full-canvas red frame decodes as all red, and a 2x2 frame at (1, 1) puts its opaque pixels there. *Verify:* `cargo test --locked --jobs 2 --test img_gif -- --test-threads=2 frame_a_is_full_canvas_opaque_red frame_b_composites_background_disposal_then_transparency` exits 0.
- **I01-AC-11** — `Background` disposal clears the previous frame's rectangle to transparent before the next frame is drawn. *Verify:* `cargo test --locked --jobs 2 --test img_gif frame_b_composites_background_disposal_then_transparency -- --test-threads=2` exits 0.
- **I01-AC-12** — `Previous` disposal restores the canvas to what it was before that frame was drawn. *Verify:* `cargo test --locked --jobs 2 --test img_gif frame_c_restores_previous_disposal_then_draws_bottom_half -- --test-threads=2` exits 0.
- **I01-AC-13** — A transparent GIF source pixel leaves the canvas pixel below it; `Keep` and `Any` disposal leave the canvas as it is; drawing is clipped to the canvas. *Verify:* unpinned.
- **I01-AC-14** — A single-frame GIF decodes to one frame. *Verify:* `cargo test --locked --jobs 2 --test img_gif single_frame_gif_yields_one_frame -- --test-threads=2` exits 0.
- **I01-AC-15** — A GIF stream with no image frame is `Malformed`. *Verify:* unpinned.
- **I01-AC-16** — A GIF, a PNG or a JPEG frame header declaring more than the 64,000,000-pixel cap (9000x9000; a progressive JPEG header of 12000x12000 with no scan data) is `Unsupported`. *Verify:* `cargo test --locked --jobs 2 --test img_gif --test img_jpeg --test img_png -- --test-threads=2 oversized_` exits 0.
- **I01-AC-17** — The pixel cap is checked before any pixel buffer is allocated, the cap is exactly 64,000,000 pixels, and a zero width or height is `Malformed`. *Verify:* unpinned.
- **I01-AC-18** — A 4000x3300 GIF (13,200,000 pixels: over the `gif` crate's default limit, under the pixel cap) decodes. *Verify:* `cargo test --locked --jobs 2 --test img_gif legitimate_large_image_under_our_cap_is_not_rejected_by_gifs_stricter_default_limit -- --test-threads=2` exits 0.
- **I01-AC-19** — The baseline and the progressive 16x16 JPEG fixtures decode to one frame of 16x16 opaque pixels whose left half is close to red and right half close to blue (within 30 per channel); the baseline frame's delay is 0. *Verify:* `cargo test --locked --jobs 2 --test img_jpeg -- --test-threads=2 decodes_baseline_jpeg_dimensions_and_regions decodes_progressive_jpeg_dimensions_and_regions` exits 0.
- **I01-AC-20** — A CMYK JPEG is `Unsupported`. *Verify:* `cargo test --locked --jobs 2 --test img_jpeg cmyk_jpeg_is_unsupported_not_silently_wrong_colors -- --test-threads=2` exits 0.
- **I01-AC-21** — A grayscale JPEG becomes opaque grey RGBA, and a 16-bit-sample JPEG is `Unsupported`. *Verify:* unpinned (no fixture).
- **I01-AC-22** — An RGBA8 PNG decodes to exactly its pixels, as one frame with delay 0. *Verify:* `cargo test --locked --jobs 2 --test img_png round_trips_rgba8_exactly -- --test-threads=2` exits 0.
- **I01-AC-23** — An RGB PNG and a palette PNG without `tRNS` decode opaque; a grayscale PNG decodes with its grey in all three channels and full alpha; a grey+alpha PNG keeps its alpha. *Verify:* `cargo test --locked --jobs 2 --test img_png -- --test-threads=2 round_trips_opaque_rgb_by_adding_full_alpha round_trips_palette_without_transparency_as_opaque round_trips_grayscale_by_replicating_into_rgb_with_full_alpha round_trips_grayscale_alpha_exactly` exits 0.
- **I01-AC-24** — A palette PNG with `tRNS` decodes its transparent index to alpha 0. *Verify:* `cargo test --locked --jobs 2 --test img_png round_trips_palette_with_transparency -- --test-threads=2` exits 0.
- **I01-AC-25** — A 16-bit PNG is reduced to 8 bits, and an APNG decodes to its default image only. *Verify:* unpinned.

## Risks

- `sniff_format` and each decoder's own magic check repeat the same magic numbers; a change to one and not
  the other makes `decode_bytes` sniff a format whose decoder then says `NotThisFormat`.
- The hint path returns the hinted decoder's `Malformed` or `Unsupported` without sniffing, so a lying
  `Content-Type` on a broken file of another format is reported as the hinted format's error.
- The GIF decoder clones the full canvas for every frame (and once more for each `Previous` frame): an
  animation of many frames costs frames x canvas bytes, bounded per frame only by the pixel cap.
- CMYK and 16-bit JPEG and APNG playback are refused or ignored on purpose; SPEC.md lists them among the
  rendering fidelity nits.
- The PNG decoder's comment says only RGBA and grey+alpha survive the transformations; any other colour type
  is reported `Unsupported` with "please file a bug".
- Every GIF frame is decoded and composited, but nothing plays them back yet: issue #119 asks `--x11` to
  play animated GIFs from these frames.
