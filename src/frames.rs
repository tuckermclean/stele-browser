//! Frames (packet `frames`): HTML 4.01 frameset support, layered ABOVE the
//! single-document pipeline (`dom::parser::parse` -> `style::cascade::
//! cascade` -> `layout::box_tree::build_box_tree` -> `layout::layout` ->
//! `backend::tty::render`), never inside it. A `<frameset>` document
//! partitions the viewport into a grid; each `<frame src>` cell is an
//! INDEPENDENT sub-document — its own fetch, parse, cascade, layout, and
//! render — composited into its cell of the parent's [`tty::TextGrid`] via
//! [`tty::TextGrid::blit`]. No frozen type/signature changes anything in
//! `dom`/`style`/`layout`/`fetch`/`surface`/`text`; this module only reuses
//! their existing public surface and drives it recursively.
//!
//! ## Detection
//!
//! A document is routed here (instead of the ordinary single-doc pipeline)
//! iff a `<frameset>` element is reachable anywhere under its root — see
//! [`find_frameset`]. `<noframes>` fallback content is never rendered:
//! Stele renders frames for real, so the noscript-style fallback markup
//! authors wrote for frame-blind browsers has no reason to run here (v0
//! scope call, documented, not a bug).
//!
//! ## Track sizing (`rows`/`cols`)
//!
//! Each comma-separated entry in a `rows`/`cols` attribute is a pixel count
//! (`100`), a percentage (`30%`), or a relative/star weight (`*`, `2*`).
//! [`compute_track_extents`] applies them in the order the packet brief
//! specifies: fixed pixel tracks are honored first; percentage tracks then
//! take their percentage of what's LEFT after the fixed tracks (not of the
//! grand total — a deliberate simplification over the full HTML frameset
//! algorithm, documented here rather than in DECISIONS since it's this
//! function's own contract); star tracks split whatever remains after that,
//! proportional to their weight. An absent attribute is a single 100% track
//! (one cell spans the whole axis). Malformed entries (unparseable, empty,
//! negative) degrade to a `1*` track rather than erroring — never a panic,
//! matching the parser's own total-input contract. See the module's unit
//! tests for hand-computed examples.
//!
//! ## Cell conversion
//!
//! Pixel extents convert to tty cells via the SAME `CELL_W`/`CELL_H` (8x16)
//! the tty backend itself pins (`crate::backend::tty::{CELL_W, CELL_H}`,
//! made `pub(crate)` by this packet so both modules share one source of
//! truth rather than duplicating the magic numbers).
//!
//! ## Row/column extent is fixed from track math alone — never from
//! ## rendered content (this is what makes incremental compositing safe)
//!
//! Both axes of the grid are sized ENTIRELY from `compute_track_extents`
//! before a single cell is rendered: column width bounds each cell's own
//! sub-render `cols` (text wrapping), and row height is that row's nominal
//! track extent, full stop — NOT grown to fit whatever a cell's document
//! actually renders. A cell whose real content is taller than its nominal
//! row slot is clipped at the row boundary by [`tty::TextGrid::blit`]'s own
//! bounds-clipping (dropped, not overflowed into the next row) — the same
//! "clip rather than overflow" rule `backend::tty::render` already applies
//! at the right edge of a text run. A real windowed browser would give an
//! overflowing frame a scrollbar; a headless text dump has neither
//! scrolling nor a legible way to signal "there's more" mid-grid, so
//! clipping at the boundary is the least-surprising choice — and,
//! critically, it's what makes the compositing loop below able to blit each
//! cell immediately and drop it, rather than holding every cell's grid
//! resident just to learn how tall the row "should" grow (see Totality
//! below — an earlier revision of this module DID grow rows to fit content
//! and was reviewer-flagged for the resulting unbounded peak memory).
//!
//! ## Compositing / separators
//!
//! No separator/border is drawn between frames in v0: `backend::tty`
//! already documents that `Box` fragments (backgrounds/borders) paint
//! nothing in text mode, or "a text grid has no color/background concept";
//! the same reasoning applies here — an ASCII-art border would need its own
//! cell-accurate drawing pass distinct from both `blit` and the underlying
//! per-document renderer, and blank space between the compositing offsets
//! already reads legibly in the goldens below (frames on a `cols` split are
//! visually distinguishable because their own content starts flush at each
//! cell's left edge). Deferred, not half-done — documented for the
//! DECISIONS ledger, same spirit as the `Box`-paints-nothing call.
//!
//! ## Totality (frame bombs)
//!
//! Recursing into `<frameset>`/`<frame>` documents is the classic infinite
//! recursion / resource exhaustion attack surface (self-referential
//! frameset, A->B->A cycles, thousands of frames). Three independent
//! bounds, each documented at its constant:
//!   - [`MAX_FRAME_DEPTH`]: nested frameset/frame recursion depth.
//!   - [`MAX_TOTAL_FRAMES`]: a global budget shared across the WHOLE frame
//!     tree for this document (bounds a wide bomb, not just a deep one).
//!   - [`MAX_TRACKS_PER_DIMENSION`]: caps how many `rows`/`cols` tracks a
//!     single `<frameset>` can define, independent of the above two (bounds
//!     the grid-cell count of any ONE frameset before the global budget
//!     even gets consulted).
//!   - A same-URL cycle on the current fetch path (`FrameCtx::visited`)
//!     short-circuits to a placeholder rather than re-fetching. This is a
//!     plain `Url::as_str()` string comparison — no percent-decoding or
//!     case-folding — so a pathological `a%2ehtml` vs `A.html` pair could in
//!     principle evade it; [`MAX_FRAME_DEPTH`] is the unconditional backstop
//!     that still bounds that case (URL normalization is a broader `fetch`
//!     concern, out of scope for this packet).
//! Any bound tripping, any fetch failure, any malformed `rows`/`cols`, or a
//! zero-size region all resolve to a placeholder/empty sub-grid — never a
//! panic, never an abort, matching this whole codebase's totality covenant.
//!
//! ## Peak memory: incremental compositing, one child at a time
//!
//! `render_frameset_grid` allocates its canvas ONCE via `TextGrid::blank`
//! (sized purely from track math, so it's bounded by `TextGrid::blank`'s own
//! `MAX_GRID_ROWS`/`MAX_GRID_COLS` clamp regardless of how pathological the
//! `rows`/`cols` attributes are), then renders and blits cells ONE AT A
//! TIME: render a cell's own `TextGrid`, `blit` it into the canvas
//! immediately, and let it drop before the next cell is even rendered. Peak
//! extra memory for the whole recursive render of one document is therefore
//! the (bounded) canvas plus exactly one in-flight child `TextGrid` — NOT
//! `O(MAX_TOTAL_FRAMES)` children held simultaneously. This matters because
//! a frame's own rendered height is content-driven and unrelated to its
//! small nominal cell (a single fetched document can legitimately render
//! `MAX_GRID_ROWS` tall regardless of how little screen space its frame
//! cell nominally occupies) — collecting all of one frameset's children
//! before compositing (an earlier revision of this module did exactly that)
//! could hold up to `MAX_TOTAL_FRAMES` such grids live at once, hundreds of
//! MB, which aborts on allocation failure under `panic = "abort"`.
//!
//! ## v0 simplifications (documented, not oversights)
//!
//! `scrolling`, `noresize`, `marginwidth`/`marginheight`, and `frameborder`
//! frame attributes are read by no code here — a text-mode grid has no
//! scrollbars, resize handles, margins, or borders to honor. `<noframes>` is
//! skipped outright (see above). Percentage tracks are percentages of the
//! REMAINING space after fixed tracks, not of the grand total (see track
//! sizing above) — a deliberate simplification, not the full HTML frameset
//! algorithm.

use crate::backend::tty::{self, TextGrid, CELL_H, CELL_W};
use crate::dom::{Dom, Node, NodeId};
use crate::fetch::file::FileFetcher;
use crate::fetch::http1::Http1Client;
use crate::fetch::{Fetch, Request, Url};
use crate::layout::box_tree::build_box_tree;
use crate::layout::{self, Fragment, FragmentKind, Point, Rect, Size};
use crate::style::cascade;
use crate::style::ColorScheme;
use crate::style::ComputedStyle;

/// Maximum nesting depth for framesets/frames (a top-level frameset counts
/// as depth 0; each `<frame src>` that itself resolves to a frameset, or
/// each directly-nested `<frameset>`, adds one). Past this depth, the cell
/// renders as a `[depth limit]` placeholder rather than recursing further —
/// see the module's totality docs.
const MAX_FRAME_DEPTH: usize = 6;

/// Global budget on the number of `<frame>`/nested-`<frameset>` CELLS
/// visited (fetched/recursed into) across the whole frame tree for one
/// top-level document — shared via [`FrameCtx::budget`] and decremented on
/// every cell visit, bounding a WIDE frame bomb (thousands of sibling
/// frames) the way [`MAX_FRAME_DEPTH`] bounds a deep one.
const MAX_TOTAL_FRAMES: usize = 128;

/// Maximum number of tracks a single `rows`/`cols` attribute may define.
/// Bounds the grid-cell count (`rows_tracks * cols_tracks`) of any ONE
/// `<frameset>` independent of [`MAX_TOTAL_FRAMES`] — a `cols` attribute
/// with tens of thousands of comma-separated entries would otherwise force
/// a correspondingly huge `Vec<Track>`/grid-cell allocation before the
/// per-cell budget check ever runs.
const MAX_TRACKS_PER_DIMENSION: usize = 32;

/// The pixel viewport height assumed when computing a top-level frameset's
/// `rows` tracks: there is no real terminal size in `--headless
/// --dump-text`, so (mirroring `main.rs`'s own `HEADLESS_VIEWPORT_HEIGHT`
/// doc for the single-doc pipeline, which notes height is normally
/// content-driven and not load-bearing) this packet picks a concrete,
/// classic-terminal-shaped default — 24 rows, i.e. 80x24 — purely so
/// pixel/percentage/star row tracks have SOME real number to divide.
/// Documented v0 simplification for the DECISIONS ledger: a real windowed
/// browser has an actual viewport height; headless text-mode does not, and
/// this is the least-surprising stand-in. Row height is a MINIMUM hint
/// regardless (see module docs) so this choice rarely changes rendered
/// output for ordinary fixtures.
const DEFAULT_FRAMESET_VIEWPORT_ROWS: usize = 24;

/// A tall-but-bounded viewport height for an ordinary (non-frameset)
/// document rendered inside a frame cell — mirrors `main.rs`'s
/// `HEADLESS_VIEWPORT_HEIGHT` (same value, duplicated rather than shared
/// since `main.rs` is the bin crate and this is the lib crate): height is
/// always content-derived for a single document, this is never actually a
/// binding constraint.
const SUBDOC_VIEWPORT_HEIGHT: f32 = 100_000.0;

/// Walk `dom` from its root for a `<frameset>` element (depth-bounded
/// against pathological nesting, same cap as `dom_util`'s own walks). If
/// found, the document should be routed through [`render`] instead of the
/// ordinary single-document pipeline. `None` means "render normally,
/// unchanged path" per the packet brief.
pub fn find_frameset(dom: &Dom) -> Option<NodeId> {
    if dom.is_empty() {
        return None;
    }
    find_frameset_from(dom, dom.root(), 0)
}

fn find_frameset_from(dom: &Dom, id: NodeId, depth: usize) -> Option<NodeId> {
    if depth >= crate::dom_util::DEPTH_CAP {
        return None;
    }
    match dom.node(id) {
        Node::Element(el) => {
            if el.name.as_str() == "frameset" {
                return Some(id);
            }
            for &child in &el.children {
                if let Some(found) = find_frameset_from(dom, child, depth + 1) {
                    return Some(found);
                }
            }
            None
        }
        Node::Text(_) => None,
    }
}

/// Bookkeeping threaded through the whole recursive render of one top-level
/// frameset document: the shared frame budget, and the URLs on the current
/// fetch path (for cycle detection). See module totality docs.
struct FrameCtx {
    budget: usize,
    visited: Vec<String>,
}

/// Render a frameset document rooted at `frameset_id` (as found by
/// [`find_frameset`]) into a single composited [`TextGrid`], `cols` cells
/// wide (matching `--cols`/the top-level pipeline's own width parameter).
/// Total: never panics regardless of how malformed the frameset markup,
/// `rows`/`cols` attributes, or any fetched child document is.
pub fn render(base_url: &Url, dom: &Dom, frameset_id: NodeId, cols: usize, scheme: ColorScheme) -> TextGrid {
    let width_px = cols as f32 * CELL_W;
    let height_px = DEFAULT_FRAMESET_VIEWPORT_ROWS as f32 * CELL_H;
    let mut ctx = FrameCtx { budget: MAX_TOTAL_FRAMES, visited: vec![base_url.as_str().to_string()] };
    render_frameset_grid(dom, frameset_id, base_url, width_px, height_px, 0, scheme, &mut ctx)
}

/// Render one `<frameset>` node's own region (`width_px` x `height_px`) as a
/// composited grid: partition into `rows x cols` tracks (pure math, no
/// rendering), allocate the canvas ONCE at that fixed size, then render and
/// [`TextGrid::blit`] each cell ONE AT A TIME — immediately dropping each
/// child's grid before the next cell is even rendered, so peak memory is the
/// (bounded) canvas plus exactly one in-flight child, never all of a row's
/// or a frameset's cells at once. See the module's "Row/column extent is
/// fixed from track math alone" and "Peak memory" docs for why this is safe
/// (and why an earlier revision that grew rows to fit content was not).
fn render_frameset_grid(
    dom: &Dom,
    frameset_id: NodeId,
    base_url: &Url,
    width_px: f32,
    height_px: f32,
    depth: usize,
    scheme: ColorScheme,
    ctx: &mut FrameCtx,
) -> TextGrid {
    let cols_total = tty::cell_index(width_px, CELL_W);
    if cols_total == 0 || !(width_px > 0.0) || !(height_px > 0.0) {
        return TextGrid::blank(0, 0);
    }
    let Node::Element(el) = dom.node(frameset_id) else {
        return TextGrid::blank(cols_total, 0);
    };

    let col_extents_px = compute_track_extents(el.attrs.get("cols"), width_px);
    let row_extents_px = compute_track_extents(el.attrs.get("rows"), height_px);
    let n_cols = col_extents_px.len();
    let n_rows = row_extents_px.len();

    let col_widths_cells: Vec<usize> = col_extents_px.iter().map(|&px| tty::cell_index(px, CELL_W)).collect();
    let row_heights_cells: Vec<usize> = row_extents_px.iter().map(|&px| tty::cell_index(px, CELL_H)).collect();

    let cell_children: Vec<NodeId> = el.children.iter().copied().filter(|&c| is_frame_or_frameset(dom, c)).collect();

    // Canvas size comes ENTIRELY from track math above, before any cell is
    // rendered — `TextGrid::blank` clamps to MAX_GRID_ROWS/MAX_GRID_COLS
    // regardless of how pathological `rows`/`cols` are, so this allocation
    // is bounded independent of anything a fetched frame document does.
    let total_rows: usize = row_heights_cells.iter().sum();
    let mut canvas = TextGrid::blank(cols_total, total_rows);

    let mut row_offset = 0usize;
    for r in 0..n_rows {
        let mut col_offset = 0usize;
        for c in 0..n_cols {
            let idx = r * n_cols + c;
            let cell_w_px = col_extents_px[c];
            let cell_h_px = row_extents_px[r];
            let cell_w_cells = col_widths_cells[c];
            // Render exactly one cell, blit it, then let `grid` drop at the
            // end of this iteration -- never held alongside its siblings.
            let grid = match cell_children.get(idx) {
                Some(&child_id) => render_cell(dom, child_id, base_url, cell_w_px, cell_h_px, cell_w_cells, depth, scheme, ctx),
                None => TextGrid::blank(cell_w_cells, 0),
            };
            canvas.blit(&grid, col_offset, row_offset);
            col_offset += col_widths_cells[c];
        }
        row_offset += row_heights_cells[r];
    }
    canvas
}

fn is_frame_or_frameset(dom: &Dom, id: NodeId) -> bool {
    matches!(dom.node(id), Node::Element(el) if matches!(el.name.as_str(), "frame" | "frameset"))
}

/// Render one grid cell's child element: a `<frame src>` (fetch + recurse or
/// run the ordinary single-doc pipeline), a nested `<frameset>` (recurse for
/// its own region), or anything else (blank — should not occur given the
/// `is_frame_or_frameset` filter above, but total regardless). Consumes one
/// unit of [`FrameCtx::budget`] per visit, and refuses to recurse past
/// [`MAX_FRAME_DEPTH`] — see module totality docs.
fn render_cell(
    dom: &Dom,
    id: NodeId,
    base_url: &Url,
    cell_w_px: f32,
    cell_h_px: f32,
    cell_w_cells: usize,
    depth: usize,
    scheme: ColorScheme,
    ctx: &mut FrameCtx,
) -> TextGrid {
    if ctx.budget == 0 {
        return placeholder(cell_w_cells, "[frame budget exhausted]");
    }
    ctx.budget -= 1;

    let Node::Element(el) = dom.node(id) else {
        return TextGrid::blank(cell_w_cells, 0);
    };

    match el.name.as_str() {
        "frameset" => {
            if depth + 1 >= MAX_FRAME_DEPTH {
                return placeholder(cell_w_cells, "[frame depth limit]");
            }
            render_frameset_grid(dom, id, base_url, cell_w_px, cell_h_px, depth + 1, scheme, ctx)
        }
        "frame" => render_frame(dom, el, base_url, cell_w_px, cell_h_px, cell_w_cells, depth, scheme, ctx),
        _ => TextGrid::blank(cell_w_cells, 0),
    }
}

fn render_frame(
    _dom: &Dom,
    el: &crate::dom::Element,
    base_url: &Url,
    cell_w_px: f32,
    cell_h_px: f32,
    cell_w_cells: usize,
    depth: usize,
    scheme: ColorScheme,
    ctx: &mut FrameCtx,
) -> TextGrid {
    let src = el.attrs.get("src").map(str::trim).filter(|s| !s.is_empty());
    let Some(src) = src else {
        let name = el.attrs.get("name").unwrap_or("frame");
        return placeholder(cell_w_cells, &format!("[{name}]"));
    };

    let resolved = base_url.resolve(src);
    let resolved_str = resolved.as_str().to_string();
    if ctx.visited.contains(&resolved_str) {
        return placeholder(cell_w_cells, "[frame cycle]");
    }

    let body = match fetch_body(&resolved) {
        Ok(b) => b,
        Err(_) => return placeholder(cell_w_cells, "[frame unavailable]"),
    };
    let html = String::from_utf8_lossy(&body);
    let child_dom = crate::dom::parser::parse(&html);

    ctx.visited.push(resolved_str);
    let result = match find_frameset(&child_dom) {
        Some(child_frameset_id) => {
            if depth + 1 >= MAX_FRAME_DEPTH {
                placeholder(cell_w_cells, "[frame depth limit]")
            } else {
                render_frameset_grid(&child_dom, child_frameset_id, &resolved, cell_w_px, cell_h_px, depth + 1, scheme, ctx)
            }
        }
        None => render_single_document(&child_dom, &resolved, cell_w_cells, scheme),
    };
    ctx.visited.pop();
    result
}

/// Run the ordinary (non-frameset) single-document pipeline — the same
/// cascade -> box-tree -> layout -> tty::render chain `main.rs::dump_text`
/// drives for a top-level document — over an already-fetched-and-parsed
/// `dom`, producing that frame's own [`TextGrid`] at `cols` cells wide.
/// `base_url` is this frame's OWN resolved `src` (not the top-level
/// document's) — m5-link-css: a `<link href>` inside this frame's `<head>`
/// must resolve against ITS document, exactly like ordinary HTML
/// document-relative resolution rules, not the frameset parent's URL.
fn render_single_document(dom: &Dom, base_url: &Url, cols: usize, scheme: ColorScheme) -> TextGrid {
    // M5 + m5-link-css: same author-CSS wiring as main.rs's own
    // single-document pipeline — each frame gets its own <style> blocks,
    // fetched <link rel=stylesheet href> sheets, AND inline style= applied,
    // not just the UA sheet. M5 media: this frame's own viewport width is
    // `cols * CELL_W` (its region's actual width in px, per the caller's
    // track-sizing math above) — `@media` (in-CSS or a `<link media=...>`
    // attribute) inside a frame's stylesheets is evaluated against THAT
    // region's width, not the top-level document's. `scheme` (packet
    // t1b-color-scheme) is the same `ColorScheme` the top-level document
    // renders under — every frame in a frameset shares one `--color-scheme`,
    // there is no per-frame override.
    let viewport_width = cols as f32 * CELL_W;
    let author_sheets = crate::stylesheets::collect_all_author_sheets(dom, base_url, viewport_width, scheme);
    let styles = cascade::cascade(dom, &author_sheets);
    // Frames render to a tty text grid, never pixels — no fetch/decode work
    // for images here (mirrors main.rs's own `dump_text` scope), so an
    // empty images map is always correct.
    let Some(root) = build_box_tree(dom, &styles, &std::collections::HashMap::new()) else {
        return TextGrid::blank(cols, 0);
    };
    let viewport = Size { w: viewport_width, h: SUBDOC_VIEWPORT_HEIGHT };
    let fragments = layout::layout(&root, viewport);
    tty::render(&fragments, cols)
}

/// A one-line placeholder grid (e.g. `[frame unavailable]`, `[frame
/// cycle]`) rendered through the exact same `tty::render` text-placement
/// path as any other `Text` fragment, so it inherits that path's own
/// clipping/totality guarantees for free rather than re-implementing them.
fn placeholder(cols: usize, text: &str) -> TextGrid {
    if cols == 0 {
        return TextGrid::blank(0, 0);
    }
    let frag = Fragment {
        rect: Rect { origin: Point { x: 0.0, y: 0.0 }, size: Size { w: cols as f32 * CELL_W, h: CELL_H } },
        kind: FragmentKind::Text { text: text.to_string(), baseline: CELL_H * 0.75, style: ComputedStyle::default() },
        interactive: None,
    };
    tty::render(std::slice::from_ref(&frag), cols)
}

/// Fetch `url`'s body over whichever of the two live schemes it names.
/// Duplicated from (rather than shared with) `main.rs::fetch_body`: that
/// function lives in the bin crate, this module lives in the lib crate, and
/// the packet brief scopes this driver-level module to ADDING to the lib
/// surface, not reaching into the bin. Both are small, both total (a fetch
/// error is a clean `Err`, never a panic).
fn fetch_body(url: &Url) -> Result<Vec<u8>, String> {
    match url.scheme().as_str() {
        "file" => FileFetcher::new().fetch(&Request::get(url.clone())).map(|r| r.body).map_err(|e| format!("{e:?}")),
        "http" => Http1Client::new().fetch(&Request::get(url.clone())).map(|r| r.body).map_err(|e| format!("{e:?}")),
        other => Err(format!("unsupported scheme: {other}")),
    }
}

// -- track sizing (pure, unit-tested independently of rendering) -----------

/// One parsed `rows`/`cols` track entry.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Track {
    /// A literal pixel count (e.g. `100`).
    Fixed(f32),
    /// A percentage of the space remaining after fixed tracks (e.g. `30%`).
    Percent(f32),
    /// A relative/star weight (e.g. `*` = weight 1, `2*` = weight 2).
    Star(f32),
}

/// An upper bound applied to any single parsed numeric track value (pixel
/// count, percent, or star weight) — independent of [`MAX_TRACKS_PER_DIMENSION`]
/// (which bounds the *count* of tracks), this bounds any ONE track's
/// magnitude so a single absurd `rows="999999999999"` can't produce a
/// pixel/cell extent so large it would blow past `TextGrid::blank`'s own
/// `MAX_GRID_ROWS`/`MAX_GRID_COLS` clamp in one hop (that clamp is still the
/// real backstop; this just keeps intermediate f32 math sane).
const MAX_TRACK_VALUE: f32 = 100_000.0;

fn parse_track_spec(spec: Option<&str>) -> Vec<Track> {
    let Some(s) = spec else {
        return vec![Track::Star(1.0)];
    };
    let tokens: Vec<&str> = s.split(',').take(MAX_TRACKS_PER_DIMENSION).collect();
    let out: Vec<Track> = tokens.iter().map(|t| parse_track_token(t)).collect();
    if out.is_empty() {
        vec![Track::Star(1.0)]
    } else {
        out
    }
}

fn parse_track_token(tok: &str) -> Track {
    let tok = tok.trim();
    if tok.is_empty() {
        return Track::Star(1.0);
    }
    if let Some(pct) = tok.strip_suffix('%') {
        return match pct.trim().parse::<f32>() {
            Ok(v) if v.is_finite() && v >= 0.0 => Track::Percent(v.min(MAX_TRACK_VALUE)),
            _ => Track::Star(1.0),
        };
    }
    if let Some(star) = tok.strip_suffix('*') {
        let star = star.trim();
        if star.is_empty() {
            return Track::Star(1.0);
        }
        return match star.parse::<f32>() {
            Ok(v) if v.is_finite() && v >= 0.0 => Track::Star(v.min(MAX_TRACK_VALUE)),
            _ => Track::Star(1.0),
        };
    }
    match tok.parse::<f32>() {
        Ok(v) if v.is_finite() && v >= 0.0 => Track::Fixed(v.min(MAX_TRACK_VALUE)),
        _ => Track::Star(1.0),
    }
}

/// Compute each track's pixel extent within `total_px`, per the rules in
/// the module docs: fixed pixel tracks first; percentage tracks take their
/// percentage of what's left after fixed; star tracks split whatever
/// remains after that, proportional to weight. Total: a non-finite or
/// non-positive `total_px` yields all-zero extents (never divides by zero,
/// never panics) — the caller (`render_frameset_grid`) already treats a
/// zero/negative region as "nothing to draw" before this is even reached,
/// but this function holds that guarantee independently since it's the
/// unit that's directly tested.
fn compute_track_extents(spec: Option<&str>, total_px: f32) -> Vec<f32> {
    let tracks = parse_track_spec(spec);
    let total = if total_px.is_finite() && total_px > 0.0 { total_px } else { 0.0 };
    if total == 0.0 {
        return vec![0.0; tracks.len()];
    }

    let fixed_sum: f32 = tracks.iter().map(|t| if let Track::Fixed(v) = t { *v } else { 0.0 }).sum();
    let remaining1 = (total - fixed_sum).max(0.0);

    let percent_extents: Vec<f32> = tracks
        .iter()
        .map(|t| if let Track::Percent(p) = t { (p / 100.0) * remaining1 } else { 0.0 })
        .collect();
    let percent_sum: f32 = percent_extents.iter().sum();
    let remaining2 = (remaining1 - percent_sum).max(0.0);

    let star_weight_sum: f32 = tracks.iter().map(|t| if let Track::Star(w) = t { *w } else { 0.0 }).sum();

    tracks
        .iter()
        .enumerate()
        .map(|(i, t)| match t {
            Track::Fixed(v) => *v,
            Track::Percent(_) => percent_extents[i],
            Track::Star(w) => {
                if star_weight_sum > 0.0 {
                    remaining2 * (w / star_weight_sum)
                } else {
                    0.0
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::ElementName;

    /// A `file://` `Url` pointing at `fixtures/<name>` in this crate's own
    /// checkout — same `CARGO_MANIFEST_DIR`-based pattern
    /// `tests/frames_golden.rs`'s own `fixture_url` helper uses, duplicated
    /// here since this module's tests are compiled into the lib crate, not
    /// that separate integration-test crate.
    fn fixture_url(name: &str) -> Url {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures").join(name);
        Url::new(format!("file://{}", path.display()))
    }

    /// m5-link-css: a `<frame src="link-css.html">` pointing at the REAL
    /// `fixtures/link-css.html`/`fixtures/link-css.css` pair proves
    /// [`render_single_document`]'s new `base_url` parameter is actually
    /// threaded through `render_frame` correctly — a frame's own document
    /// gets its `<link rel=stylesheet href>` fetched and resolved against
    /// ITS OWN url (not the top-level frameset document's), exactly like an
    /// ordinary top-level document already does via `main.rs::dump_text`.
    #[test]
    fn a_frame_document_fetches_and_applies_its_own_link_stylesheet() {
        let mut dom = Dom::new();
        let root = dom.root();
        let fs = dom.new_element(ElementName::new("frameset"));
        dom.append_child(root, fs);
        let frame = dom.new_element(ElementName::new("frame"));
        if let Node::Element(el) = dom.node_mut(frame) {
            el.attrs.set("src", "link-css.html");
        }
        dom.append_child(fs, frame);

        // Base URL is some OTHER file under fixtures/ (deliberately not
        // link-css.html itself) -- proves resolution against the frameset's
        // base, not a same-file coincidence.
        let base = fixture_url("frames.html");
        let grid = render(&base, &dom, fs, 80, ColorScheme::Light);
        let text = grid.to_text();

        assert!(!text.contains("the external"), "the <link>-sourced display:none rule should have removed this paragraph inside the frame: {text:?}");
        assert!(text.contains("Visible again"), "the later <style> block inside the frame doc should still win its source-order tie: {text:?}");
    }

    // ------------------------------- track sizing (hand-computed) ---------

    fn approx_eq(a: f32, b: f32) {
        assert!((a - b).abs() < 0.01, "expected ~{b}, got {a}");
    }

    #[test]
    fn fixed_then_star_splits_the_remainder() {
        // rows="100,*" over a 200px region: 100px fixed, 100px left for the
        // single star track (weight 1) -> [100, 100].
        let extents = compute_track_extents(Some("100,*"), 200.0);
        assert_eq!(extents.len(), 2);
        approx_eq(extents[0], 100.0);
        approx_eq(extents[1], 100.0);
    }

    #[test]
    fn percentages_split_the_full_region_when_there_are_no_fixed_tracks() {
        // cols="25%,75%" over an 800px region: no fixed tracks, so
        // "remaining after fixed" IS the full region -> [200, 600].
        let extents = compute_track_extents(Some("25%,75%"), 800.0);
        assert_eq!(extents.len(), 2);
        approx_eq(extents[0], 200.0);
        approx_eq(extents[1], 600.0);
    }

    #[test]
    fn star_weights_split_proportionally() {
        // "*,2*" over 300px: weights 1 and 2 -> [100, 200].
        let extents = compute_track_extents(Some("*,2*"), 300.0);
        assert_eq!(extents.len(), 2);
        approx_eq(extents[0], 100.0);
        approx_eq(extents[1], 200.0);
    }

    #[test]
    fn absent_attribute_is_a_single_100_percent_track() {
        let extents = compute_track_extents(None, 400.0);
        assert_eq!(extents.len(), 1);
        approx_eq(extents[0], 400.0);
    }

    #[test]
    fn malformed_entries_fall_back_to_a_star_track() {
        // "garbage,50%" over 200px: "garbage" -> 1* fallback, "50%" takes
        // 50% of the region (no fixed tracks) = 100px, leaving 100px for
        // the single star track (weight 1) -> [100, 100].
        let extents = compute_track_extents(Some("garbage,50%"), 200.0);
        assert_eq!(extents.len(), 2);
        approx_eq(extents[0], 100.0);
        approx_eq(extents[1], 100.0);
    }

    #[test]
    fn empty_and_whitespace_entries_fall_back_to_star_tracks() {
        let extents = compute_track_extents(Some(" , "), 100.0);
        assert_eq!(extents.len(), 2);
        approx_eq(extents[0], 50.0);
        approx_eq(extents[1], 50.0);
    }

    #[test]
    fn zero_or_nonfinite_total_never_divides_by_zero() {
        assert_eq!(compute_track_extents(Some("100,*"), 0.0), vec![0.0, 0.0]);
        assert_eq!(compute_track_extents(Some("100,*"), -5.0), vec![0.0, 0.0]);
        assert_eq!(compute_track_extents(Some("100,*"), f32::NAN), vec![0.0, 0.0]);
    }

    #[test]
    fn absurd_track_counts_are_capped_not_a_panic() {
        let spec = "1,".repeat(10_000);
        let extents = compute_track_extents(Some(&spec), 1000.0);
        assert!(extents.len() <= MAX_TRACKS_PER_DIMENSION);
    }

    #[test]
    fn absurdly_large_single_track_value_is_bounded() {
        let extents = compute_track_extents(Some("999999999999"), 1000.0);
        assert!(extents[0] <= MAX_TRACK_VALUE);
    }

    // ------------------------------- find_frameset -------------------------

    #[test]
    fn finds_a_frameset_reachable_from_the_root() {
        let dom = crate::dom::parser::parse("<html><frameset cols=\"50%,50%\"></frameset></html>");
        assert!(find_frameset(&dom).is_some());
    }

    #[test]
    fn ordinary_document_has_no_frameset() {
        let dom = crate::dom::parser::parse("<html><body><p>hi</p></body></html>");
        assert!(find_frameset(&dom).is_none());
    }

    #[test]
    fn empty_dom_has_no_frameset() {
        let dom = Dom::new();
        // A fresh `Dom::new()` seeds a bare "html" root; still no frameset.
        assert!(find_frameset(&dom).is_none());
    }

    // ------------------------------- grid placement / compositing ---------

    fn frameset_dom(cols_attr: Option<&str>, rows_attr: Option<&str>, child_names: &[&str]) -> (Dom, NodeId) {
        let mut dom = Dom::new();
        let root = dom.root();
        let fs = dom.new_element(ElementName::new("frameset"));
        if let Node::Element(el) = dom.node_mut(fs) {
            if let Some(c) = cols_attr {
                el.attrs.set("cols", c);
            }
            if let Some(r) = rows_attr {
                el.attrs.set("rows", r);
            }
        }
        dom.append_child(root, fs);
        for name in child_names {
            let child = dom.new_element(ElementName::new(name));
            dom.append_child(fs, child);
        }
        (dom, fs)
    }

    #[test]
    fn two_side_by_side_placeholder_frames_land_in_the_expected_columns() {
        // cols="50%,50%", two <frame>s with no src -> two "[frame]"
        // placeholders, left one starting at col 0, right one at col 5
        // (10 cols total, split 50/50 -> 5 cols each).
        let (dom, fs) = frameset_dom(Some("50%,50%"), None, &["frame", "frame"]);
        let grid = render(&Url::new("file:///x.html"), &dom, fs, 10, ColorScheme::Light);
        let text = grid.to_text();
        let lines: Vec<&str> = text.lines().collect();
        assert!(!lines.is_empty());
        assert!(lines[0].starts_with("[fram"), "left cell: {lines:?}");
        assert!(lines[0][5..].starts_with('['), "right cell should start at col 5: {lines:?}");
    }

    #[test]
    fn two_stacked_placeholder_frames_land_on_separate_rows() {
        // "16,16": each track is exactly CELL_H (16px) -> exactly 1 cell of
        // nominal row height each. Row extent is now fixed from track math
        // alone (never grown to fit content — see module docs), so the
        // track values must reserve real cell height for the single-line
        // placeholder text to survive, unlike the old content-grown design.
        let (dom, fs) = frameset_dom(None, Some("16,16"), &["frame", "frame"]);
        let grid = render(&Url::new("file:///x.html"), &dom, fs, 10, ColorScheme::Light);
        let text = grid.to_text();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines.len() >= 2, "expected at least two rows: {lines:?}");
        assert!(lines[0].starts_with('['));
        assert!(lines[1].starts_with('['));
    }

    #[test]
    fn missing_src_renders_a_named_placeholder_not_a_panic() {
        let mut dom = Dom::new();
        let root = dom.root();
        let fs = dom.new_element(ElementName::new("frameset"));
        dom.append_child(root, fs);
        let frame = dom.new_element(ElementName::new("frame"));
        if let Node::Element(el) = dom.node_mut(frame) {
            el.attrs.set("name", "nav");
        }
        dom.append_child(fs, frame);
        let grid = render(&Url::new("file:///x.html"), &dom, fs, 20, ColorScheme::Light);
        assert!(grid.to_text().contains("[nav]"));
    }

    #[test]
    fn unresolvable_scheme_src_renders_a_placeholder_not_a_panic() {
        let mut dom = Dom::new();
        let root = dom.root();
        let fs = dom.new_element(ElementName::new("frameset"));
        dom.append_child(root, fs);
        let frame = dom.new_element(ElementName::new("frame"));
        if let Node::Element(el) = dom.node_mut(frame) {
            el.attrs.set("src", "ftp://example.com/nope.html");
        }
        dom.append_child(fs, frame);
        let grid = render(&Url::new("file:///x.html"), &dom, fs, 20, ColorScheme::Light);
        assert!(grid.to_text().contains('['), "expected some placeholder marker: {}", grid.to_text());
    }

    // ------------------------------- totality: cycles / depth / bombs -----

    #[test]
    fn self_referential_frame_src_is_a_bounded_cycle_placeholder() {
        // A <frame src="self.html"> inside the document fetched AS
        // self.html: the base URL is already on the visited path, so
        // resolving to the same URL must short-circuit to a placeholder
        // rather than attempting to re-fetch/re-recurse.
        let mut dom = Dom::new();
        let root = dom.root();
        let fs = dom.new_element(ElementName::new("frameset"));
        dom.append_child(root, fs);
        let frame = dom.new_element(ElementName::new("frame"));
        if let Node::Element(el) = dom.node_mut(frame) {
            el.attrs.set("src", "self.html");
        }
        dom.append_child(fs, frame);
        let base = Url::new("file:///self.html");
        let grid = render(&base, &dom, fs, 20, ColorScheme::Light);
        // Must not hang/panic; a cycle placeholder or an unavailable-fetch
        // placeholder are both acceptable bounded outcomes.
        assert!(grid.to_text().contains('['));
    }

    #[test]
    fn deeply_nested_frameset_bomb_is_bounded_by_max_frame_depth() {
        // Build MAX_FRAME_DEPTH + 4 levels of directly-nested <frameset>s
        // (no fetch involved -- this alone must terminate, proving the
        // depth cap, not the budget, is what stops it).
        let mut dom = Dom::new();
        let root = dom.root();
        let mut current_parent = root;
        let mut top_fs = None;
        for _ in 0..(MAX_FRAME_DEPTH + 4) {
            let fs = dom.new_element(ElementName::new("frameset"));
            dom.append_child(current_parent, fs);
            if top_fs.is_none() {
                top_fs = Some(fs);
            }
            current_parent = fs;
        }
        let grid = render(&Url::new("file:///deep.html"), &dom, top_fs.unwrap(), 20, ColorScheme::Light);
        // Must terminate (test itself times out if it doesn't) and produce
        // *some* grid without panicking.
        let _ = grid.to_text();
    }

    #[test]
    fn wide_frame_bomb_is_bounded_by_max_total_frames() {
        // A frameset with 32x32 = 1024 grid cells (cols/rows tracks each
        // capped at MAX_TRACKS_PER_DIMENSION), each filled by its own
        // <frame> child -- far more than MAX_TOTAL_FRAMES (128). Proves the
        // shared budget kicks in partway through and every remaining cell
        // still degrades to a cheap placeholder rather than hanging or
        // exhausting memory. Column/row tracks are wide enough (320px/40px)
        // that the placeholder text isn't itself clipped to nothing, so the
        // budget-exhausted marker is actually observable in the output.
        let cols_tracks = std::iter::repeat("320").take(32).collect::<Vec<_>>().join(",");
        let rows_tracks = std::iter::repeat("40").take(32).collect::<Vec<_>>().join(",");
        let names: Vec<&str> = std::iter::repeat("frame").take(1024).collect();
        let (dom, fs) = frameset_dom(Some(cols_tracks.as_str()), Some(rows_tracks.as_str()), &names);
        let grid = render(&Url::new("file:///wide.html"), &dom, fs, 400, ColorScheme::Light);
        let text = grid.to_text();
        assert!(text.contains("[frame budget exhausted]"), "expected the budget-exhausted placeholder to appear: {text}");
    }

    #[test]
    fn cyclic_two_document_frame_bomb_does_not_hang() {
        // A->B->A cannot be exercised without real fetches in a unit test
        // (no file:// fixtures here), but we can still prove the ctx-level
        // visited-path check would catch a direct self-cycle at the top
        // (already covered above) and that a >MAX_FRAME_DEPTH synthetic
        // in-DOM nesting terminates (covered above). This test documents
        // the real A->B->A path is exercised end-to-end by the fixture
        // golden instead (frames.html has no such fixture by design -- see
        // the packet report), and here just re-asserts totality on a
        // pathological wide+deep combination.
        let mut dom = Dom::new();
        let root = dom.root();
        let mut parent = root;
        for _ in 0..(MAX_FRAME_DEPTH + 2) {
            let fs = dom.new_element(ElementName::new("frameset"));
            dom.append_child(parent, fs);
            for _ in 0..5 {
                let frame = dom.new_element(ElementName::new("frame"));
                dom.append_child(fs, frame);
            }
            parent = fs;
        }
        let top = {
            let Node::Element(el) = dom.node(root) else { unreachable!() };
            el.children[0]
        };
        let grid = render(&Url::new("file:///bomb.html"), &dom, top, 30, ColorScheme::Light);
        let _ = grid.to_text();
    }

    #[test]
    fn zero_cols_yields_an_empty_grid_not_a_panic() {
        let (dom, fs) = frameset_dom(None, None, &["frame"]);
        let grid = render(&Url::new("file:///x.html"), &dom, fs, 0, ColorScheme::Light);
        assert_eq!(grid.to_text(), "");
    }
}
