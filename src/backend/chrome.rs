//! Browser chrome — pure layout + drawing for the top bar (back button,
//! address field, throbber) and the bottom status bar.
//!
//! design: docs/superpowers/specs/2026-08-20-browser-chrome-design.md §1.
//! Deliberately pure: `layout` maps a window size to pixel `Rect`s, `draw`
//! paints those rects into a `Surface` from a snapshot `ChromeState` — no
//! I/O, no event handling, no history/animation state. That belongs to
//! `run_x11` (manual-verified, T3); everything HERE is unit-tested and,
//! via the `--chrome` screenshot mode (T2), golden-tested too.

use crate::style::computed::FontWeight;
use crate::surface::{Color, Rect, Surface, TextRun};
use crate::text::{Metrics, TerminusFont};

/// Height in pixels of the top bar (back button + address field + throbber).
pub const TOP_H: u32 = 28;
/// Height in pixels of the bottom status bar.
pub const STATUS_H: u32 = 18;

/// Text is always drawn at the font's native resolution (`text::
/// text_render_px`'s floor) — chrome labels are short and fixed-size, no
/// author `font-size` involved, so there is no reason to ever go below it.
const TEXT_SIZE_PX: f32 = 16.0;

/// Pixel geometry for one chrome frame, computed from the window size.
/// Every field is a `Rect` in the same surface-pixel space `Surface`'s
/// draw ops use; `viewport` is where the document itself paints — `draw`
/// never touches it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChromeLayout {
    /// The full-width top bar (back + address + throbber sit inside it).
    pub top: Rect,
    /// The back-navigation button, a roughly-square box at the left of `top`.
    pub back: Rect,
    /// The address field, the space between `back` and `throbber`.
    pub address: Rect,
    /// The throbber/load-indicator, a roughly-square box at the right of `top`.
    pub throbber: Rect,
    /// The document paint area: everything between `top` and `status`.
    pub viewport: Rect,
    /// The full-width status bar at the bottom of the window.
    pub status: Rect,
}

/// Compute the chrome layout for a `win_w` x `win_h` window.
///
/// Total: every dimension is clamped so degenerate/tiny windows (down to
/// `0x0`) never underflow a `u32` subtraction — bar heights shrink to fit
/// the window before anything else is computed, and the derived button/
/// address rects are clamped again against whatever bar space is actually
/// left. Worst case every rect collapses to zero size; none ever panics.
pub fn layout(win_w: u32, win_h: u32) -> ChromeLayout {
    // Bars claim only as much height as the window actually has, top bar
    // first (it's the primary chrome), status bar from what's left.
    let top_h = TOP_H.min(win_h);
    let status_h = STATUS_H.min(win_h.saturating_sub(top_h));
    let viewport_h = win_h.saturating_sub(top_h).saturating_sub(status_h);

    let top = Rect { x: 0, y: 0, w: win_w, h: top_h };
    let status = Rect { x: 0, y: (win_h.saturating_sub(status_h)) as i32, w: win_w, h: status_h };
    let viewport = Rect { x: 0, y: top_h as i32, w: win_w, h: viewport_h };

    // Back button: a ~24px square inset a couple px from the top bar's
    // left edge, clamped to fit inside both the bar's height and the
    // window's width.
    const INSET: i32 = 2;
    let back_size = 24u32.min(top_h.saturating_sub(4)).min(win_w.saturating_sub(4));
    let back_y = INSET + ((top_h.saturating_sub(4).saturating_sub(back_size)) / 2) as i32;
    let back = Rect { x: INSET, y: back_y, w: back_size, h: back_size };

    // Throbber: a ~20px square inset from the top bar's right edge, same clamp.
    let throbber_size = 20u32.min(top_h.saturating_sub(4)).min(win_w.saturating_sub(4));
    let throbber_x = (win_w as i64 - INSET as i64 - throbber_size as i64).max(0) as i32;
    let throbber_y = INSET + ((top_h.saturating_sub(4).saturating_sub(throbber_size)) / 2) as i32;
    let throbber = Rect { x: throbber_x, y: throbber_y, w: throbber_size, h: throbber_size };

    // Address field: whatever's left between `back` and `throbber`,
    // clamped to never go negative-width (a tiny window can make `back`
    // and `throbber` overlap or even swap order — the field just
    // collapses to zero rather than underflowing).
    const GAP: i64 = 4;
    let addr_x = back.x as i64 + back.w as i64 + GAP;
    let addr_right = throbber.x as i64 - GAP;
    let addr_w = (addr_right - addr_x).max(0) as u32;
    let address = Rect { x: addr_x as i32, y: back.y, w: addr_w, h: back.h };

    ChromeLayout { top, back, address, throbber, viewport, status }
}

/// A snapshot of the interactive state `draw` needs to paint one frame.
/// Owned/updated by `run_x11` (T3); this struct itself carries no behavior.
pub struct ChromeState<'a> {
    /// The current page URL, shown display-only in the address field.
    pub url: &'a str,
    /// The status line, shown in the bottom status bar.
    pub status: &'a str,
    /// Whether a page fetch is in flight (drives the throbber's animated frame).
    pub loading: bool,
    /// Which throbber frame to draw while `loading` (`% N` inside `draw`).
    pub throbber_frame: u8,
    /// Whether the back button should render enabled (a non-empty history stack).
    pub can_go_back: bool,
}

/// Bar background (top bar + status bar).
const BAR_COLOR: Color = Color::rgb(221, 221, 221);
/// Back-button box fill when enabled.
const BUTTON_COLOR: Color = Color::rgb(200, 200, 200);
/// Back-button box fill when disabled (close to `BAR_COLOR`, reads as dim).
const BUTTON_DISABLED_COLOR: Color = Color::rgb(212, 212, 212);
/// Ink for enabled text/glyphs (back arrow, address, status).
const INK: Color = Color::rgb(17, 17, 17);
/// Ink for disabled/dim glyphs (back arrow when `!can_go_back`).
const INK_DIMMED: Color = Color::rgb(160, 160, 160);
/// Throbber tick/dot fill.
const THROBBER_COLOR: Color = Color::rgb(90, 90, 90);

/// Paint one chrome frame: the top bar (back button, address field,
/// throbber) and the bottom status bar. Never touches `lay.viewport` — the
/// document paints there separately.
///
/// Total: every sub-draw guards zero-size rects (`w == 0 || h == 0`) before
/// doing anything with them, and every text draw is wrapped in
/// `set_clip(Some(field))` / `set_clip(None)` so a hostile/very-long
/// `url`/`status` string can't paint past its field's edge — no panics, no
/// indexing overflow, empty strings are simply invisible.
pub fn draw(surface: &mut dyn Surface, lay: &ChromeLayout, st: &ChromeState) {
    if lay.top.w > 0 && lay.top.h > 0 {
        surface.fill_rect(lay.top, BAR_COLOR);
    }
    if lay.status.w > 0 && lay.status.h > 0 {
        surface.fill_rect(lay.status, BAR_COLOR);
    }

    draw_back_button(surface, lay.back, st.can_go_back);
    draw_address(surface, lay.address, st.url);
    draw_throbber(surface, lay.throbber, st.loading, st.throbber_frame);
    draw_status(surface, lay.status, st.status);
}

fn draw_back_button(surface: &mut dyn Surface, rect: Rect, can_go_back: bool) {
    if rect.w == 0 || rect.h == 0 {
        return;
    }
    let (box_color, ink) = if can_go_back { (BUTTON_COLOR, INK) } else { (BUTTON_DISABLED_COLOR, INK_DIMMED) };
    surface.fill_rect(rect, box_color);
    draw_centered_glyph(surface, rect, '<', ink);
}

fn draw_address(surface: &mut dyn Surface, rect: Rect, url: &str) {
    if rect.w == 0 || rect.h == 0 {
        return;
    }
    surface.fill_rect(rect, Color::WHITE);
    draw_left_aligned_clipped(surface, rect, url, INK);
}

fn draw_status(surface: &mut dyn Surface, rect: Rect, status: &str) {
    if rect.w == 0 || rect.h == 0 {
        return;
    }
    draw_left_aligned_clipped(surface, rect, status, INK);
}

/// A rotating tick when `loading` (one of `FRAMES` positions around the
/// box, selected by `frame % FRAMES`), a static centered dot when idle.
/// No animation timing lives here — `run_x11` owns advancing `frame` per
/// redraw; this just renders whichever frame it's handed.
fn draw_throbber(surface: &mut dyn Surface, rect: Rect, loading: bool, frame: u8) {
    if rect.w == 0 || rect.h == 0 {
        return;
    }
    let cx = rect.x + rect.w as i32 / 2;
    let cy = rect.y + rect.h as i32 / 2;
    let short_side = rect.w.min(rect.h);

    if !loading {
        // Idle: a small centered dot (approximating ○).
        let s = (short_side / 3).max(1);
        surface.fill_rect(Rect { x: cx - (s as i32 / 2), y: cy - (s as i32 / 2), w: s, h: s }, THROBBER_COLOR);
        return;
    }

    // Loading: a tick that visits one of 4 positions around the box.
    const FRAMES: u8 = 4;
    let tick = (short_side / 4).max(1);
    let half_tick = tick as i32 / 2;
    let (px, py) = match frame % FRAMES {
        0 => (cx, rect.y),                             // top
        1 => (rect.x + rect.w as i32 - 1, cy),          // right
        2 => (cx, rect.y + rect.h as i32 - 1),          // bottom
        _ => (rect.x, cy),                              // left
    };
    surface.fill_rect(Rect { x: px - half_tick, y: py - half_tick, w: tick, h: tick }, THROBBER_COLOR);
}

/// Draw `text` left-aligned inside `rect` with a small left/vertical
/// padding, clipped so it can never spill past `rect`'s edges regardless
/// of length. Empty strings are a no-op (handled by `Surface::draw_text`
/// itself); the clip is always reset to `None` afterward so it doesn't leak
/// into whatever draws next.
fn draw_left_aligned_clipped(surface: &mut dyn Surface, rect: Rect, text: &str, color: Color) {
    const PAD_X: i32 = 4;
    // packet/terminus-font, Task 4: TerminusFont replaces BitmapFont::
    // vga_8x16() everywhere -- numerically identical at TEXT_SIZE_PX (16px,
    // the default bucket both agree on exactly), so this swap is a pure
    // glyph-shape change here, not a metrics change (design doc §3).
    let font = TerminusFont::new();
    // Cast the (target-stable) metrics to integers ONCE, then center with
    // pure integer arithmetic. A float `(rect.h - line_h) / 2.0` here rounds
    // differently under the i486 target's x87 80-bit intermediates than under
    // the host's SSE, shifting the text a pixel and breaking the host==i486
    // golden byte-identity every other A5 render already relies on.
    let ascent = Metrics::ascent(&font, TEXT_SIZE_PX) as i32;
    let line_h = Metrics::line_height(&font, TEXT_SIZE_PX) as i32;
    let text_top = rect.y + (rect.h as i32 - line_h).max(0) / 2;
    let baseline = text_top + ascent;

    surface.set_clip(Some(rect));
    // Chrome labels never carry an author `font-weight` -- always Normal.
    surface.draw_text(&TextRun { text, x: rect.x + PAD_X, baseline, size_px: TEXT_SIZE_PX, color, weight: FontWeight::Normal });
    surface.set_clip(None);
}

/// Draw a single glyph centered inside `rect` (used for the back button's
/// `<` arrow) — same vertical centering as `draw_left_aligned_clipped`, but
/// horizontally centered too since it's a single fixed-width glyph in a
/// square box rather than a left-flowing text run.
fn draw_centered_glyph(surface: &mut dyn Surface, rect: Rect, ch: char, color: Color) {
    const CELL_W: i32 = 8; // TerminusFont's 16px-bucket cell width (same as the old vga_8x16 cell).
    let font = TerminusFont::new();
    // Integer centering (see draw_left_aligned_clipped): float math here
    // diverges between the i486 (x87) and host (SSE) targets and would break
    // the chrome golden's host==i486 byte-identity.
    let ascent = Metrics::ascent(&font, TEXT_SIZE_PX) as i32;
    let line_h = Metrics::line_height(&font, TEXT_SIZE_PX) as i32;
    let text_top = rect.y + (rect.h as i32 - line_h).max(0) / 2;
    let baseline = text_top + ascent;
    let x = rect.x + (rect.w as i32 - CELL_W).max(0) / 2;

    let mut buf = [0u8; 4];
    let s = ch.encode_utf8(&mut buf);
    surface.set_clip(Some(rect));
    surface.draw_text(&TextRun { text: s, x, baseline, size_px: TEXT_SIZE_PX, color, weight: FontWeight::Normal });
    surface.set_clip(None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::MemSurface;

    // ------------------------------------------------------------- layout

    #[test]
    fn layout_normal_window_matches_the_spec_geometry() {
        let lay = layout(1024, 768);
        assert_eq!(lay.top, Rect { x: 0, y: 0, w: 1024, h: TOP_H });
        assert_eq!(lay.status.h, STATUS_H);
        assert_eq!(lay.status.y, (768 - STATUS_H) as i32);
        assert_eq!(lay.viewport.y, TOP_H as i32);
        assert_eq!(lay.viewport.h, 768 - TOP_H - STATUS_H);
        assert_eq!(lay.viewport.w, 1024);
    }

    #[test]
    fn layout_bars_and_viewport_never_overlap_for_a_normal_window() {
        let lay = layout(1024, 768);
        // top ends where viewport begins; viewport ends where status begins.
        assert_eq!(lay.top.y + lay.top.h as i32, lay.viewport.y);
        assert_eq!(lay.viewport.y + lay.viewport.h as i32, lay.status.y);
    }

    #[test]
    fn layout_back_address_throbber_sit_inside_the_top_bar() {
        let lay = layout(1024, 768);
        assert!(lay.back.y >= lay.top.y);
        assert!(lay.back.y + lay.back.h as i32 <= lay.top.y + lay.top.h as i32);
        assert!(lay.throbber.x + lay.throbber.w as i32 <= lay.top.w as i32);
        // Address sits between back and throbber.
        assert!(lay.address.x >= lay.back.x + lay.back.w as i32);
        assert!(lay.address.x + lay.address.w as i32 <= lay.throbber.x);
    }

    #[test]
    fn layout_tiny_window_never_panics_and_yields_non_negative_sizes() {
        for (w, h) in [(10u32, 10u32), (0, 0), (1, 1), (5, 40), (40, 5), (3, 3)] {
            let lay = layout(w, h);
            // u32 fields can't be negative by type; computing this without
            // panicking (no underflow) is itself most of the assertion.
            // What's worth checking on top: every rect stays within the
            // window bounds, and the three vertically-stacked bands
            // (top/viewport/status) never claim more height than the
            // window actually has.
            for r in [lay.top, lay.back, lay.address, lay.throbber, lay.viewport, lay.status] {
                assert!(r.w <= w, "rect width {} exceeds window width {} for ({w},{h})", r.w, w);
                assert!(r.h <= h, "rect height {} exceeds window height {} for ({w},{h})", r.h, h);
            }
            assert!(
                lay.top.h + lay.status.h + lay.viewport.h <= h,
                "top+status+viewport height exceeds window height {h} for ({w},{h}): {:?}",
                lay
            );
        }
    }

    // --------------------------------------------------------------- draw

    fn bar_pixel(s: &MemSurface, x: i32, y: i32) -> (u8, u8, u8, u8) {
        let (w, _h) = s.size();
        let i = ((y as usize) * (w as usize) + (x as usize)) * 4;
        let b = s.bytes();
        (b[i], b[i + 1], b[i + 2], b[i + 3])
    }

    #[test]
    fn draw_paints_the_top_bar_color_in_the_middle_of_the_top_bar() {
        let (w, h) = (300u32, 200u32);
        let lay = layout(w, h);
        let mut s = MemSurface::new(w, h, Color::rgba(0, 0, 0, 0));
        let st = ChromeState { url: "http://example.test/", status: "Done", loading: false, throbber_frame: 0, can_go_back: false };
        draw(&mut s, &lay, &st);

        // A point in the top bar away from the back/address/throbber boxes:
        // just use the top bar's vertical center, horizontal middle of the
        // window (the address field's own background is white, so probe
        // just below the bar's top edge, left of the address field... use
        // the known bar color check on a pixel we know isn't inside any
        // sub-widget: y=0 (the very top row) is always bar or a widget's
        // own top row; instead check a corner that's outside every widget).
        let (r, g, b, a) = bar_pixel(&s, (w - 1) as i32, 0);
        assert_eq!((r, g, b, a), (221, 221, 221, 255), "top-right corner (outside back/address/throbber) should be the bar color");
    }

    #[test]
    fn draw_address_field_is_white_or_dark_ink_never_untouched() {
        let (w, h) = (300u32, 200u32);
        let lay = layout(w, h);
        let mut s = MemSurface::new(w, h, Color::rgba(0, 0, 0, 0));
        let st = ChromeState { url: "http://example.test/", status: "", loading: false, throbber_frame: 0, can_go_back: true };
        draw(&mut s, &lay, &st);

        assert!(lay.address.w > 0 && lay.address.h > 0, "test window should yield a real address field");
        let mid_y = lay.address.y + lay.address.h as i32 / 2;
        let mut saw_non_background = false;
        for x in lay.address.x..(lay.address.x + lay.address.w as i32) {
            let px = bar_pixel(&s, x, mid_y);
            assert_ne!(px, (0, 0, 0, 0), "address field pixel at ({x},{mid_y}) is untouched (still fully transparent)");
            if px != (255, 255, 255, 255) {
                saw_non_background = true;
            }
        }
        assert!(saw_non_background, "expected at least one dark-ink pixel from the url text somewhere in the address field");
    }

    #[test]
    fn draw_a_hostile_long_url_does_not_spill_outside_the_address_field() {
        let (w, h) = (300u32, 200u32);
        let lay = layout(w, h);
        let mut s = MemSurface::new(w, h, Color::rgba(0, 0, 0, 0));
        let long_url: String = std::iter::repeat("http://example.test/very/long/path/segment/")
            .take(50)
            .collect();
        let st = ChromeState { url: &long_url, status: "", loading: false, throbber_frame: 0, can_go_back: true };
        draw(&mut s, &lay, &st);

        // Just past the address field's right edge, inside the top bar
        // (not inside the throbber box): must still be the plain bar color,
        // never text ink, even though the url is far longer than the field.
        let probe_x = lay.address.x + lay.address.w as i32 + 1;
        if probe_x < lay.throbber.x {
            let mid_y = lay.address.y + lay.address.h as i32 / 2;
            let px = bar_pixel(&s, probe_x, mid_y);
            assert_eq!(px, (221, 221, 221, 255), "pixel just outside the address field must stay the bar color, not spill-over ink");
        }
    }

    #[test]
    fn draw_never_touches_the_viewport_region() {
        let (w, h) = (300u32, 200u32);
        let lay = layout(w, h);
        let mut s = MemSurface::new(w, h, Color::rgba(0, 0, 0, 0));
        let long_status: String = std::iter::repeat('x').take(500).collect();
        let st = ChromeState { url: "http://example.test/", status: &long_status, loading: true, throbber_frame: 7, can_go_back: true };
        draw(&mut s, &lay, &st);

        assert!(lay.viewport.h > 0, "test window should yield a real viewport");
        for y in [lay.viewport.y, lay.viewport.y + lay.viewport.h as i32 / 2, lay.viewport.y + lay.viewport.h as i32 - 1] {
            for x in [0, w as i32 / 2, w as i32 - 1] {
                let px = bar_pixel(&s, x, y);
                assert_eq!(px, (0, 0, 0, 0), "viewport pixel at ({x},{y}) should stay whatever the surface was initialized to (untouched)");
            }
        }
    }

    #[test]
    fn draw_never_panics_on_a_tiny_or_degenerate_window() {
        for (w, h) in [(10u32, 10u32), (0, 0), (1, 1), (2, 2)] {
            let lay = layout(w, h);
            let mut s = MemSurface::new(w.max(1), h.max(1), Color::WHITE);
            let st = ChromeState { url: "x", status: "y", loading: true, throbber_frame: 200, can_go_back: false };
            draw(&mut s, &lay, &st);
        }
    }

    #[test]
    fn draw_empty_strings_and_all_throbber_frames_never_panic() {
        let (w, h) = (300u32, 200u32);
        let lay = layout(w, h);
        for frame in 0..=255u8 {
            let mut s = MemSurface::new(w, h, Color::WHITE);
            let st = ChromeState { url: "", status: "", loading: frame % 2 == 0, throbber_frame: frame, can_go_back: frame % 3 == 0 };
            draw(&mut s, &lay, &st);
        }
    }
}
