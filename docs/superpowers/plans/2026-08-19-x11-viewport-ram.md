# X11 O(viewport) RAM — PR 2 (T5) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Retire the whole-document `MemSurface` in `--x11` so interactive RAM is O(viewport), not O(page) — the tall-page doctrine applied where scrolling actually lives.

**Architecture:** `reflow_from_dom` stops painting a content-height surface; it returns a `RenderState` (fragments + bg_images + doc height). A new `paint_viewport_band` paints only the fragments intersecting a requested page-y band into a **band-sized** surface (cull by y-range, translate into band-local coords, reuse `raster::paint` unchanged — `MemSurface` clips straddling writes). The `--x11` redraw helpers paint bands on demand instead of cropping a giant surface.

**Tech Stack:** Rust std; `raster::paint` (unchanged); `MemSurface` (clips writes, verified). No new deps.

**Spec:** `docs/superpowers/specs/2026-08-19-x11-responsiveness-design.md` (§ "PR 2 — T5"). Builds on PR 1 (merged: pixmap double-buffer, `reflow_from_dom`, coalesce loop).

## Global Constraints

- **`--x11` only.** `fb.rs`/tty/headless-dump untouched. **`raster.rs` is NOT modified** — so the A5 PNG surface goldens (headless `dump_png` path) stay byte-identical by construction. If any A5 golden moves, that's a bug, not a re-bless.
- **No change to what is painted**, only how much surface is allocated: a band shows the same pixels the same rows of the old whole-doc surface showed (`MemSurface` clips off-band writes; the band is white where no fragment lands — same as the old crop's white padding).
- **O(viewport) RAM is the deliverable:** no code path may allocate a `MemSurface` taller than the viewport (± the scroll strip) for `--x11`. The fence test asserts this structurally (band surface height == requested band height, independent of document height).
- **No local builds** (AGENTS.md §3): implementers transcribe verbatim + commit; CI compiles + runs `cargo test`. The `--x11` window path has no server in CI — the redraw wiring (Task 2) has no unit test; its correctness rests on the pure `paint_viewport_band`/cull tests + the (PR-1) xvfb smoke + operator testimonial.
- **Totality:** non-finite fragment coords are skipped (as the existing content-height loop already does); no panic on a tall/empty doc.
- **Branch:** `packet/x11-viewport-ram`, off `main` (includes PR 1). Conventional subjects (`refactor(x11):`, `perf(x11):`, `test(x11):`).
- **Size:** report the i486 delta (this REMOVES a large allocation and adds a small paint helper; net code size ~neutral, RAM much lower). Measure.

## Current state (ground-truthed, post-PR1)

- `reflow_from_dom(dom, final_url, width) -> Result<(MemSurface, Vec<Fragment>), String>` (main.rs) builds a `MemSurface::new(width, content_height.clamp(1, MAX_PNG_HEIGHT))` — up to ~64 MB — and `raster::paint`s the WHOLE document into it. This surface is the O(page) allocation.
- `x11_full_redraw`/`x11_scroll_to` (main.rs) `crop_surface_rows(surface, width, band_h, scroll_y)` a band out of that whole-doc surface, convert, and `put_image` it into the pixmap.
- `raster::paint(surface: &mut dyn Surface, fragments: &[Fragment], bg_images: &HashMap<String, Rc<RgbaImage>>, canvas: Color)` — paints each fragment at `fragment.rect.origin.y`. `MemSurface` clips all writes to bounds (verified: `put_pixel`/`fill_rect`/`blit` all clip).
- `Fragment { rect: Rect, kind, interactive }` derives `Clone`; `Rect { origin: Point{x,y}, size: Size{w,h} }`, `Point`/`Size` derive `Copy`.
- `X11Session { dom, final_url }` + `load_x11_page(url, width) -> Result<(X11Session, MemSurface, Vec<Fragment>), String>` (PR 1) — the navigation loader.
- No RSS/tall-page memory test exists (the spec's "existing fence" is aspirational). This plan CREATES an allocation-size fence (robust; not a flaky RSS probe).

## File Structure

- **Modify** `src/main.rs` — the whole change: `RenderState`, `reflow_from_dom` (returns `RenderState`, no surface), `paint_viewport_band` + `visible_translated_fragments`, `load_x11_page` (returns `RenderState`), `run_x11` + `x11_full_redraw`/`x11_scroll_to` rewired, `crop_surface_rows` removed. `raster.rs`/`x11.rs` untouched.
- **Modify** `JOURNAL.md` — packet note + the O(viewport) fence result.

---

### Task 1: `RenderState` + `paint_viewport_band` (the O(viewport) core, pure/testable)

Split rendering into "reflow → state" and "state → band surface". Fully CI-testable.

**Files:** Modify `src/main.rs`.

**Interfaces:**
- Produces:
  - `struct RenderState { fragments: Vec<layout::Fragment>, bg_images: std::collections::HashMap<String, std::rc::Rc<stele::img::RgbaImage>>, doc_height: u32 }`
  - `fn reflow_from_dom(dom_tree: &dom::ast::Dom, final_url: &Url, width: u32) -> Result<RenderState, String>` (changed return type)
  - `fn visible_translated_fragments(fragments: &[layout::Fragment], band_page_y: u32, band_h: u32) -> Vec<layout::Fragment>`
  - `fn paint_viewport_band(state: &RenderState, width: u32, band_page_y: u32, band_h: u32) -> MemSurface`
  - `fn load_x11_page(url: &Url, width: u32) -> Result<(X11Session, RenderState), String>` (changed)

- [ ] **Step 1: Write the failing tests**

Add to `src/main.rs`'s `#[cfg(test)]` module. (The exact `HashMap`/`Rc`/`RgbaImage` paths must match what `collect_bg_images` returns — `std::collections::HashMap<String, std::rc::Rc<stele::img::RgbaImage>>`; adjust if the in-file alias differs.)

```rust
    #[test]
    fn paint_viewport_band_allocates_a_viewport_sized_surface_not_the_document() {
        // A very tall document scrolled far down must still paint into a
        // band-HEIGHT surface (O(viewport)), never a document-height one.
        let html = format!("<html><body>{}</body></html>", "<p>line</p>".repeat(4000));
        let dom = stele::dom::parser::parse(&html);
        let state = reflow_from_dom(&dom, &Url::new("file:///tall.html"), 800).expect("reflow");
        assert!(state.doc_height > 5000, "the fixture must be genuinely tall (was {})", state.doc_height);
        let band = paint_viewport_band(&state, 800, 4000, 768);
        assert_eq!(stele::surface::Surface::size(&band), (800, 768),
            "band surface must be viewport-sized regardless of doc height");
    }

    #[test]
    fn visible_translated_fragments_culls_out_of_band_and_translates() {
        let html = format!("<html><body>{}</body></html>", "<p>line</p>".repeat(4000));
        let dom = stele::dom::parser::parse(&html);
        let state = reflow_from_dom(&dom, &Url::new("file:///tall.html"), 800).expect("reflow");
        let total = state.fragments.len();
        let band = visible_translated_fragments(&state.fragments, 2000, 768);
        assert!(band.len() < total, "a band must contain fewer fragments than the whole doc");
        assert!(!band.is_empty(), "a mid-document band must contain some fragments");
        // Every returned fragment intersects the band, and its y is translated
        // into band-local coords (so it lands within/near [0, 768)).
        for f in &band {
            let y = f.rect.origin.y;
            let h = f.rect.size.h;
            assert!(y + h > -1.0 && y < 768.0, "fragment y {y} h {h} not in band-local range");
        }
    }

    #[test]
    fn reflow_from_dom_returns_state_without_a_document_surface() {
        // Structural O(viewport) guarantee: reflow no longer returns a MemSurface
        // at all — it returns render state; painting is deferred to band paints.
        let html = "<html><body><p>hi</p></body></html>";
        let dom = stele::dom::parser::parse(html);
        let state = reflow_from_dom(&dom, &Url::new("file:///x.html"), 800).expect("reflow");
        assert!(!state.fragments.is_empty());
        assert!(state.doc_height >= 1);
    }
```

- [ ] **Step 2: Verify they fail** — CI: `cargo test` → FAIL (`RenderState`/`paint_viewport_band`/`visible_translated_fragments` undefined; `reflow_from_dom` returns a tuple).

- [ ] **Step 3: Implement**

Add the struct and functions to `src/main.rs`. Change `reflow_from_dom` to drop the surface allocation:

```rust
/// The retained, O(1)-in-viewport render state for `--x11`: the fragment list
/// (already produced by layout) plus the bg-image map and document height.
/// Painting is DEFERRED to `paint_viewport_band` — no whole-document surface is
/// ever allocated (the tall-page doctrine: interactive RAM is O(viewport)).
struct RenderState {
    fragments: Vec<layout::Fragment>,
    bg_images: std::collections::HashMap<String, std::rc::Rc<stele::img::RgbaImage>>,
    doc_height: u32,
}

fn reflow_from_dom(dom_tree: &dom::ast::Dom, final_url: &Url, width: u32) -> Result<RenderState, String> {
    if frames::find_frameset(dom_tree).is_some() {
        return Err("frameset documents are not supported by --x11".to_string());
    }
    let author_sheets = stele::stylesheets::collect_all_author_sheets(dom_tree, final_url, width as f32, style::ColorScheme::Light);
    let styles = cascade::cascade(dom_tree, &author_sheets);
    let images = stele::images::collect_images(dom_tree, final_url);
    let Some(root) = build_box_tree(dom_tree, &styles, &images) else {
        return Err("empty document (nothing to render)".to_string());
    };
    let viewport = Size { w: width as f32, h: HEADLESS_VIEWPORT_HEIGHT };
    let fragments = layout::layout(&root, viewport);

    let mut content_bottom = 0.0f32;
    for f in &fragments {
        let y = f.rect.origin.y;
        let h = f.rect.size.h;
        if y.is_finite() && h.is_finite() {
            content_bottom = content_bottom.max(y + h);
        }
    }
    let doc_height = if content_bottom.is_finite() && content_bottom > 0.0 {
        (content_bottom.ceil() as u32).clamp(1, MAX_PNG_HEIGHT)
    } else {
        1
    };
    let bg_images = stele::bg_images::collect_bg_images(&styles, final_url);
    Ok(RenderState { fragments, bg_images, doc_height })
}

/// The fragments intersecting the page-y band `[band_page_y, band_page_y +
/// band_h)`, cloned and translated into band-local coords (origin.y shifted up
/// by `band_page_y`). Culling keeps painting O(visible), not O(document).
fn visible_translated_fragments(fragments: &[layout::Fragment], band_page_y: u32, band_h: u32) -> Vec<layout::Fragment> {
    let y0 = band_page_y as f32;
    let y1 = (band_page_y + band_h) as f32;
    fragments
        .iter()
        .filter(|f| {
            let y = f.rect.origin.y;
            let h = f.rect.size.h;
            y.is_finite() && h.is_finite() && (y + h) > y0 && y < y1
        })
        .map(|f| {
            let mut g = f.clone();
            g.rect.origin.y -= y0;
            g
        })
        .collect()
}

/// Paint only the page-y band `[band_page_y, band_page_y + band_h)` into a
/// fresh `band_h`-tall surface (O(viewport) RAM). `raster::paint` is reused
/// unchanged; `MemSurface` clips any straddling-fragment writes to the band.
fn paint_viewport_band(state: &RenderState, width: u32, band_page_y: u32, band_h: u32) -> MemSurface {
    let band_h = band_h.max(1);
    let visible = visible_translated_fragments(&state.fragments, band_page_y, band_h);
    let mut band = MemSurface::new(width, band_h, Color::WHITE);
    raster::paint(&mut band, &visible, &state.bg_images, Color::WHITE);
    band
}
```

Update `load_x11_page` to return `(X11Session, RenderState)`:

```rust
fn load_x11_page(url: &Url, width: u32) -> Result<(X11Session, RenderState), String> {
    let response = fetch_response(url)?;
    let html = String::from_utf8_lossy(&response.body);
    let dom = dom::parser::parse(&html);
    let session = X11Session { dom, final_url: response.final_url.clone() };
    let state = reflow_from_dom(&session.dom, &session.final_url, width)?;
    Ok((session, state))
}
```

- [ ] **Step 4: Verify they pass** — CI: `cargo test` → the three new tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/main.rs
git commit -m "perf(x11): RenderState + paint_viewport_band — O(viewport) band painting"
```

---

### Task 2: Rewire `run_x11` + redraw helpers onto `RenderState` (impure)

Make the loop hold `RenderState` and paint bands on demand; drop the whole-doc surface and `crop_surface_rows`. No CI window test (no server); A5 goldens unaffected (raster untouched).

**Files:** Modify `src/main.rs`.

- [ ] **Step 1: Repoint the redraw helpers at `RenderState` + `paint_viewport_band`**

- `x11_full_redraw(conn, state: &RenderState, pixmap, window, gc, depth, bpp, scanline_pad, width, height, scroll_y)`: replace `let cropped = crop_surface_rows(surface, width, height, scroll_y);` + its `convert_to_fb_bytes` on a whole-doc crop with: `let band = paint_viewport_band(state, width, scroll_y, height);` then convert `band`'s bytes (`band` is already `width × height`, so `crop_surface_rows(&band, width, height, 0)` is just `band.bytes()` — use the surface's raw bytes directly, or convert the band). Keep `x11_row_stride`/`convert_to_fb_bytes`/`put_image(pixmap,…)`/`copy_area(pixmap,window)` exactly.
- `x11_scroll_to(conn, state: &RenderState, pixmap, …, old_scroll_y, new_scroll_y)`: the `Partial` strip paint changes from `crop_surface_rows(surface, width, strip_h, strip_page_y)` to `paint_viewport_band(state, width, strip_page_y, strip_h)` (a `strip_h`-tall band); the within-pixmap `copy_area` for retained rows is UNCHANGED; the `Full` arm delegates to `x11_full_redraw`.
- Both take the band's raw RGBA bytes (a band surface is already viewport/strip-sized, top-left aligned, so no cropping needed — read its bytes via the `Surface`/`MemSurface` byte accessor the code already uses).

Note for the implementer: `convert_to_fb_bytes` currently consumes the `Vec<u8>` from `crop_surface_rows`. A band surface's bytes are the same RGBA8 layout; obtain them the same way `crop_surface_rows` did internally (it read `surface` rows). Simplest: keep a tiny `fn surface_rgba_bytes(&MemSurface) -> Vec<u8>` (or reuse `crop_surface_rows(&band, width, band_h, 0)` which returns the whole band unchanged) so the `convert_to_fb_bytes` call site is untouched. Prefer reusing `crop_surface_rows(&band, width, band_h, 0)` — it returns exactly the band's bytes with zero offset — to minimize change; then `crop_surface_rows` is still used (don't remove it) but only ever with `scroll_y == 0` on an already-band-sized surface.

DECISION for the implementer: **keep `crop_surface_rows`** and call it as `crop_surface_rows(&band, width, band_h, 0)` inside the redraw helpers — this reuses the existing byte-extraction + white-padding logic verbatim and keeps its unit test valid. (The band is already the right size, so offset 0 is a straight copy.) This is simpler and lower-risk than removing it.

- [ ] **Step 2: Rewire `run_x11`**

- Replace `let mut surface: MemSurface` + `let mut fragments` with `let mut state: RenderState` (from the initial `load_x11_page`).
- Everywhere the loop used `surface`/`fragments`:
  - Scroll clamp: `x11_max_scroll(state.doc_height, height)` (was `Surface::size(&surface).1`).
  - Click hit-test: `hit_test_pixel(&state.fragments, doc_x, doc_y)` (was `&fragments`).
  - `x11_full_redraw`/`x11_scroll_to` calls: pass `&state` (was `&surface`).
  - Navigation (`Click`/`Reload`/initial): `load_x11_page(url, width)` → set `session`, `state` (was `surface`/`fragments`).
  - `Resize`: `reflow_from_dom(&session.dom, &session.final_url, width)` → set `state` (was `surface`/`fragments`); the pixmap free/recreate + clamp + full_redraw unchanged.

- [ ] **Step 3: Commit** (no unit test — impure `--x11` wiring; A5 goldens byte-identical because `raster.rs`/headless path untouched)

```bash
git add src/main.rs
git commit -m "perf(x11): drive the loop from RenderState; paint bands, never a whole-doc surface"
```

- [ ] **Step 4: Push and confirm goldens hold + compile**

```bash
git push -u origin packet/x11-viewport-ram
```

Read `m0-acceptance`. Pass: compile green; `cargo test` green (Task 1 tests); **A5 PNG goldens byte-identical**; the non-gating `x11-smoke` still opens/scrolls/quits clean. A golden move = a bug (raster/headless was supposedly untouched) — root-cause, don't re-bless.

---

### Task 3: Interactive O(viewport) fence test + JOURNAL

Prove the peak allocation stays O(viewport) across a full-document scroll (the "RSS fence" reimagined as an allocation-size assertion, since no RSS harness exists).

**Files:** Modify `src/main.rs` (test), `JOURNAL.md`.

- [ ] **Step 1: Write the full-scroll fence test**

Add to `src/main.rs`'s test module. Simulates scrolling a genuinely tall document through the band-paint layer and asserts every band — at every scroll position — is viewport-sized, never document-sized:

```rust
    #[test]
    fn full_scroll_of_a_tall_document_only_ever_paints_viewport_bands() {
        // 68k.news-scale stand-in: a very tall document.
        let html = format!("<html><body>{}</body></html>", "<p>paragraph</p>".repeat(6000));
        let dom = stele::dom::parser::parse(&html);
        let state = reflow_from_dom(&dom, &Url::new("file:///tall.html"), 800).expect("reflow");
        assert!(state.doc_height > 10_000, "fixture must be much taller than a viewport (was {})", state.doc_height);

        let viewport_h = 768u32;
        let max_scroll = state.doc_height.saturating_sub(viewport_h);
        // Walk the whole document in viewport steps; every painted band must be
        // exactly viewport-height — the peak surface allocation is O(viewport),
        // never the ~doc_height*width*4 the old whole-document MemSurface took.
        let mut y = 0u32;
        while y <= max_scroll {
            let band = paint_viewport_band(&state, 800, y, viewport_h);
            assert_eq!(stele::surface::Surface::size(&band), (800, viewport_h));
            y += viewport_h;
        }
        // And the final clamped band.
        let last = paint_viewport_band(&state, 800, max_scroll, viewport_h);
        assert_eq!(stele::surface::Surface::size(&last), (800, viewport_h));
    }
```

- [ ] **Step 2: Verify it passes** — CI: `cargo test full_scroll_of_a_tall_document` → PASS.

- [ ] **Step 3: JOURNAL note**

Append to `JOURNAL.md` (newest at bottom, matching style): T5 landed — `--x11` retired the whole-document `MemSurface`; `reflow_from_dom` returns `RenderState` (fragments + bg_images + doc_height), and `paint_viewport_band` paints only the visible band (cull by y-range + translate) into a viewport-sized surface. Interactive RAM is now O(viewport) — a tall page (68k.news-scale) no longer allocates up to ~64 MB (the old content-height × width × 4 clamp). The O(viewport) guarantee is fence-tested by asserting every band across a full scroll is viewport-height. `raster.rs`/headless path untouched → A5 goldens byte-identical. Report the i486 size delta from CI.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs JOURNAL.md
git commit -m "test(x11): O(viewport) fence — full scroll only paints viewport bands; journal T5"
```

---

## Self-Review

**1. Spec coverage (T5):**
- "Retire the whole-document MemSurface; paint only the viewport band; fragment culling by y-range" → Task 1 (`RenderState`, `paint_viewport_band`, `visible_translated_fragments`) + Task 2 (loop/helpers rewired, no whole-doc surface). ✓
- "Interactive RSS fence — full-document scroll, assert O(viewport)" → Task 3, reimplemented as an allocation-size fence (band height == viewport across a full scroll) since no RSS harness exists. **Deviation from the spec's letter, noted:** the spec said "extend the existing headless tall-page fence," but no such fence exists; an allocation-size assertion is a stronger, non-flaky proof of O(viewport). ✓ (documented gap)
- "Composes with T2 (pixmap reads bands from the client surface; T5 swaps the client source to viewport-only)" → Task 2 keeps the pixmap/CopyArea path; only the band source changes from crop-of-whole-doc to paint-of-band. ✓

**2. Placeholder scan:** No "TBD"/"handle errors". The one judgment call (reuse `crop_surface_rows(&band,…,0)` vs a new bytes accessor) is given an explicit DECISION (reuse it) with rationale, not left open.

**3. Type consistency:** `RenderState { fragments, bg_images, doc_height }` used identically in Tasks 1/2/3. `paint_viewport_band(&RenderState, u32, u32, u32) -> MemSurface` and `visible_translated_fragments(&[Fragment], u32, u32) -> Vec<Fragment>` consistent between definition, the redraw helpers, and the tests. `reflow_from_dom` new return type `Result<RenderState, String>` matches its callers (`load_x11_page`, the Resize arm, the tests). `load_x11_page -> Result<(X11Session, RenderState), String>` matches `run_x11`'s navigation arms.

**4. Golden safety (the load-bearing invariant):** `raster.rs` and the headless `dump_png`/`render_fb_surface` path are NOT touched — restated in Global Constraints and Task 2 Step 4. `MemSurface` clipping (verified) makes band-painting of straddling fragments safe. The only rendered-output risk would be a `raster.rs` edit, which this plan forbids.
