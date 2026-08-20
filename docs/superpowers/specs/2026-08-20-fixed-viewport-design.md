# Fixed-viewport render mode — design

**Date:** 2026-08-20 · Goal: an opt-in render mode that lays the document root out at a FIXED viewport height
(the window), so `html{overflow:hidden}` (P5) clips the positioned content into a window instead of the
default content-height sprawl. Unblocks the Acid2 smiley (its face is positioned within a fixed 800x600
viewport with `html{overflow:hidden}`; today it renders 800x3960) and is what a real windowed browser needs.

## Design
- **`layout_tree` (block.rs):** today it clamps the root WIDTH to `vw` (lines ~390-396) and ignores `vh`
  (`let _ = vh`). Add an opt-in HEIGHT clamp symmetric to the width one: when requested and `vh > 0`, set the
  root's `style.size.height = length(vh)` (+ `box_sizing = BorderBox`, same as width) so taffy lays the root
  at fixed height. With `html{overflow:hidden}` the root container clips its descendants to that box (P5's emit
  clip). Thread the choice via a `clamp_height: bool` param on an internal impl.
- **`layout` (mod.rs):** keep `pub fn layout(root, viewport)` = content-height (clamp_height=false, unchanged —
  every existing golden untouched). Add `pub fn layout_viewport(root, viewport)` = clamp_height=true.
- **CLI (main.rs):** add `--viewport-height <N>` (CSS px). When set with `--dump-png`, render via
  `layout_viewport` at `(width, N)` (the doc root fixed to Nx viewport height, clipped) instead of the plain
  content-height `layout`. Absent → unchanged content-height render (golden-safe).
- Non-negotiables: no new dep; parse total; golden-safe (opt-in — no `--viewport-height` ⇒ byte-identical);
  no local i486 builds; no JS/C3.

## Verify
- Unit: `layout_viewport` clamps the root to the given height (a tall document's root fragment height == vh);
  an `overflow:hidden` root clips a taller child to the viewport (fragment clip present). `layout` unchanged.
- Experiment: render `fixtures/acid2.html` via `--dump-png --viewport-height 600` at width 800 → CONTROLLER
  views/measures whether the face composes into ~800x600 (vs the 800x3960 sprawl). Golden + KILL-gate ONLY if
  it genuinely composes the reference smiley (never rubber-stamp — honest measurement either way).
