# X11 responsiveness + resource pass — design

**Date:** 2026-08-19
**Status:** approved design, pre-implementation
**Scope:** `--x11` interactive shell only. Two packets — PR 1 = T1–T4 + T6 (latency); PR 2 = T5 (RAM).

## Goal

Make `--x11` scrolling and resizing responsive under **both** cost models it ships against — Xfbdev on the Monolith/486 (tiny local server, cheap round-trips, retained contents) and Xwayland/WSLg (bursty events, compositor-serialized ops, non-retained window contents). The current loop was implicitly tuned for the first. The fix is to assume neither: **batch input, batch output, keep frames server-side**, and hold only O(viewport) pixels.

`--fb` is out of scope: ground-truthing confirmed it is a one-shot render-to-`/dev/fb0` (`render_fb: Option<String>`; `fb.rs` is output-only — `render_to_device`/`convert_to_fb_bytes`, no event loop, no scroll). The interactive shell is `--x11` everywhere it ships; `fb.rs` is not touched.

## Non-negotiables this design serves

- **Charter/AGENTS discipline:** TDD (failing test first), implementer/reviewer split, golden blessing by pixel-verification, no-local-i486-build (CI compiles).
- **No JavaScript / no uninvited computation (C3):** untouched — this is backend plumbing.
- **1.44 MB floppy = 1,474,560 bytes:** this adds X11 client code (new encoders, a fold layer, a frame buffer) and removes a large allocation (T5). Report the size delta from the CI `stele-i486` artifact; expected small. Measure, don't guess.
- **No timers, no frame pacing, no async.** Paint immediately, once, per drained batch. Responsiveness here means *fewer, larger, sooner* — never scheduled.
- **Totality:** a malformed/short event read stays a clean `Err` (loop exits cleanly); the fold and encoders never panic.

## Current state (ground-truthed against `main`; line-refs verified)

| # | Finding | Location |
|---|---|---|
| F1 | `next_event` blocks on one 32-byte event (`read_exact`); `run_x11` does one `x11_scroll_to` (a full blit) per event — no coalescing; wheel 4/5 and key autorepeat all hit this | `x11.rs:906`, `main.rs:1244`/`1403` |
| F2 | `encode_create_gc` sends value-mask = 0, so graphics-exposures uses the X default TRUE → every `CopyArea` spawns NoExpose/GraphicsExpose we read-and-discard; discarding GraphicsExpose on non-retaining servers = scroll garbage | `x11.rs:382` |
| F3 | `Expose` is a unit variant; region `(x,y,w,h)` and `count` discarded → an Expose series triggers N full-viewport `PutImage`s (~1.9 MB each at 800×600×32) instead of one | `x11.rs:677` |
| F4 | `render_x11_page` calls `fetch_response` inside it; ConfigureNotify calls `render_x11_page` → interactive resize re-downloads + reparses + re-lays-out per resize event | `main.rs:972`, `main.rs:1338` |
| F5 | `render_x11_page` paints the **whole document** into a `MemSurface` sized to content height, clamped to `MAX_PNG_HEIGHT = 20_000` → O(page) interactive RAM (up to ~64 MB at 800px) | `main.rs:1008`, `main.rs:57` |
| F6 | `XConnection::send` = one `write_all` per encode → banded PutImage = many small writes, no frame-level buffering | `x11.rs:813` |

The render pipeline inside `render_x11_page` (relevant to T3): `fetch_response` → `dom::parser::parse` → `collect_all_author_sheets(dom, final_url, WIDTH, Light)` → `cascade` → `collect_images` → `build_box_tree` → `layout::layout(root, viewport{WIDTH, HEADLESS_VIEWPORT_HEIGHT})` → content-height → `collect_bg_images` → `MemSurface` + `raster::paint`. **Width-independent:** fetch, parse (`dom_tree`), `final_url`, `collect_images`. **Width-dependent** (media queries resolve at viewport width): `collect_all_author_sheets` → `cascade` → `build_box_tree` → `layout` → paint.

## PR 1 — T1–T4 + T6 (latency)

### T1 — input coalescing

Split into a **pure decision layer** (byte-testable) and a **thin impure drain**:

- `XConnection::drain_events(&mut self) -> Result<Vec<XEvent>, String>`: block for the first event (today's `next_event` semantics), then `poll` the socket with a zero timeout (rustix `event::poll`, the same primitive `run_shell`'s tty loop already uses in `main.rs`) and non-blockingly read every currently-queued **complete** 32-byte event, buffering any partial remainder for the next call. Returns the batch (≥1). Impure, thin, not unit-tested (no server in CI).
- `fold_events(&[XEvent]) -> Vec<XIntent>` — **pure**, co-located with `parse_event` in `x11.rs` (this is protocol-layer event coalescing, the same layer as parsing; it depends only on `XEvent`). `XIntent` is a small enum: `ScrollBy(i32)`, `Resize { w, h }`, `Expose(Rect)`, `Click { x, y }`, `Key(...)`, `Quit`. Folding rules:
  - Consecutive scroll-generating events (wheel 4/5, Up/Down, PageUp/Down) sum into **one** net `ScrollBy` delta.
  - `Expose` regions accumulate into **one** union `Rect` (see T2's full parse + `count`).
  - `ConfigureNotify` collapses to the **last** size in the batch.
  - Clicks and non-scroll keys pass through **in order**, splitting the scroll runs around them (a click between two wheel runs yields: ScrollBy(run-1), Click, ScrollBy(run-2)).
- `run_x11` calls `drain_events` → `fold_events` → acts on each `XIntent` using the **existing** decisions (`x11_scroll_to`, `hit_test_pixel`/navigate, relayout, repaint). Scroll-amount and clamping decisions stay where they are (`X11_LINE_SCROLL`, `x11_max_scroll`).

**Contract tests (headless, bytes→intents):** 50 synthetic wheel events → exactly one `ScrollBy` with the summed delta; a mixed batch preserves click ordering relative to the scroll runs around it; a ConfigureNotify storm → one `Resize` at the final size.

### T2 — server-side frame (the WSLg correctness + cost fix)

- **`CreateGC` with `graphics-exposures = FALSE`**: set the value-mask bit for graphics-exposures (`0x00010000`) and append one value word `0`. Kills the self-inflicted NoExpose/GraphicsExpose flood *and* the scroll-garbage risk on non-retaining servers.
- **Double-buffer via a server-side `Pixmap`** (viewport-sized, matching the window's depth):
  - Scroll: `CopyArea` **within** the pixmap (shift the retained region) + banded `PutImage` of only the newly-exposed strip **into** the pixmap, then one `CopyArea` pixmap→window.
  - Expose (any series, any region): `CopyArea` pixmap→window of the damage rect — **zero** client→server image bytes, no reliance on window backing-store retention (the WSLg garbage risk dies here).
  - Navigation / resize (content changed): repaint the pixmap's viewport from the surface, then `CopyArea` pixmap→window.
- **Parse `Expose` fully** (`x, y, w, h, count`); T1's fold accumulates the region until `count == 0`, then repaints once via the pixmap copy.
- **New byte-exact encoders** (same pure-encoder pattern as the rest of `x11.rs`, unit-tested against captured bytes): `encode_create_pixmap`, `encode_free_pixmap`, and `encode_create_gc` extended to carry the graphics-exposures value (or a dedicated variant). The pixmap is (re)created at the window's depth on first paint and on resize; freed on resize before recreate.

### T3 — resize sanity

Split `render_x11_page` into navigation vs reflow:

- **Session cache** (a small struct held by `run_x11`): `{ dom_tree, final_url, images }` — the width-independent artifacts. Populated on every navigation (initial load, link click, F5 reload) via fetch+parse.
- **`reflow(cache, width) -> (MemSurface, Vec<Fragment>)`**: from the cached `dom_tree`/`final_url`, re-run the **width-dependent** tail — `collect_all_author_sheets(width)` → `cascade` → `build_box_tree` → `layout` → paint. ConfigureNotify calls `reflow` only: **zero network, zero reparse**.
- **Contract test** with a counting fetch stub: a navigation triggers exactly one fetch; N subsequent resizes trigger **zero** additional fetches.
- T1's ConfigureNotify coalescing means `reflow` runs once per drained batch at the final size, not per resize event.

### T4 — output batching

- **Frame-buffered writes on `XConnection`:** a frame accumulates its requests (the CopyAreas + PutImage bands of one paint) into a single buffer flushed with one `write`/`writev` per frame, instead of `send`'s current per-encode `write_all`. Shape: a `begin_frame()`/`end_frame()` (or a `send_frame(&[&[u8]])`) that buffers then flushes once. `send`'s existing single-write path stays for one-off requests (setup, GC/pixmap creation).
- **Measure:** strace the 50-wheel storm before/after; the syscall-count delta goes in the T6 report.

### T6 — instrumentation + acceptance

- **`--stats` / debug-env counters** for `--x11`: events-drained-per-batch, `scroll_to` calls, PutImage bytes/frame, CopyArea ops, frames painted. Before/after table for the 50-wheel storm (expect ~50 blit rounds → 1).
- **CI (required, headless):** the T1 fold contract tests, the T3 zero-fetch-on-resize test, the new-encoder byte-exact tests (CreatePixmap/FreePixmap/CreateGC-with-graphics-exposures), the full-Expose parse test, and the **A5 PNG surface goldens staying byte-identical** (the paint output is unchanged; only *how* it reaches the window changed).
- **CI (optional, non-gating):** one timeboxed xvfb smoke — open a window, drive a synthetic 50-wheel storm, quit clean. xvfb does not replicate WSLg's compositor cost model and can be flaky, so it never gates merge.
- **Operator testimonial (journaled):** WSLg feel before/after on the same WSL box; 486/Xfbdev per-frame cost on the Monolith (see Guardrails). The world proves the hardware.

## PR 2 — T5 (O(viewport) for the interactive shell)

- **Retire the whole-document `MemSurface` in `--x11`.** Retain the `Fragment` list (already produced); paint only the **viewport band** into a **viewport-sized** surface. Scroll paints only the newly-exposed band's fragments — **fragment culling by y-range** (a fragment is painted iff its `[y, y+h]` intersects the visible band). This is the tall-page doctrine finally applied where scrolling actually lives.
- **Interactive RSS fence:** extend the existing headless tall-page fence to an interactive-shape test — layout the 68k.news-scale fixture, simulate a full-document scroll **through the decision layer** (fold + band selection, no real X server), and assert peak RSS stays O(viewport) (budget per the existing headless fence).
- Composes with T2: T2's server-side pixmap reads bands from the client surface; T5 swaps that client source from whole-doc to viewport-only. Server buffer vs client buffer — orthogonal.
- Lands as its **own PR after** T1–T4 ship the latency fixes. The feel-fix does not block on the RAM-fix.

## Verification model (what "merge-ready" means)

Because CI has **no X server** and there is **no real 486** in reach, verification is split:

- **CI proves (required):** the pure fold decisions (T1), zero-fetch-on-resize (T3), byte-exact encoders (T2), full-Expose parse (F3/T2), and that the rendered **surface** is unchanged (A5 PNG goldens byte-identical).
- **CI attempts (non-gating):** the xvfb smoke (T6).
- **The operator proves (journaled, per the world-proves-the-hardware doctrine):** actual **window-pixel** correctness (CI can't see X pixels), WSLg responsiveness feel, and 486/Xfbdev per-frame cost.

This seam is explicit: the "byte-identical gui golden" guardrail is **not** something CI can assert on window pixels — CI asserts the surface content and the encoder bytes; window-pixel correctness rests on the encoder tests + the xvfb smoke + your testimonial.

## Guardrails

- **Xfbdev/Monolith must not regress.** The A5 PNG surface goldens stay byte-identical through T1–T4 (the pixmap path produces the same window pixels from the same surface). Scroll on the DX2 profile must not gain per-frame cost — the pixmap adds one server-side `CopyArea` per frame, cheaper than the bus-bound alternative everywhere we ship, but **measure it in the 486 profile before merging PR 1**, not after (operator-measured on the Monolith; journaled).
- **No timers, no frame pacing, no async** (restated — it is the core discipline): paint immediately, once, per drained batch.

## Out of scope (YAGNI)

- `fb.rs` / `--fb` (one-shot render; no interactive surface to shrink).
- The tty shell's loop (already decision-layer-pure; untouched).
- Frame pacing / vsync / animation.
- Any change to the paint output itself (the surface goldens must not move).
