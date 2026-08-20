# Browser chrome — address bar, back, throbber, status bar — design

**Date:** 2026-08-20 · **Status:** approved design · **Not an Acid2 packet** — the interactive browser view
requested after the Acid2 program completed.

## Goal
Give the interactive `--x11` window a minimal browser chrome: a top bar with a **back button**, an **address
bar** (the current URL), and a **throbber** (load indicator); and a bottom **status bar**. The document
renders in the region between them. Keep the from-scratch/self-rendered ethos: the chrome is drawn by the
engine into the same pixel `Surface`, no toolkit.

## Design principle: pure + goldenable core, thin interactive shell
The `--x11` event loop is manual-verify-only (no CI). So put everything testable in PURE functions —
**chrome layout** (window size → rects) and **chrome drawing** (into a `Surface`) — and add a headless
**screenshot** render mode that draws a page inside the chrome to a PNG, so CI pixel-goldens the chrome's
appearance. Only click-routing, history, and throbber animation live in `run_x11` (manual, like the rest of it).

## Non-negotiables
- No new dependency (draw with the existing `Surface` primitives + `text` glyphs). 1.44 MB floppy — report
  i486 delta. No JS / C3. Total: a hostile URL/status string never panics the chrome draw.
- Golden-safe: chrome is opt-in (`--x11` gets it; the plain `--dump-png` document render is unchanged unless
  `--chrome` is passed). Existing goldens untouched.

## Current state (ground-truthed)
- `--x11` `run_x11` (`main.rs`) holds `X11Session { dom, final_url }` + `RenderState { fragments, bg_images,
  doc_height }`; paints a viewport band via `paint_viewport_band` → `x11_full_redraw` (`PutImage` the whole
  window). `load_x11_page(url, width)` fetches+parses+reflows. Window `DEFAULT_X11_WIDTH/HEIGHT` (1024×768).
  It already tracks `final_url` and does link hit-testing/scroll. No history, no chrome.
- `Surface` trait: `fill_rect`, `blit`, `draw_text`, `put_pixel`, `set_clip` (P5). `MemSurface` is the impl.
  `text::BitmapFont::vga_8x16()` renders glyphs.

## Design

### 1. `src/backend/chrome.rs` (new, pure)
- **Constants:** `TOP_H: u32 = 28` (back button + address + throbber), `STATUS_H: u32 = 18`.
- **`pub struct ChromeLayout { pub top: Rect, pub back: Rect, pub address: Rect, pub throbber: Rect,
  pub viewport: Rect, pub status: Rect }`** and **`pub fn layout(win_w: u32, win_h: u32) -> ChromeLayout`**:
  top bar = full-width `TOP_H`; back = a ~24px square at left; throbber = a ~20px square at the top-right;
  address = the field between back and throbber; status = full-width `STATUS_H` at the bottom; viewport = the
  rectangle between top and status (`y = TOP_H`, `h = win_h - TOP_H - STATUS_H`). Pure; unit-tested (rects
  non-overlapping, viewport is window minus bars, degenerate tiny windows don't underflow — clamp to 0).
- **`pub struct ChromeState<'a> { pub url: &'a str, pub status: &'a str, pub loading: bool,
  pub throbber_frame: u8, pub can_go_back: bool }`**.
- **`pub fn draw(surface: &mut dyn Surface, lay: &ChromeLayout, st: &ChromeState)`**: fill `top` and `status`
  with a light-gray bar color; draw the `back` button (a filled box + a `◀`/`<` arrow glyph, dimmed when
  `!can_go_back`); fill `address` white + draw `st.url` (clipped to the field via `set_clip`, truncated/left-
  aligned); draw the `throbber` (a small spinner: pick 1 of N frames by `throbber_frame` when `loading`, else
  an idle dot/○ — a few `fill_rect`s or a glyph, no animation logic here); draw `st.status` text in the status
  bar. Totality: uses `set_clip` so long URLs/status can't draw outside their fields; empty strings are fine.
  Does NOT touch the `viewport` region (the document paints there separately).

### 2. Headless screenshot mode — `src/main.rs`
- Add a `--chrome` flag that, combined with `--dump-png <src> <out>`, renders the page into the chrome's
  `viewport` region and draws the chrome around it (window size = the `--dump-png` width × a fixed height, or a
  `--window WxH`), producing a full "browser screenshot" PNG. Reuse the existing dump-png fetch+layout, but
  paint the document band into `layout(win).viewport` (offset/clip the fragments to the viewport rect) and
  call `chrome::draw` for the bars with `ChromeState { url: final_url, status: "Done", loading: false,
  throbber_frame: 0, can_go_back: false }`. This is the CI-goldenable artifact.

### 3. History + `run_x11` integration — `src/main.rs` (manual-verified)
- Add a back-stack: `Vec<Url>` in `run_x11`; navigating to a new URL pushes the previous `final_url`; `can_go_back = !stack.is_empty()`.
- On redraw (`x11_full_redraw` and the scroll paths): paint the document band into the `viewport` region
  instead of the whole window (offset the band by `TOP_H`, clip to `viewport`), then `chrome::draw` the bars
  with the live `ChromeState` (url = session `final_url`, status = last status line, loading flag, a
  `throbber_frame` incremented per redraw while loading, `can_go_back`).
- Event handling: a `ButtonPress` inside `layout(win).back` and `can_go_back` → pop the stack, `load_x11_page`
  the previous URL, redraw. A link click inside the `viewport` (hit-test offset by `TOP_H`) → push current,
  navigate (adjust the existing link hit-test for the viewport offset). Status bar shows the URL being loaded
  during a fetch and "Done"/errors after. Throbber shows `loading` during `load_x11_page`.
- The address bar is display-only in this MVP (shows the URL); editable navigation (type a URL) is a later
  increment — noted, not built (keeps the packet bounded).

### Testing / fixtures
- **Unit (CI):** `chrome::layout` rect geometry (viewport = window − bars; no overlap; tiny-window clamp);
  `chrome::draw` into a `MemSurface` — assert the top/status bars are the bar color, the address field is
  white with dark URL glyph ink, the back button box is present; a hostile long URL/status doesn't draw
  outside its field (a pixel just outside the address field stays the bar color).
- **Golden (pixel-verified):** `--dump-png --chrome` over a simple fixture (e.g. `fixtures/basic.html`) → a
  browser-screenshot PNG: top bar with back button + the file URL in the address field + throbber, the
  document in the middle, a status bar at the bottom. Controller pixel-verifies (bars present, URL legible,
  document in the viewport region) before blessing.

### Charter / decisions
- This is a **C5 (the chair / interactive shell) feature**, not a C2 dialect amendment — note it in the
  charter's interactive-shell context, and record a DECISIONS entry: chrome drawn by the engine into the
  `Surface` (no toolkit); pure layout+draw goldened via a `--chrome` screenshot; history + click-routing +
  throbber-animation in `run_x11` (manual-verified); address bar display-only for now.

## Out of scope (this MVP)
- Editable address bar / typing a URL to navigate (display-only now); forward button; tabs; bookmarks UI;
  reload button; throbber true animation timing (frame advances per redraw, good enough); scrollbar chrome.
