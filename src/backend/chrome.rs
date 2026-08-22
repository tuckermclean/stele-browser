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
    /// The forward-navigation button (`packet/chrome-ux-fixes`): a
    /// roughly-square box immediately right of `back` -- same size/clamp
    /// discipline, mirroring it exactly except for the glyph `draw` paints
    /// and which `ChromeState` flag (`can_go_forward` vs `can_go_back`) it
    /// dims on. `reload`'s own left bound (below) is now computed against
    /// THIS rect instead of `back` directly, since it sits in between.
    pub forward: Rect,
    /// The reload button (`packet/chrome-address-edit`): a roughly-square
    /// box immediately right of `forward` — all three are "page navigation"
    /// actions, grouped left, matching the back/forward-then-address
    /// reading order most browsers use. Clicking it re-runs the SAME
    /// load/redraw sequence `XIntent::Reload`/`F5` already trigger
    /// (`run_x11`) — this rect is purely a second, discoverable trigger for
    /// that existing logic, not new reload machinery.
    pub reload: Rect,
    /// The address field, the space between `reload` and `attest`.
    pub address: Rect,
    /// The throbber/load-indicator, a roughly-square box at the right of `top`.
    pub throbber: Rect,
    /// The attestations affordance (packet/attestation-modal): a small
    /// square immediately left of `throbber`, clicking it navigates to
    /// `about:attestations` (wired in `run_x11`, this struct only carries
    /// its geometry).
    pub attest: Rect,
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

    const GAP: i64 = 4;

    // Forward button (packet/chrome-ux-fixes): mirrors `back` exactly --
    // same ~24px-square size/clamp discipline, sitting immediately right of
    // it. `reload`'s own left bound (below) is computed against THIS rect,
    // not `back` directly, now that it sits in between.
    let forward_size = 24u32.min(top_h.saturating_sub(4)).min(win_w.saturating_sub(4));
    let forward_x = (back.x as i64 + back.w as i64 + GAP) as i32;
    let forward_y = INSET + ((top_h.saturating_sub(4).saturating_sub(forward_size)) / 2) as i32;
    let forward = Rect { x: forward_x, y: forward_y, w: forward_size, h: forward_size };

    // Reload button (packet/chrome-address-edit): another ~20px square,
    // immediately right of `forward` -- same size/clamp discipline as
    // `attest`/`throbber`. `address`'s left bound (below) is computed
    // against THIS rect, not `back`/`forward` directly, now that they sit
    // in between.
    let reload_size = 20u32.min(top_h.saturating_sub(4)).min(win_w.saturating_sub(4));
    let reload_x = (forward.x as i64 + forward.w as i64 + GAP) as i32;
    let reload_y = INSET + ((top_h.saturating_sub(4).saturating_sub(reload_size)) / 2) as i32;
    let reload = Rect { x: reload_x, y: reload_y, w: reload_size, h: reload_size };

    // Attestations button: another ~20px square, immediately left of the
    // throbber (same size/clamp discipline) -- packet/attestation-modal.
    // Sits between `address` and `throbber`, so `address`'s right bound
    // below is computed against THIS rect, not `throbber` directly.
    let attest_size = 20u32.min(top_h.saturating_sub(4)).min(win_w.saturating_sub(4));
    let attest_x = (throbber.x as i64 - GAP - attest_size as i64).max(0) as i32;
    let attest_y = INSET + ((top_h.saturating_sub(4).saturating_sub(attest_size)) / 2) as i32;
    let attest = Rect { x: attest_x, y: attest_y, w: attest_size, h: attest_size };

    // Address field: whatever's left between `reload` (which itself sits
    // right of `back`) and `attest` (which sits left of `throbber`),
    // clamped to never go negative-width (a tiny window can make these
    // boxes overlap or even swap order — the field just collapses to zero
    // rather than underflowing).
    let addr_x = reload.x as i64 + reload.w as i64 + GAP;
    let addr_right = attest.x as i64 - GAP;
    let addr_w = (addr_right - addr_x).max(0) as u32;
    let address = Rect { x: addr_x as i32, y: back.y, w: addr_w, h: back.h };

    ChromeLayout { top, back, forward, reload, address, throbber, attest, viewport, status }
}

/// A snapshot of the interactive state `draw` needs to paint one frame.
/// Owned/updated by `run_x11` (T3); this struct itself carries no behavior.
pub struct ChromeState<'a> {
    /// The current page URL, shown display-only in the address field when
    /// `edit` is `None`.
    pub url: &'a str,
    /// `packet/chrome-address-edit`: `Some((live buffer, cursor char-index))`
    /// while the address bar is focused/being edited, `None` otherwise. When
    /// `Some`, `draw_address` renders the LIVE buffer + a cursor caret
    /// instead of `url` — `url` itself is untouched either way (it always
    /// tracks the real current, navigated page; the caller, `run_x11`, never
    /// needs to "restore" it because it was never overwritten by an in-
    /// progress edit).
    pub edit: Option<(&'a str, usize)>,
    /// The status line, shown in the bottom status bar.
    pub status: &'a str,
    /// Whether a page fetch is in flight (drives the throbber's animated frame).
    pub loading: bool,
    /// Which throbber frame to draw while `loading` (`% N` inside `draw`).
    pub throbber_frame: u8,
    /// Whether the back button should render enabled (a non-empty history stack).
    pub can_go_back: bool,
    /// `packet/chrome-ux-fixes`: whether the forward button should render
    /// enabled (a non-empty `History::forward` stack) -- the mirror of
    /// `can_go_back` for the new forward button.
    pub can_go_forward: bool,
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
    draw_forward_button(surface, lay.forward, st.can_go_forward);
    draw_reload_button(surface, lay.reload);
    draw_address(surface, lay.address, st.url, st.edit);
    draw_attest_button(surface, lay.attest);
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

/// The forward-navigation button (`packet/chrome-ux-fixes`): mirrors
/// `draw_back_button` exactly, dimmed on `!can_go_forward` the same way
/// `back` dims on `!can_go_back`. Glyph `'>'` -- a plain, already-embedded
/// ASCII char (zero new glyph-atlas bytes), the mirror image of `back`'s
/// `'<'`.
fn draw_forward_button(surface: &mut dyn Surface, rect: Rect, can_go_forward: bool) {
    if rect.w == 0 || rect.h == 0 {
        return;
    }
    let (box_color, ink) = if can_go_forward { (BUTTON_COLOR, INK) } else { (BUTTON_DISABLED_COLOR, INK_DIMMED) };
    surface.fill_rect(rect, box_color);
    draw_centered_glyph(surface, rect, '>', ink);
}

/// The attestations affordance (packet/attestation-modal): always painted
/// "enabled" (unlike `back`, `about:attestations` is always navigable, no
/// disabled state to represent) -- a small box with a centered "\u{00A9}"
/// (copyright sign, within the embedded Terminus subset's Latin-1 range)
/// standing in for "about this build". Click wiring lives in `run_x11`
/// (manual-verify, same posture as `back`'s own click handling); this
/// function only paints.
fn draw_attest_button(surface: &mut dyn Surface, rect: Rect) {
    if rect.w == 0 || rect.h == 0 {
        return;
    }
    surface.fill_rect(rect, BUTTON_COLOR);
    draw_centered_glyph(surface, rect, '\u{00A9}', INK);
}

/// The reload button (`packet/chrome-address-edit`): always painted
/// "enabled" (like `attest`, unlike `back` — reloading the current page is
/// always a valid action, no disabled state to represent). Glyph `'R'` — a
/// plain, already-embedded ASCII capital letter (zero new glyph-atlas
/// bytes), the leanest option that's still legible next to `back`'s `'<'`
/// and `attest`'s `'\u{00A9}'`. Click wiring lives in `run_x11` (manual-
/// verify, same posture as `back`'s own click handling) and reuses
/// `XIntent::Reload`'s existing load/redraw logic — this function only
/// paints.
fn draw_reload_button(surface: &mut dyn Surface, rect: Rect) {
    if rect.w == 0 || rect.h == 0 {
        return;
    }
    surface.fill_rect(rect, BUTTON_COLOR);
    draw_centered_glyph(surface, rect, 'R', INK);
}

/// Left padding inside the address field before text/caret starts — shared
/// between [`draw_left_aligned_clipped`] (the text) and [`draw_address`]'s
/// own caret offset math, so the two never drift apart.
const ADDRESS_PAD_X: i32 = 4;

/// Cursor-caret fill color — same ink as the text itself.
const CARET_COLOR: Color = INK;

/// `packet/chrome-address-edit`: `edit` branches this between the live,
/// focused edit buffer (+ a cursor caret) and today's exact unfocused
/// behavior (draw `url`, byte-identical to before this packet — `edit:
/// None` never touches the caret code path at all).
fn draw_address(surface: &mut dyn Surface, rect: Rect, url: &str, edit: Option<(&str, usize)>) {
    if rect.w == 0 || rect.h == 0 {
        return;
    }
    surface.fill_rect(rect, Color::WHITE);
    match edit {
        None => draw_left_aligned_clipped(surface, rect, url, INK),
        Some((buf, cursor)) => {
            draw_left_aligned_clipped(surface, rect, buf, INK);
            draw_address_caret(surface, rect, buf, cursor);
        }
    }
}

/// Paint a 2px-wide vertical caret at the pixel x-offset corresponding to
/// char index `cursor` into `buf` — computed via `Metrics::measure` on the
/// prefix `buf[..cursor]` chars, the SAME per-char-advance machinery
/// `draw_left_aligned_clipped`'s own `TextRun` painting already uses, so the
/// caret always lands exactly where the text it's next to was drawn (no
/// separate/duplicated advance table to drift out of sync). Clipped to
/// `rect` exactly like the text itself -- a caret past the field's visible
/// right edge is simply invisible (no horizontal scroll-within-field, a
/// flagged, accepted MVP simplification, design doc §5), never a panic or a
/// paint outside the field.
fn draw_address_caret(surface: &mut dyn Surface, rect: Rect, buf: &str, cursor: usize) {
    const CARET_W: u32 = 2;
    let font = TerminusFont::new();
    let prefix: String = buf.chars().take(cursor).collect();
    let offset = Metrics::measure(&font, &prefix, TEXT_SIZE_PX) as i32;
    let caret_x = rect.x + ADDRESS_PAD_X + offset;

    surface.set_clip(Some(rect));
    surface.fill_rect(Rect { x: caret_x, y: rect.y, w: CARET_W, h: rect.h }, CARET_COLOR);
    surface.set_clip(None);
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
    // packet/chrome-address-edit: shared with draw_address_caret's own
    // offset math (both must agree on where text starts inside the field).
    let pad_x = ADDRESS_PAD_X;
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
    surface.draw_text(&TextRun { text, x: rect.x + pad_x, baseline, size_px: TEXT_SIZE_PX, color, weight: FontWeight::Normal });
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

    /// `packet/chrome-ux-fixes` — mirrors
    /// `layout_reload_button_has_nonzero_size_and_does_not_overlap_siblings`.
    #[test]
    fn layout_forward_button_has_nonzero_size_and_does_not_overlap_siblings() {
        let lay = layout(1024, 768);
        assert!(lay.forward.w > 0 && lay.forward.h > 0, "forward rect: {:?}", lay.forward);

        fn overlaps(a: Rect, b: Rect) -> bool {
            a.x < b.x + b.w as i32
                && b.x < a.x + a.w as i32
                && a.y < b.y + b.h as i32
                && b.y < a.y + a.h as i32
        }
        assert!(!overlaps(lay.forward, lay.back), "forward overlaps back: {:?} / {:?}", lay.forward, lay.back);
        assert!(!overlaps(lay.forward, lay.reload), "forward overlaps reload: {:?} / {:?}", lay.forward, lay.reload);
        assert!(!overlaps(lay.forward, lay.address), "forward overlaps address: {:?} / {:?}", lay.forward, lay.address);
        assert!(!overlaps(lay.forward, lay.attest), "forward overlaps attest: {:?} / {:?}", lay.forward, lay.attest);
        assert!(!overlaps(lay.forward, lay.throbber), "forward overlaps throbber: {:?} / {:?}", lay.forward, lay.throbber);
    }

    /// `packet/chrome-ux-fixes` — mirrors
    /// `layout_reload_button_tiny_window_never_panics_and_stays_in_bounds`.
    #[test]
    fn layout_forward_button_tiny_window_never_panics_and_stays_in_bounds() {
        for (w, h) in [(10u32, 10u32), (0, 0), (1, 1), (5, 40), (40, 5), (3, 3)] {
            let lay = layout(w, h);
            assert!(lay.forward.w <= w, "forward width {} exceeds window width {w}", lay.forward.w);
            assert!(lay.forward.h <= h, "forward height {} exceeds window height {h}", lay.forward.h);
        }
    }

    /// Confirms the recompute, not just that `forward` exists in isolation:
    /// `reload`'s left bound must now sit at or past `forward`'s right edge.
    #[test]
    fn layout_reload_left_bound_now_starts_after_forward() {
        let lay = layout(1024, 768);
        assert!(lay.reload.x >= lay.forward.x + lay.forward.w as i32, "reload {:?} starts before forward {:?} ends", lay.reload, lay.forward);
    }

    /// packet/chrome-address-edit, Task 4 — mirrors
    /// `layout_attest_button_has_nonzero_size_and_does_not_overlap_siblings`.
    #[test]
    fn layout_reload_button_has_nonzero_size_and_does_not_overlap_siblings() {
        let lay = layout(1024, 768);
        assert!(lay.reload.w > 0 && lay.reload.h > 0, "reload rect: {:?}", lay.reload);

        fn overlaps(a: Rect, b: Rect) -> bool {
            a.x < b.x + b.w as i32
                && b.x < a.x + a.w as i32
                && a.y < b.y + b.h as i32
                && b.y < a.y + a.h as i32
        }
        assert!(!overlaps(lay.reload, lay.back), "reload overlaps back: {:?} / {:?}", lay.reload, lay.back);
        assert!(!overlaps(lay.reload, lay.address), "reload overlaps address: {:?} / {:?}", lay.reload, lay.address);
        assert!(!overlaps(lay.reload, lay.attest), "reload overlaps attest: {:?} / {:?}", lay.reload, lay.attest);
        assert!(!overlaps(lay.reload, lay.throbber), "reload overlaps throbber: {:?} / {:?}", lay.reload, lay.throbber);
    }

    /// packet/chrome-address-edit, Task 4 — mirrors
    /// `layout_attest_button_tiny_window_never_panics_and_stays_in_bounds`.
    #[test]
    fn layout_reload_button_tiny_window_never_panics_and_stays_in_bounds() {
        for (w, h) in [(10u32, 10u32), (0, 0), (1, 1), (5, 40), (40, 5), (3, 3)] {
            let lay = layout(w, h);
            assert!(lay.reload.w <= w, "reload width {} exceeds window width {w}", lay.reload.w);
            assert!(lay.reload.h <= h, "reload height {} exceeds window height {h}", lay.reload.h);
        }
    }

    /// Confirms the recompute, not just that `reload` exists in isolation:
    /// `address`'s left bound must now sit at or past `reload`'s right edge.
    #[test]
    fn layout_address_left_bound_now_starts_after_reload() {
        let lay = layout(1024, 768);
        assert!(lay.address.x >= lay.reload.x + lay.reload.w as i32, "address {:?} starts before reload {:?} ends", lay.address, lay.reload);
    }

    #[test]
    fn layout_attest_button_has_nonzero_size_and_does_not_overlap_siblings() {
        let lay = layout(1024, 768);
        assert!(lay.attest.w > 0 && lay.attest.h > 0, "attest rect: {:?}", lay.attest);

        fn overlaps(a: Rect, b: Rect) -> bool {
            a.x < b.x + b.w as i32
                && b.x < a.x + a.w as i32
                && a.y < b.y + b.h as i32
                && b.y < a.y + a.h as i32
        }
        assert!(!overlaps(lay.attest, lay.back), "attest overlaps back: {:?} / {:?}", lay.attest, lay.back);
        assert!(!overlaps(lay.attest, lay.address), "attest overlaps address: {:?} / {:?}", lay.attest, lay.address);
        assert!(!overlaps(lay.attest, lay.throbber), "attest overlaps throbber: {:?} / {:?}", lay.attest, lay.throbber);
    }

    #[test]
    fn layout_attest_button_tiny_window_never_panics_and_stays_in_bounds() {
        for (w, h) in [(10u32, 10u32), (0, 0), (1, 1), (5, 40), (40, 5), (3, 3)] {
            let lay = layout(w, h);
            assert!(lay.attest.w <= w, "attest width {} exceeds window width {w}", lay.attest.w);
            assert!(lay.attest.h <= h, "attest height {} exceeds window height {h}", lay.attest.h);
        }
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
            for r in [lay.top, lay.back, lay.forward, lay.reload, lay.address, lay.throbber, lay.viewport, lay.status] {
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
        let st = ChromeState { url: "http://example.test/", edit: None, status: "Done", loading: false, throbber_frame: 0, can_go_back: false, can_go_forward: false };
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
        let st = ChromeState { url: "http://example.test/", edit: None, status: "", loading: false, throbber_frame: 0, can_go_back: true, can_go_forward: true };
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
        let st = ChromeState { url: &long_url, edit: None, status: "", loading: false, throbber_frame: 0, can_go_back: true, can_go_forward: true };
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

    /// packet/chrome-address-edit, Task 5: focused rendering shows the LIVE
    /// buffer (not `url`) plus a caret at the pixel offset independently
    /// computed the same way `draw_address_caret` does.
    #[test]
    fn draw_address_focused_shows_live_buffer_and_caret_at_expected_offset() {
        let (w, h) = (300u32, 200u32);
        let lay = layout(w, h);
        let mut s = MemSurface::new(w, h, Color::rgba(0, 0, 0, 0));
        let buf = "http://x/";
        let cursor = 5;
        let st = ChromeState { url: "http://unused/", edit: Some((buf, cursor)), status: "", loading: false, throbber_frame: 0, can_go_back: true, can_go_forward: true };
        draw(&mut s, &lay, &st);

        assert!(lay.address.w > 0 && lay.address.h > 0, "test window should yield a real address field");
        let mid_y = lay.address.y + lay.address.h as i32 / 2;

        let mut saw_non_background = false;
        for x in lay.address.x..(lay.address.x + lay.address.w as i32) {
            let px = bar_pixel(&s, x, mid_y);
            if px != (255, 255, 255, 255) {
                saw_non_background = true;
            }
        }
        assert!(saw_non_background, "expected the live buffer's ink somewhere in the address field");

        // Independently compute the expected caret x-offset (same formula
        // draw_address_caret uses) and confirm a caret-colored pixel column
        // actually lands there, not just "a caret exists somewhere".
        let font = TerminusFont::new();
        let prefix: String = buf.chars().take(cursor).collect();
        let expected_offset = Metrics::measure(&font, &prefix, TEXT_SIZE_PX) as i32;
        let expected_x = lay.address.x + ADDRESS_PAD_X + expected_offset;

        let mut saw_caret = false;
        for y in lay.address.y..(lay.address.y + lay.address.h as i32) {
            if bar_pixel(&s, expected_x, y) == (17, 17, 17, 255) {
                saw_caret = true;
                break;
            }
        }
        assert!(saw_caret, "expected a caret-colored pixel column at the computed x-offset {expected_x}");
    }

    /// Mirrors `draw_a_hostile_long_url_does_not_spill_outside_the_address_field`,
    /// now exercised through the `edit` path instead of `url`.
    #[test]
    fn draw_a_hostile_long_edit_buffer_does_not_spill_outside_the_address_field() {
        let (w, h) = (300u32, 200u32);
        let lay = layout(w, h);
        let mut s = MemSurface::new(w, h, Color::rgba(0, 0, 0, 0));
        let long_buf: String = std::iter::repeat("http://example.test/very/long/path/segment/")
            .take(50)
            .collect();
        let cursor = long_buf.chars().count();
        let st = ChromeState { url: "http://unused/", edit: Some((&long_buf, cursor)), status: "", loading: false, throbber_frame: 0, can_go_back: true, can_go_forward: true };
        draw(&mut s, &lay, &st);

        let probe_x = lay.address.x + lay.address.w as i32 + 1;
        if probe_x < lay.throbber.x {
            let mid_y = lay.address.y + lay.address.h as i32 / 2;
            let px = bar_pixel(&s, probe_x, mid_y);
            assert_eq!(px, (221, 221, 221, 255), "pixel just outside the address field must stay the bar color, not spill-over ink or caret");
        }
    }

    /// Edge case: a freshly-focused field over a blank seed (empty buffer,
    /// cursor 0) still draws a caret at the field's left edge, no panic.
    #[test]
    fn draw_address_empty_focused_buffer_draws_a_caret_at_the_left_edge_without_panicking() {
        let (w, h) = (300u32, 200u32);
        let lay = layout(w, h);
        let mut s = MemSurface::new(w, h, Color::rgba(0, 0, 0, 0));
        let st = ChromeState { url: "http://unused/", edit: Some(("", 0)), status: "", loading: false, throbber_frame: 0, can_go_back: true, can_go_forward: true };
        draw(&mut s, &lay, &st);

        assert!(lay.address.w > 0 && lay.address.h > 0);
        let expected_x = lay.address.x + ADDRESS_PAD_X;
        let mid_y = lay.address.y + lay.address.h as i32 / 2;
        assert_eq!(bar_pixel(&s, expected_x, mid_y), (17, 17, 17, 255), "expected the caret at the field's left edge for an empty buffer");
    }

    #[test]
    fn draw_never_touches_the_viewport_region() {
        let (w, h) = (300u32, 200u32);
        let lay = layout(w, h);
        let mut s = MemSurface::new(w, h, Color::rgba(0, 0, 0, 0));
        let long_status: String = std::iter::repeat('x').take(500).collect();
        let st = ChromeState { url: "http://example.test/", edit: None, status: &long_status, loading: true, throbber_frame: 7, can_go_back: true, can_go_forward: true };
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
    fn draw_attest_button_guards_zero_size_rect_without_panicking() {
        let mut s = MemSurface::new(1, 1, Color::WHITE);
        draw_attest_button(&mut s, Rect { x: 0, y: 0, w: 0, h: 0 });
    }

    /// packet/chrome-address-edit, Task 4 — mirrors
    /// `draw_attest_button_guards_zero_size_rect_without_panicking`.
    #[test]
    fn draw_reload_button_guards_zero_size_rect_without_panicking() {
        let mut s = MemSurface::new(1, 1, Color::WHITE);
        draw_reload_button(&mut s, Rect { x: 0, y: 0, w: 0, h: 0 });
    }

    /// `packet/chrome-ux-fixes` — mirrors
    /// `draw_reload_button_guards_zero_size_rect_without_panicking`.
    #[test]
    fn draw_forward_button_guards_zero_size_rect_without_panicking() {
        let mut s = MemSurface::new(1, 1, Color::WHITE);
        draw_forward_button(&mut s, Rect { x: 0, y: 0, w: 0, h: 0 }, true);
        draw_forward_button(&mut s, Rect { x: 0, y: 0, w: 0, h: 0 }, false);
    }

    /// Confirms the glyph actually painted, not just that the rect exists —
    /// mirrors `draw_address_field_is_white_or_dark_ink_never_untouched`'s
    /// probe style.
    #[test]
    fn draw_paints_a_non_background_pixel_inside_the_reload_button() {
        let (w, h) = (300u32, 200u32);
        let lay = layout(w, h);
        let mut s = MemSurface::new(w, h, Color::rgba(0, 0, 0, 0));
        let st = ChromeState { url: "http://example.test/", edit: None, status: "", loading: false, throbber_frame: 0, can_go_back: true, can_go_forward: true };
        draw(&mut s, &lay, &st);

        assert!(lay.reload.w > 0 && lay.reload.h > 0, "test window should yield a real reload button");
        let mut saw_non_button_color = false;
        for y in lay.reload.y..(lay.reload.y + lay.reload.h as i32) {
            for x in lay.reload.x..(lay.reload.x + lay.reload.w as i32) {
                let px = bar_pixel(&s, x, y);
                if px != (200, 200, 200, 255) {
                    saw_non_button_color = true;
                }
            }
        }
        assert!(saw_non_button_color, "expected the 'R' glyph to paint at least one non-button-color pixel inside the reload rect");
    }

    /// `packet/chrome-ux-fixes`: confirms the forward button dims like
    /// `back` does — enabled uses `BUTTON_COLOR`/`INK`, disabled uses
    /// `BUTTON_DISABLED_COLOR`/`INK_DIMMED`, and the two box fills differ.
    #[test]
    fn draw_forward_button_dims_when_can_go_forward_is_false() {
        let (w, h) = (300u32, 200u32);
        let lay = layout(w, h);

        let mut enabled = MemSurface::new(w, h, Color::rgba(0, 0, 0, 0));
        let st_enabled = ChromeState { url: "http://example.test/", edit: None, status: "", loading: false, throbber_frame: 0, can_go_back: true, can_go_forward: true };
        draw(&mut enabled, &lay, &st_enabled);

        let mut disabled = MemSurface::new(w, h, Color::rgba(0, 0, 0, 0));
        let st_disabled = ChromeState { url: "http://example.test/", edit: None, status: "", loading: false, throbber_frame: 0, can_go_back: true, can_go_forward: false };
        draw(&mut disabled, &lay, &st_disabled);

        assert!(lay.forward.w > 0 && lay.forward.h > 0, "test window should yield a real forward button");
        // The box fill itself (a corner pixel, away from the glyph) must
        // differ between enabled/disabled.
        let corner_enabled = bar_pixel(&enabled, lay.forward.x, lay.forward.y);
        let corner_disabled = bar_pixel(&disabled, lay.forward.x, lay.forward.y);
        assert_ne!(corner_enabled, corner_disabled, "enabled/disabled forward button box fill must differ");
    }

    #[test]
    fn draw_never_panics_on_a_tiny_or_degenerate_window() {
        for (w, h) in [(10u32, 10u32), (0, 0), (1, 1), (2, 2)] {
            let lay = layout(w, h);
            let mut s = MemSurface::new(w.max(1), h.max(1), Color::WHITE);
            let st = ChromeState { url: "x", edit: None, status: "y", loading: true, throbber_frame: 200, can_go_back: false, can_go_forward: false };
            draw(&mut s, &lay, &st);
        }
    }

    #[test]
    fn draw_empty_strings_and_all_throbber_frames_never_panic() {
        let (w, h) = (300u32, 200u32);
        let lay = layout(w, h);
        for frame in 0..=255u8 {
            let mut s = MemSurface::new(w, h, Color::WHITE);
            let st = ChromeState { url: "", edit: None, status: "", loading: frame % 2 == 0, throbber_frame: frame, can_go_back: frame % 3 == 0, can_go_forward: frame % 5 == 0 };
            draw(&mut s, &lay, &st);
        }
    }
}
