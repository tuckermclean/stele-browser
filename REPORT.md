# REPORT — M4 images (packet/m4-images)

Implementer report for the orchestrator. Not merged into `JOURNAL.md`/
`DECISIONS.md` by this packet — rationale below is for the orchestrator to
fold in (or not) at review time.

## What landed

`<img>` now renders real pixels. The pipeline, end to end:

`decode` (`img::decode_bytes`, already existed, P4) → `pre-pass`
(`images::collect_images`, new) → `box tree` (`layout::box_tree::
build_box_tree`, now threads decoded images into `Replaced` boxes) →
`layout` (`layout::block::emit`, now emits `FragmentKind::Image` for a
decoded `Replaced`) → `paint` (`backend::raster::paint`, now calls
`surface.blit`) → `blit` (`MemSurface::blit`, the frozen `todo!()` stub,
now real).

1. **Commit 1 — freeze amendment** (`src/layout/mod.rs`): `BoxContent::
   Replaced` gains one field, `image: Option<Rc<RgbaImage>>`. `Rc` (not an
   owned `RgbaImage`) so cloning a `LayoutNode` during layout translation
   never copies a pixel buffer — everything here is single-threaded, so
   `Rc` is the right primitive, not `Arc`. Every construction/match site
   updated with `image` always `None` at this commit — confirmed
   byte-identical `goldens/basic.png` and a fully green suite before any
   feature code landed.

2. **`MemSurface::blit`** (`src/surface/mem.rs`): nearest-neighbor-scales
   the source image into the target rect and alpha-blends each sampled
   pixel over the destination via the same `put_pixel` blend `fill_rect`/
   `draw_glyph` already use. All coordinates are widened to `i64` before any
   arithmetic (a `u32::MAX`-wide target rect can't overflow computing its
   far edge), and the destination is clipped to the surface bounds up front
   — the pixel loop itself never runs more iterations than the *clipped*
   rect needs, unlike relying on `put_pixel`'s own per-pixel clip alone.
   No-ops on a zero-sized image or zero-sized target rect (guards the
   division in the scale math too).

3. **`images::collect_images`** (`src/images.rs`, new module): walks a
   parsed `Dom` for `<img src>` elements (bounded by its own `DEPTH_CAP`,
   matching the pattern already established by `box_tree`/`block`/
   `dom_util`), resolves each `src` against the document's base `Url`,
   fetches it (`file://`/`http://`, a small duplicated fetch helper
   mirroring `frames.rs`'s own documented "small, total, driver-level,
   duplicated rather than shared with the bin" convention — this copy keeps
   the `Content-Type` header as a decode hint), and decodes via
   `img::decode_bytes`, taking **frame 0** for the static render. A fetch
   error, unsupported scheme, or malformed/unrecognized image simply leaves
   that `NodeId` out of the returned map — never a panic, never aborts the
   page. Capped at `MAX_IMAGES = 256` fetch+decode attempts per document.
   Only ever called on the `--dump-png` path; `--dump-text` always passes an
   empty map (a tty dump paints no pixels, so there's nothing to decode
   for).

4. **Threading** (`src/layout/box_tree.rs`, `src/layout/block.rs`,
   `src/backend/raster.rs`): `build_box_tree`/`build_node` gained an
   `images: &HashMap<NodeId, Rc<RgbaImage>>` parameter (implementation
   detail, not frozen) — an `<img>`'s `Replaced` box gets
   `images.get(&id).cloned()`. `layout::block::emit`'s `Replaced` arm
   branches on that field: `Some(image)` emits exactly one `Image`
   fragment at the box's laid-out rect (one real pixel-buffer clone here,
   out of the `Rc`, at emit time — the brief's one sanctioned clone);
   `None` keeps the pre-existing placeholder `Box`. `raster::paint`'s
   `Image` arm is now a one-line hand-off to `surface.blit`.

## Screenshot

`goldens/images.png` — **PROPOSED**, not self-blessed (brief §10: an
implementer never blesses its own render blind). Rendered from
`fixtures/images.html` via the real `stele --headless --dump-png` pipeline
(`accept.sh`'s new **A3f** check, `--tty-only` locally PASSing alongside
A3/A3b/A3c/A3d/A3e). Dimensions: **800×347** px.

What it should show, top to bottom, at the default 800px viewport width:

- An "Images" `<h1>` heading.
- "A PNG:" followed by a solid **red** 16×16 square (`fixtures/images-red.png`,
  generated — see below).
- "A JPEG:" followed by a 16×16 **photographic** square (`fixtures/
  p4-baseline.jpg`, reused as-is from the P4 image-decoder packet — NOT a
  flat color, since it's real JPEG content, unlike the other three).
- "A GIF:" followed by a solid **blue** 16×16 square (`fixtures/
  images-blue.gif`, generated).
- "An animated GIF:" followed by a solid **yellow** 16×16 square
  (`fixtures/images-anim.gif`, generated as a 2-frame animation — yellow
  then green — confirming this packet's documented "frame 0 only for the
  static render" rule: the *first* frame's color is what should appear,
  never the second/green frame).

I sampled the golden's decoded RGBA pixels directly (a small script, not
committed) to confirm placement before proposing it: red `(220,30,30)`
first appears around row 116, mixed JPEG colors (including a purple/blue
region, consistent with real photographic content) around row 180, blue
`(30,60,220)` around row 244, and yellow `(220,200,30)` — the animated
GIF's frame 0, not its frame 1 green — around row 308. `tests/
images_golden.rs::all_four_images_actually_decode_not_fallen_back_to_placeholders`
independently pins that all four `<img>`s decoded (not silently fell back
to placeholder boxes) as a check distinct from the pixel-exact golden
comparison.

## Test summary (scoped to this packet's new/touched code)

All green, `cargo +nightly test` (full suite): 302 lib unit tests + every
integration test binary, **0 failures**, 1 explicitly `#[ignore]`d
(`tests/gen_images_fixtures.rs`'s generator — intentionally not part of the
normal suite, see its own doc comment). Packet-scoped highlights:

- `src/surface/mem.rs`: +8 tests for `blit` (nearest-neighbor scaling,
  alpha-blending over background, zero-size image/target no-ops,
  off-surface/overhanging-rect clipping, huge-rect totality).
- `src/images.rs`: +8 tests for `collect_images` (resolves + fetches +
  decodes a real `<img src>`, resolves a relative `src` against the
  document base, missing-file and malformed-bytes totality, non-`<img>`/
  missing-`src` no-op, empty document, `MAX_IMAGES` cap, deep-nesting
  totality).
- `src/layout/box_tree.rs`: +2 tests (`images` map threads a decoded image
  into the matching `<img>`'s `Replaced` box by exact `Rc`; an `<img>` with
  no map entry stays `image: None`).
- `tests/layout_block.rs`: +1 test (`Replaced` with `Some(image)` emits
  exactly one `Image` fragment at its rect, not a placeholder `Box`).
- `src/backend/raster.rs`: `image_fragment_is_skipped_not_blitted` (M2-era,
  premise now false) replaced with 2 tests: an opaque image's pixels
  actually land in the fragment's rect via `blit`; a fully transparent
  image still paints nothing, via `blit`'s own alpha blend (not a
  paint-level skip).
- `tests/images_golden.rs`: 3 tests — THE SCREENSHOT's exact pixel match
  vs. the PROPOSED golden (real `file://` fetch, not `include_str!` —
  needed since the images pre-pass does real I/O), golden well-formedness/
  non-blankness, all-four-images-decoded sanity check.
- `accept.sh --tty-only`: A3/A3b/A3c/A3d/A3e/**A3f** all PASS locally.

## `git diff main..HEAD` on frozen surfaces

- `src/layout/mod.rs`: **only** the `Replaced` variant's new `image` field
  (plus its doc comment) — no other type/signature in this file changed.
- `src/surface/mod.rs` (the `Surface` trait itself), `src/style/*`,
  `src/fetch/*`, `src/img/*`: **untouched** (`git diff main..HEAD --
  <path>` is empty for each).
- `Cargo.toml`/`Cargo.lock`: **untouched** — no new dependency (the `gif`
  crate, already a dependency for decoding, is also used to *encode* the
  two generated GIF fixture assets; no new crate was added for that).
- `grep -rn unsafe` over this packet's diff: no matches.
- `box_tree::build_box_tree`'s signature changed (gains an `images`
  parameter) — this is implementation, not a frozen type, per the packet
  brief's explicit sanction.

## Decisions for the ledger (orchestrator's call on what to keep)

- **`Rc<RgbaImage>` in `Replaced`**, not an owned `RgbaImage` or `Arc`:
  single-threaded throughout (charter), so `Rc` avoids pixel-buffer copies
  on every `LayoutNode` clone during layout translation at zero real cost.
- **`MAX_IMAGES = 256`**: bounds total fetch+decode work per document
  independent of the P4 decoders' own per-image `MAX_DECODE_PIXELS` cap —
  a page with tens of thousands of `<img>`s can't drive unbounded
  network/decode work. Images past the cap render as their ordinary
  intrinsic-size placeholder, same as a fetch/decode failure.
- **Animated GIF → frame 0 only** for the static `--dump-png` render (the
  brief's own call) — `images::collect_images` takes `frames[0].image` and
  discards the rest. The ticking/interactive loop is out of scope here.
- **`blit` scaling: nearest-neighbor**, inverse-mapped per output pixel
  (same technique `MemSurface::draw_glyph` already uses for text) — handles
  any target rect size, not just integer multiples of the source image.
- **PNG-only decode gating**: the images fetch+decode pre-pass only runs on
  the `--dump-png` path; `--dump-text` always passes an empty map, so a
  tty-only run never pays fetch/decode cost for images it can't paint
  anyway (they stay `[alt]`-style placeholders, unchanged from before this
  packet).
- **Generated fixture assets**: `fixtures/images-red.png` (16×16 solid red,
  via `backend::raster::encode_png` — the same encoder the existing PNG
  goldens already trust), `fixtures/images-blue.gif` (16×16 solid blue,
  one frame), `fixtures/images-anim.gif` (16×16, 2 frames — yellow then
  green, 500ms apart, looping) — all produced by a **committed generator**,
  `tests/gen_images_fixtures.rs` (`#[ignore]`d; run explicitly via `cargo
  test --test gen_images_fixtures -- --ignored` to reproduce them
  byte-for-byte, since every generated image is a flat solid color with no
  timestamp/randomness involved). The JPEG asset is not generated —
  `fixtures/p4-baseline.jpg` (already committed, from the P4 packet) is
  reused as-is.
- **`fixtures/images.html`**: all four `<img>`s sit in normal flow (no
  `align=left`) — floats are explicitly the next packet's job, not this
  one's.
