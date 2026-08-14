# REPORT — M4 pixel foundation (packet/fb-pixel-foundation)

Implementer report for the orchestrator. Not merged into `JOURNAL.md`/
`DECISIONS.md` by this packet — rationale below is for the orchestrator to
fold in (or not) at review time.

## Fonts / licenses

- **Font**: `font8x8_basic` — 128 glyphs (ASCII `U+0000..=U+007F`), 8 bytes
  per glyph (one byte per pixel row).
- **Source**: <https://raw.githubusercontent.com/dhepper/font8x8/master/font8x8_basic.h>
  (repository <https://github.com/dhepper/font8x8>, by Daniel Hepper).
- **License**: **Public Domain**, per that file's own header comment. It in
  turn credits Marcel Sondaar / International Business Machines' original
  public-domain VGA fonts as its basis. Public Domain has no attribution or
  copyleft obligation, so nothing further needs to ship in the binary or
  this repo beyond the citation itself.
- Embedded as a compiled-in Rust `const` table in `src/text/glyphs.rs` (not
  fetched at runtime — no network/file dependency in the shipped binary).
  The same citation is duplicated in that module's doc comment, the source
  of truth going forward.

## What landed

1. **Font atlas** (`src/text/glyphs.rs`, new): the `font8x8_basic` table
   plus `pub(crate) fn lookup(ch: char) -> [u8; 8]`, total over all of
   `char` — any scalar outside `0x00..=0x7F` renders a small hollow
   "tofu box" fallback rather than vanishing or panicking. Bit order: bit 0
   (LSB) of each row byte is the **leftmost** pixel — confirmed empirically
   by decoding the 'A' glyph into a recognizable capital-A silhouette (see
   the module's doc comment and its `capital_a_decodes_to_a_recognizable_
   silhouette` test).
2. **`BitmapFont` inherent methods** (`src/text/bitmap.rs`): `glyph(ch) ->
   [u8; 8]` (delegates to `glyphs::lookup`) and `glyph_scale(size_px) -> f32`
   (a `pub` wrapper over the existing private `scale()`, so the rasterizer
   reuses the exact same size_px -> scale mapping the `Metrics` trait
   methods already use — no duplicated/drifting math). Both are additive;
   no frozen signature touched.
3. **`MemSurface::draw_text`** (`src/surface/mem.rs`): fills in the frozen
   method's body. Placement rule: each glyph's 8px-tall source box has its
   **bottom row sit exactly on `run.baseline`** — at the font's native size
   (16px, scale 1.0) that lands the glyph at design rows `[4, 12)` of
   `vga_8x16`'s 16-row/12-ascent cell, i.e. centered in the cell AND
   baseline-bottom-aligned at once (8 is exactly half of 16, and 12 - 8 = 4),
   so there was no top-vs-centered tradeoff to make — see the doc comment on
   `MemSurface::draw_glyph`. Scaling is nearest-neighbor, computed by
   inverse-mapping each *output* pixel back to a source pixel (rather than
   forward-filling source rows into variable-width bands), so it handles any
   positive `scale` — including non-integer ones (e.g. a 24px heading, scale
   1.5) — with no gaps or overlaps. A `MAX_GLYPH_PX` cap (1024px per glyph
   edge) bounds the per-glyph pixel loop against a hostile/huge `size_px`.
4. **Raster painter** (`src/backend/raster.rs`, new): `paint(surface,
   fragments)`, the pixel analog of `backend::tty::render`. `Box` fragments
   fill `background_color` (skipped when fully transparent) then each
   `solid`, nonzero-width border edge as its own filled rect in that edge's
   own color. `Text` fragments build a `TextRun` from the fragment's own
   `rect.origin.x`/`rect.origin.y + baseline` (mirroring `backend::tty`'s
   documented "top of line box + offset" contract for that field) and call
   `Surface::draw_text`. `Image` fragments are skipped — `// TODO(images
   packet): blit`; `MemSurface::blit` is untouched (still `todo!()`).
   `encode_png(surface: &MemSurface) -> Vec<u8>` lives in the same module:
   deterministic RGBA8 PNG encoding via the `png` crate (no timestamp/text
   chunks), with a zero-dimension surface degrading to a blank 1x1 white PNG
   instead of handing the encoder an invalid `IHDR`.
5. **`--dump-png <src> <out.png>` CLI** (`src/main.rs`): parallel to
   `--dump-text`, gated the same way behind `--headless`. Fixed 800px
   viewport width (`DEFAULT_PNG_WIDTH`, no `--width` flag yet — a documented
   v0 simplification), content-driven height (max fragment bottom edge,
   bounded by `MAX_PNG_HEIGHT` = 20,000px the same way `backend::tty`
   bounds `MAX_GRID_ROWS`). A fetch error, unsupported scheme, empty/
   `display:none` document, or `<frameset>` document (pixel rendering of
   frames is explicitly out of scope here — a follow-up's job) all degrade
   to a clean blank 1x1 PNG (`blank_png()`), never a panic — mirroring
   `dump_text`'s own totality contract.
6. **First pixel golden**: `goldens/basic.png` (800x247), rendered from
   `fixtures/basic.html` via the real `--dump-png` path. **PROPOSED, NOT
   self-blessed** — per the brief's blessing discipline, this implementer
   is not the one who countersigns it; the orchestrator should open the PNG
   and confirm it looks right (headings, a paragraph with a blue link,
   correct line spacing) before trusting it. `tests/png_golden.rs` asserts
   an exact match by **decoding both PNGs and comparing RGBA pixel arrays**
   (not raw bytes), per the brief. `accept.sh`'s new **A3e** check
   round-trips the same fixture through the *compiled* binary (a real
   `file://` fetch, not `include_str!`) and compares raw PNG bytes — valid
   here specifically because `encode_png` is proven deterministic (see
   `encode_png_is_deterministic`), so a byte compare and a pixel compare
   agree; the Rust test remains the actual pixel-level check.

## Test summary (scoped to this packet's new/touched code)

All green, `cargo +nightly test` (full suite): all lib unit tests plus every
integration test binary, **0 failures**. Packet-scoped highlights:

- `src/text/glyphs.rs`: 6 tests (bit order, printable-ASCII coverage,
  non-ASCII fallback, totality).
- `src/text/bitmap.rs`: +4 tests for `glyph`/`glyph_scale`.
- `src/surface/mem.rs`: +9 tests for `draw_text` (native-size placement,
  advance, nearest-neighbor 2x scaling, empty/degenerate/huge `size_px`,
  off-surface totality, color/alpha).
- `src/backend/raster.rs`: 13 tests (background fill, all four border
  edges, `None`-style border no-op, text painting, empty text, `Image`
  skip/no-blit, paint-order-wins-ties, degenerate-rect totality, PNG
  round-trip/zero-dim-fallback/determinism).
- `src/main.rs`: +8 tests for `--dump-png` (arg parsing, valid PNG at
  default width, blank-PNG fallback on fetch-error/bad-scheme/frameset,
  filesystem write round-trip, unwritable-path `Err`).
- `tests/png_golden.rs`: 3 tests (exact pixel match vs. the PROPOSED
  golden, golden well-formedness/non-blankness, empty-document canvas
  shape).
- `accept.sh --tty-only`: A3/A3b/A3c/A3d/**A3e** all PASS locally.

## `git diff main..HEAD` on frozen files

`src/surface/mod.rs`, `src/layout/mod.rs`, `src/style/*`, `src/text/mod.rs`
(doc-comment-only change, see below), `src/dom/*`, `src/fetch/*`,
`src/img/*`, `Cargo.toml`, `Cargo.lock` — **no type or signature changed**.
`Cargo.toml`/`Cargo.lock` are byte-for-byte unchanged (`png` was already a
dependency). `src/text/mod.rs`'s only change is its module-level doc
comment (mentions `glyphs` now existing) plus a new `pub(crate) mod
glyphs;` line — no existing item touched. `MemSurface::draw_text`'s body
was filled in (signature frozen, unchanged); `MemSurface::blit` is
untouched, still `todo!()`.

## Decisions for the ledger (orchestrator's call on what to keep)

- **Font**: `font8x8_basic` (Public Domain), embedded compiled-in. Bit
  order: bit 0 (LSB) = leftmost pixel.
- **Glyph placement**: bottom-of-glyph sits on the baseline (not top- or
  center-of-cell placement) — the only choice that's simultaneously
  "centered in the 16-unit cell" and "sits on the baseline" for an 8-tall
  glyph in a 16-tall/12-ascent cell, so no real tradeoff was made.
- **Scaling**: nearest-neighbor, inverse-mapped per output pixel — supports
  any positive `size_px`/scale, not just integer multiples of 16.
- **`--dump-png` default viewport width**: 800px, fixed, no CLI override
  yet (`DEFAULT_PNG_WIDTH` in `main.rs`).
- **v0 simplifications / explicitly out of scope this packet**: images
  (`FragmentKind::Image`, `MemSurface::blit`) — next packet's job, per the
  task brief. Floats — also explicitly out of scope. Frameset documents
  render to a blank placeholder PNG via `--dump-png`, not real pixels —
  flagged as a follow-up, not fixed here.
