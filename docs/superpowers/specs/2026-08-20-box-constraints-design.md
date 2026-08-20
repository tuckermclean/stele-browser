# Box constraints — min/max size, `overflow:hidden`, `background-position` (Acid2 Packet 5) — design

**Date:** 2026-08-20 · **Status:** approved design · **Program:** Acid2 roadmap Packet 5 of 7.

## Goal
Three independent, box-model-local features Acid2 needs: (1) `min-width`/`max-width`/`min-height`/`max-height`
clamping (taffy-native), (2) `overflow: hidden` paint clipping, (3) `background-position` for background
images. Acid2 uses min/max ×7, overflow ×3, plus background-position among its ~30 background refs.

## Non-negotiables
- No new dependency. 1.44 MB floppy — report i486 delta. Parsing TOTAL (bad values → initial, no panic).
- **Golden-safe:** each feature is inert at its initial value (`min:0`/`max:none`; `overflow:visible`;
  `background-position:0% 0%`) — a document not using them is byte-identical. Verify existing goldens unchanged.
- Test-first; no local i486 builds (CI); PNG goldens pixel-verified (controller). No JS / C3.

## Current state (ground-truthed)
- **taffy** `Style` has `min_size`/`max_size: Size<Dimension>` — the clamp is taffy's; `base_style`
  (`block.rs`) is the injection point (like P1's `inset`). `map`/`length`/`percent`/`auto` helpers exist.
- **overflow:** no paint clipping today. `emit` (`block.rs`) recurses building a FLAT `Vec<Fragment>`; the
  raster painter's `fill_rect`/`blit`/`draw_text` already clip to the SURFACE, but nothing clips a subtree to
  an ancestor's box. `Fragment { rect, kind, interactive }` (`layout/mod.rs`).
- **background images ARE painted** (`bg_images.rs` decodes; `raster.rs` blits+tiles a box's `background_image`,
  the tile phase "anchored to the box's own origin" — `raster.rs:363`). So `background-position` = offset that
  anchor. Only affects background IMAGES (a solid `background-color` fills the box regardless).

## Design

### 1. min/max size (taffy plumbing) — `computed.rs`/`value.rs`/`cascade.rs`/`block.rs`
- `ComputedStyle`: `min_width`, `max_width`, `min_height`, `max_height: Dimension` (reuse `Dimension`; defaults
  `min:*=Px(0)`? — use the CSS initial: `min-*: auto`(→0 for our purposes) and `max-*: none`. Represent
  `max` as `Dimension` with a `None`/`Auto` = "no max"; `min` default `Px(0.0)`/`Auto`). Non-inherited.
- `value.rs`: parse `min-width`/`max-width`/`min-height`/`max-height` (length | percentage | `auto`/`none`)
  — mirror `width`/`height` parsing; `max-*: none` ⇒ no max.
- `cascade.rs`: resolve (own, non-inherited), mirroring `width`/`height`.
- `block.rs base_style`: set taffy `min_size`/`max_size` from these (map `auto`/`none` → taffy's
  auto/none = no constraint). Taffy applies the clamp to the used size. Add a mapping test.

### 2. `overflow: hidden` clipping — `computed.rs`/`value.rs`/`cascade.rs`/`layout/mod.rs`/`block.rs`/`raster.rs`
- `computed.rs`: `pub enum Overflow { Visible, Hidden }` (default `Visible`, non-inherited); `overflow: Overflow`.
  (Acid2 uses `overflow:hidden`; `scroll`/`auto` render as `hidden` for a static paint — map them to Hidden;
  `visible` is the initial.)
- `value.rs`: parse `overflow` (and the `overflow-x`/`-y` longhands → if EITHER is hidden/scroll/auto, treat
  the box as clipping — Acid2-sufficient; a single `Overflow` field). Total.
- `cascade.rs`: own, non-inherited.
- **`layout/mod.rs`:** add `pub clip: Option<Rect>` to `Fragment` (the intersection of all ancestor
  `overflow:hidden` boxes' content rects that contain this fragment; `None` = unclipped).
- **`block.rs emit`:** thread a `clip: Option<Rect>` param through the recursion. When entering a container
  whose `style.overflow == Hidden`, compute `child_clip = intersect(clip, this box's border/padding rect)` and
  pass it to the children's `emit`; stamp every emitted `Fragment.clip = clip` (the clip in force where it's
  emitted). The container's OWN box fragment keeps the incoming `clip` (a box isn't clipped by its own
  overflow, only its descendants are). `intersect(Option<Rect>, Rect)` returns the overlapping rectangle
  (empty if disjoint).
- **`raster.rs`:** before drawing a fragment, intersect its target rect with `fragment.clip` (if `Some`); draw
  nothing outside it. Since `fill_rect`/`blit`/`draw_text` already clip to a rect, this is: compute the
  effective clip = surface ∩ fragment.clip, and pass it to those primitives (or skip the fragment if the
  intersection is empty). The tty backend ignores `clip` (no pixel clipping there — document it).

### 3. `background-position` — `computed.rs`/`value.rs`/`cascade.rs`/`raster.rs`
- `computed.rs`: `pub background_position: (LengthPercentage, LengthPercentage)` (x, y; default `(0%,0%)` =
  `Px(0)`-equivalent origin). Non-inherited.
- `value.rs`: parse `background-position` — 1 or 2 components: keywords `left/center/right`/`top/center/bottom`
  → 0%/50%/100%, or length/percentage. One value ⇒ x set, y = center(50%). Also extract the position from the
  `background` shorthand IF present (optional — mirror how the shorthand already extracts color/image; if
  awkward, skip the shorthand's position and just do the longhand — Acid2-sufficient). Total.
- `cascade.rs`: own, non-inherited.
- **`raster.rs`:** when blitting a box's `background_image`, offset the tile-phase anchor by the resolved
  background-position within the box (percentage resolves against `box_size - image_size` per CSS; length is a
  direct offset). Currently the anchor is the box origin (`raster.rs:363`) — add the position offset there.

## Testing / fixtures
- **Unit (CI):** min/max mapping onto taffy `min_size`/`max_size` (incl. a box whose width is clamped up by
  min and down by max); `overflow`/`background-position` parse totality; `emit` stamps `Fragment.clip` on a
  descendant of an `overflow:hidden` box (and `None` otherwise); `intersect` geometry.
- **Golden micro-fixtures** (pixel-verified):
  - `bc-minmax.html` — a box with `width:40px; min-width:80px` (clamped UP to 80) beside one with
    `width:300px; max-width:100px` (clamped DOWN to 100).
  - `bc-overflow.html` — a `overflow:hidden` parent (e.g. 60×60) containing a larger child (e.g. a 200×200
    colored box); the child is clipped to the parent's box.
  - `bc-bg-position.html` — a box with a `background-image` (a `data:` PNG — P4!) positioned e.g.
    `background-position: right bottom` (or `20px 10px`); the image sits at the offset, not the origin.

## Charter / decisions
- Charter C2: add `min`/`max-width`/`height`, `overflow:hidden` (clip), `background-position` (Acid2 Packet 5).
- DECISIONS: min/max taffy-native; `overflow:hidden` via a per-fragment clip rect stamped at `emit` time
  (flat fragment stream, no paint-tree) with `scroll`/`auto`→`hidden` for static paint; `background-position`
  offsets the bg-image tile anchor (bg-color unaffected); tty ignores clip.

## Out of scope (YAGNI)
- `overflow: scroll`/`auto` interactive scrolling (static render — clip only); `overflow-x`≠`overflow-y`
  independence (single clip); `min/max` on flex/grid cross-axis beyond taffy's native handling;
  `background-position` on solid-color backgrounds (no-op); `background-size`/`background-repeat` control
  (existing tiling behavior stays).
