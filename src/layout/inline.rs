//! The bespoke inline engine (P6 M2 + M4 floats/inline-images): break
//! inline-level content — text runs and non-floated replaced atoms (`<img>`
//! with no `float`), each carrying its own [`ComputedStyle`] — into line
//! boxes at a given available width, using [`Metrics`] for glyph advances,
//! and place floated replaced content (`float: left|right`, the 1996 `img
//! align=left` shape) at the containing block's edge with an exclusion that
//! shortens overlapping line boxes so text wraps around it.
//!
//! `text-align: center`/`right` (packet `text-align`) shift each completed
//! line's fragments within that line's own float-aware available width —
//! see [`layout_runs`]'s `text_align` parameter and `align_offset`.
//! `justify` is treated as `left` (no inter-word stretching) — a documented
//! v0 simplification; no fixture needs true justification.
//!
//! Out of scope even after that: bidi/complex shaping (the `Metrics` seam is
//! shaping-free by design),
//! cross-block float continuation (a float that outlives its own containing
//! block's inline formatting context — M4 scope is "the float and the
//! wrapping text share one IFC", the classic `<p><img align=left>text...
//! </p>` shape; see `place_floats`'s doc comment) and full `clear`
//! interaction (see the same doc comment for why that's a no-op here without
//! silently dropping anything real).
//!
//! Whitespace handling: consecutive whitespace collapses to a single space
//! (CSS `white-space: normal`), matching the curated `WhiteSpace` property's
//! only two states (`Normal`/`Pre`) — `Pre` is not yet honored (documented
//! scope call: v1 always collapses; a `Pre` fast-path is a follow-up). Line
//! breaking only happens at those collapsed-space opportunities: a single
//! "word" (including one that is itself wider than the available width, or a
//! replaced atom wider than the available width) never splits — it just
//! overflows onto its own line, which keeps the engine total (no panics) on
//! any input.
//!
//! Forced breaks (M6 hardening, HTML `<br>`): [`LINE_BREAK_SENTINEL`] is a
//! reserved character `box_tree` emits as a `<br>` element's own `Text`
//! content; [`tokenize`] recognizes it and emits a [`Token::Break`] instead
//! of folding it into a word, and [`layout_runs`]'s main loop unconditionally
//! ends the current line when it sees one — regardless of remaining width —
//! then resumes laying out on a fresh line, exactly like a soft wrap except
//! it happens even when the current line isn't full.
//!
//! An inline "word" may itself be glued together out of pieces from more
//! than one source run when no whitespace separates them in the source
//! (e.g. `<b>bold</b>text` — no space between the two). Such glued pieces
//! are tracked as one atomic unit for the fits-on-this-line decision (so the
//! visual word is never split across lines) while still being emitted as
//! separate [`PositionedRun`]s so each retains its own style. A replaced
//! atom is never glued to neighboring text this way (each occupies its own
//! source run with a unique index — see [`InlineContent::Replaced`]), but it
//! still participates in the same "never split a cluster across lines" rule
//! via [`cluster`].

use std::rc::Rc;

use crate::img::RgbaImage;
use crate::layout::{Point, Rect, Size};
use crate::style::computed::{Float as CssFloat, LineHeight, TextAlign};
use crate::style::ComputedStyle;
use crate::text::Metrics;

/// M6 hardening: the sentinel [`BoxContent::Text`] payload
/// `layout::box_tree` synthesizes for an HTML `<br>` element (a real forced
/// line break, per the packet brief's kitchen-sink coverage list) — a
/// Private Use Area codepoint, chosen because `dom::parser` never itself
/// produces one (no named/numeric HTML entity decodes to it, and the parser
/// otherwise passes source bytes through as literal UTF-8 text), so this
/// module's tokenizer can reliably tell "this Text run IS a `<br>`" apart
/// from ordinary character data without a new frozen `BoxContent`/
/// `InlineContent` variant threading all the way from `box_tree` (which only
/// ever sees the frozen `LayoutNode`/`BoxContent` shapes) through to here. A
/// hostile/unusual document whose own text content happens to contain this
/// literal codepoint would see it misrendered as a forced break instead of a
/// literal (invisible, unmapped-glyph) character — a cosmetic edge case, not
/// a totality/crash concern, and one the fuzz harness's own random-byte
/// inputs exercise (see `tests/fuzz_totality.rs`) without ever panicking.
pub(crate) const LINE_BREAK_SENTINEL: char = '\u{E000}';

/// Bound on any single dimension (width or height) pulled from untrusted
/// content (an `<img>`'s intrinsic size, itself sourced from HTML
/// `width`/`height` attributes with no upper bound of their own — see
/// `layout::box_tree::img_intrinsic`). Applied via [`clamp_dim`] everywhere
/// an intrinsic size or a derived measurement enters this module's
/// arithmetic, so a hostile `width="999999999999"` (or a decoder-declared
/// dimension had one leaked through) can't march any accumulator toward
/// `f32::INFINITY`/`NaN` (`inf - inf` is `NaN`, and `NaN` comparisons are
/// false, which can defeat totality invariants that assume `>`/`<` are total
/// orders over the values in play). 1,000,000px is far beyond any real
/// document's content while leaving enormous headroom over anything a real
/// fixture needs.
pub(crate) const MAX_DIM: f32 = 1_000_000.0;

/// The maximum number of floated replaced atoms one [`layout_runs`] call
/// will place. Distinct from any depth/count cap upstream (`box_tree`'s
/// `DEPTH_CAP`, `block`'s `DEPTH_CAP`): those bound *tree* shape, but a
/// single flat paragraph with thousands of `<img align=left>` siblings (all
/// at depth 1) would sail past them and still hand [`place_floats`] an
/// unbounded slice — each placement is O(1) but [`line_exclusion`] is
/// O(floats) *per line*, so an unbounded float count makes the whole
/// [`layout_runs`] call O(lines * floats), which a large document could
/// otherwise blow past any reasonable time budget for. 256 is far beyond any
/// real 1996-era hand-authored page (a handful of `align=left` images per
/// paragraph is already unusual) while keeping the worst case a small, fast,
/// fixed constant. Floats past the cap are silently not placed (excluded
/// from the returned [`InlineLayout::floats`], so `block.rs` never emits a
/// fragment for them) — a documented, bounded degrade, not a panic.
const MAX_FLOATS: usize = 256;

/// Clamp `v` into `[0, MAX_DIM]`, flooring any non-finite (`NaN`/`±inf`) or
/// negative value to `0.0`. The one totality seam every intrinsic size,
/// glyph measurement, and derived accumulator in this module passes through
/// before it can influence line-breaking or float placement.
pub(crate) fn clamp_dim(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, MAX_DIM)
    } else {
        0.0
    }
}

/// One inline-level content run's payload: either character data for the
/// line-breaker to split into words, or a replaced element (`<img>`) sized
/// at its `intrinsic` px size, optionally carrying decoded pixel data for
/// `block::emit` to paint (`None` falls back to a placeholder box, exactly
/// as `BoxContent::Replaced` does upstream).
///
/// A `Replaced` run's `style.float` (carried on the owning [`InlineRun`],
/// not here) decides its fate: `Float::None` makes it an inline-level ATOM
/// that sits on the line like an unbreakable word (M4 D14 gap, part 2);
/// `Float::Left`/`Float::Right` pulls it out of line flow entirely and
/// routes it through [`place_floats`] instead (M4 part 3) — see
/// [`tokenize`]'s dispatch on this same flag.
#[derive(Debug, Clone)]
pub enum InlineContent {
    Text(String),
    Replaced {
        intrinsic: Size,
        image: Option<Rc<RgbaImage>>,
    },
}

/// One inline-level content run: a span of text (or one replaced atom)
/// sharing one style. Built by flattening a `LayoutNode`'s inline-level
/// content (text, non-floated replaced elements, and any nested `display:
/// inline` containers) in document order.
#[derive(Debug, Clone)]
pub struct InlineRun {
    pub content: InlineContent,
    pub style: ComputedStyle,
    /// Interactive provenance (P7 interactive-provenance freeze amendment):
    /// carried straight from the source `LayoutNode::interactive` this run
    /// was flattened from (`block::translate_any`/`block::flatten_inline`),
    /// so `block::emit` can copy it onto every `Fragment` this run produces
    /// — including each line a wrapped link's text splits across, since a
    /// wrapped run still points back at the same `runs[run_index]` entry.
    pub interactive: Option<crate::layout::Interactive>,
}

/// A slice of one source [`InlineRun`] that landed on one line, positioned
/// relative to the line box's left edge (`x`), with a `width` for hit-testing
/// / painting convenience. For a replaced atom (`runs[run_index].content` is
/// `Replaced`), `text` is always empty — the caller (`block::emit`) paints
/// the atom's image/placeholder at `(x, width)` instead of drawing glyphs.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionedRun {
    /// Index into the `runs` slice passed to [`layout_runs`].
    pub run_index: usize,
    pub text: String,
    pub x: f32,
    pub width: f32,
}

/// One wrapped line: its box relative to the inline container's content-box
/// origin, the runs positioned within it (left to right, in source order),
/// and the baseline offset down from `rect.origin.y`. `rect.origin.x` is
/// normally `0.0`, but is offset rightward by a left float's width whenever
/// this line's vertical span overlaps one (see [`line_exclusion`]) — the
/// float-wrap mechanism.
#[derive(Debug, Clone, PartialEq)]
pub struct LineBox {
    pub rect: Rect,
    pub baseline: f32,
    pub runs: Vec<PositionedRun>,
}

/// Which edge of the containing block a floated replaced atom is pinned to
/// (the CSS `float` property's two non-`none` values, carried per-atom here
/// rather than re-reading `ComputedStyle.float` at emit time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatSide {
    Left,
    Right,
}

/// A floated replaced atom's final placement (see [`place_floats`]),
/// relative to the same origin as [`LineBox::rect`] (the inline container's
/// content-box origin) — `block::emit` adds this to the leaf's own painted
/// origin exactly like it does for `LineBox`/`PositionedRun`.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionedFloat {
    /// Index into the `runs` slice passed to [`layout_runs`] — look up
    /// `runs[run_index].content`'s `image`/`intrinsic` to paint it.
    pub run_index: usize,
    pub side: FloatSide,
    pub rect: Rect,
}

/// The result of breaking `runs` into lines (and placing any floats) at some
/// available width: the overall bounding size (width = widest line or
/// float's right edge, whichever is greater; height = line stack height or
/// the deepest float's bottom edge, whichever is greater) plus the lines and
/// placed floats themselves for fragment emission.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InlineLayout {
    pub size: Size,
    pub lines: Vec<LineBox>,
    pub floats: Vec<PositionedFloat>,
}

/// One maximal run of non-whitespace content from a single source run,
/// tagged with whether whitespace preceded it in the (flattened) token
/// stream. Glued cross-run words (no whitespace at the run boundary) are
/// separate `Word`s with `space_before = false`, so they cluster together
/// below. A replaced atom is always its own `Word` (`text` empty — its width
/// comes from `runs[run].content`'s `intrinsic`, not glyph measurement; see
/// [`word_metrics`]) since it always occupies a whole source run by
/// construction (`flatten_inline` never merges a `Replaced` into a `Text`
/// run's string).
struct Word {
    run: usize,
    text: String,
    space_before: bool,
}

/// One item in the flattened token stream: an ordinary word, or a forced
/// break ([`LINE_BREAK_SENTINEL`]) carrying the source run index (so an
/// isolated break with no text on its line still has a `ComputedStyle` to
/// derive a sensible line-height/baseline from — see `layout_runs`'s
/// `Cluster::Break` handling).
enum Token {
    Word(Word),
    Break(usize),
}

/// Split the flattened content of `runs` into whitespace-delimited `Word`s
/// (wrapped as [`Token::Word`]) plus [`Token::Break`]s wherever
/// [`LINE_BREAK_SENTINEL`] appears in a text run, collapsing any run of
/// ordinary whitespace (even one spanning a run boundary) into a single break
/// opportunity. A floated `Replaced` run (`style.float != Float::None`) is
/// transparent to this token stream — it contributes no token and neither
/// flushes nor sets `pending_space` — it is placed separately by
/// [`place_floats`], not laid out as inline flow content; a non-floated
/// `Replaced` run becomes its own atomic `Word` (flushing whatever text was
/// pending first, exactly like hitting whitespace would). Total: handles
/// empty runs, all-whitespace runs, and runs with no whitespace at all
/// without panicking.
fn tokenize(runs: &[InlineRun]) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut pending_space = false;
    let mut cur: Option<(usize, String)> = None;

    let flush = |cur: &mut Option<(usize, String)>, pending_space: &mut bool, tokens: &mut Vec<Token>| {
        if let Some((run, text)) = cur.take() {
            tokens.push(Token::Word(Word { run, text, space_before: *pending_space }));
            *pending_space = false;
        }
    };

    for (i, r) in runs.iter().enumerate() {
        match &r.content {
            InlineContent::Text(text) => {
                for ch in text.chars() {
                    if ch == LINE_BREAK_SENTINEL {
                        flush(&mut cur, &mut pending_space, &mut tokens);
                        tokens.push(Token::Break(i));
                        pending_space = false;
                    } else if ch.is_whitespace() {
                        flush(&mut cur, &mut pending_space, &mut tokens);
                        pending_space = true;
                    } else {
                        match &mut cur {
                            Some((run, t)) if *run == i => t.push(ch),
                            _ => {
                                flush(&mut cur, &mut pending_space, &mut tokens);
                                cur = Some((i, ch.to_string()));
                            }
                        }
                    }
                }
            }
            InlineContent::Replaced { .. } => {
                if r.style.float == CssFloat::None {
                    flush(&mut cur, &mut pending_space, &mut tokens);
                    tokens.push(Token::Word(Word { run: i, text: String::new(), space_before: pending_space }));
                    pending_space = false;
                }
                // A floated atom is invisible to the token stream: neither
                // flushed nor recorded — `place_floats` handles it instead.
            }
        }
    }
    flush(&mut cur, &mut pending_space, &mut tokens);
    tokens
}

/// A maximal group of `Word`s with no whitespace between them (the
/// line-breaking atom, never split across lines and never glued across a
/// [`Cluster::Break`]), or a forced break itself. `space_before` says whether
/// a collapsed whitespace opportunity precedes a `Words` cluster.
enum Cluster {
    Words { space_before: bool, words: Vec<Word> },
    Break { run: usize },
}

fn cluster(tokens: Vec<Token>) -> Vec<Cluster> {
    let mut clusters: Vec<Cluster> = Vec::new();
    for t in tokens {
        match t {
            Token::Break(run) => clusters.push(Cluster::Break { run }),
            Token::Word(w) => {
                if !w.space_before {
                    if let Some(Cluster::Words { words, .. }) = clusters.last_mut() {
                        words.push(w);
                        continue;
                    }
                }
                let space_before = w.space_before;
                clusters.push(Cluster::Words { space_before, words: vec![w] });
            }
        }
    }
    clusters
}

fn resolved_line_height<M: Metrics>(style: &ComputedStyle, metrics: &M) -> f32 {
    match style.line_height {
        LineHeight::Normal => metrics.line_height(style.font_size),
        LineHeight::Px(v) if v.is_finite() && v > 0.0 => v,
        LineHeight::Px(_) => metrics.line_height(style.font_size),
    }
}

/// One word's contribution to line layout: `(width, ascent, descent,
/// line_height)`, dispatching on whether `runs[w.run]` is text (glyph
/// metrics, as before M4) or a replaced atom (its clamped `intrinsic` size
/// stands in for all four — an image's "ascent" is its full height, "descent"
/// zero, so the image's bottom edge sits on the line's baseline, matching
/// the common UA default of `vertical-align: baseline` for replaced content;
/// its own "line height" is just its height, since `resolved_line_height`'s
/// font-driven notion doesn't apply to a non-text atom).
fn word_metrics<M: Metrics>(runs: &[InlineRun], w: &Word, metrics: &M) -> (f32, f32, f32, f32) {
    let style = &runs[w.run].style;
    match &runs[w.run].content {
        InlineContent::Text(_) => {
            let width = clamp_dim(metrics.measure(&w.text, style.font_size));
            let ascent = clamp_dim(metrics.ascent(style.font_size));
            let descent = clamp_dim(metrics.descent(style.font_size));
            let line_height = clamp_dim(resolved_line_height(style, metrics));
            (width, ascent, descent, line_height)
        }
        InlineContent::Replaced { intrinsic, .. } => {
            let w_ = clamp_dim(intrinsic.w);
            let h_ = clamp_dim(intrinsic.h);
            (w_, h_, 0.0, h_)
        }
    }
}

/// Collect every FLOATED replaced run (`style.float != Float::None`) out of
/// `runs`, in document order, each tagged with its side and clamped
/// intrinsic size — the input [`place_floats`] positions. Capped at
/// [`MAX_FLOATS`] (see its own doc comment); later floats in document order
/// past the cap are simply not collected (and so never placed/emitted).
fn collect_float_specs(runs: &[InlineRun]) -> Vec<(usize, FloatSide, Size)> {
    let mut out = Vec::new();
    for (i, r) in runs.iter().enumerate() {
        if out.len() >= MAX_FLOATS {
            break;
        }
        if let InlineContent::Replaced { intrinsic, .. } = &r.content {
            let side = match r.style.float {
                CssFloat::Left => Some(FloatSide::Left),
                CssFloat::Right => Some(FloatSide::Right),
                CssFloat::None => None,
            };
            if let Some(side) = side {
                out.push((i, side, Size { w: clamp_dim(intrinsic.w), h: clamp_dim(intrinsic.h) }));
            }
        }
    }
    out
}

/// Place every collected float at its containing block's left/right edge,
/// stacking same-side floats horizontally until they no longer fit the
/// available width, then dropping to a new "row" below the tallest float
/// placed so far on that side — independently for the left and right sides
/// (a documented M4 simplification: a left float and a right float placed at
/// the same row are not checked against each other for overlap; real CSS's
/// float-avoidance is considerably more involved, and no M4 fixture needs
/// left+right floats colliding in one paragraph).
///
/// M4 scope: every float here is placed starting at `y = 0` (the top of its
/// own inline formatting context / containing block), NOT at the vertical
/// position its `<img>` tag happens to appear at in the source. This is the
/// documented simplification for "the float and the wrapping text share one
/// IFC" (module docs): the overwhelmingly common 1996 shape is `<p><img
/// align=left>text...</p>` — the float as the very first thing in the
/// paragraph — so `y = 0` already matches real UA behavior for that shape.
/// A float placed mid-paragraph (`text <img align=left> more text`) would,
/// in a real UA, start no higher than its point in the flow; here it starts
/// at the IFC's top instead, floating "above" text that precedes it in
/// source order. Flagged, not silently dropped: no M4 fixture exercises a
/// mid-paragraph float, and the exclusion mechanism ([`line_exclusion`])
/// still correctly wraps every line under it regardless of where its box
/// starts. Revisit-trigger: a fixture needs a float's true source position.
///
/// `clear` is likewise not honored here: since floats never escape their own
/// IFC in this scope (no cross-block continuation — see the module docs),
/// there is no OTHER block/line in scope that could ever need to clear past
/// one; a `clear` on some run within this same paragraph would be
/// nonsensical (nothing after it in the same IFC could be "below" a float
/// that already spans the IFC from y=0). Deferred, not implemented — flagged
/// per the packet brief's explicit allowance to simplify/defer full `clear`
/// interaction.
///
/// Total: `specs` is already capped at [`MAX_FLOATS`]; a float wider than
/// `available_width` is clamped to exactly `available_width` (occupies the
/// whole row by itself — "text goes below" per the packet's totality
/// requirement); every dimension is pre-clamped finite/non-negative by
/// [`collect_float_specs`]. A plain `for` loop over an already-bounded slice
/// — no while-loop retry, so no infinite-loop surface at all.
fn place_floats(specs: &[(usize, FloatSide, Size)], available_width: f32) -> Vec<PositionedFloat> {
    let available_width = clamp_dim(available_width);
    let mut left_x = 0.0f32;
    let mut left_row_top = 0.0f32;
    let mut left_row_bottom = 0.0f32;
    let mut right_used = 0.0f32;
    let mut right_row_top = 0.0f32;
    let mut right_row_bottom = 0.0f32;

    let mut out = Vec::with_capacity(specs.len());
    for &(run_index, side, size) in specs {
        let w = clamp_dim(size.w).min(available_width);
        let h = clamp_dim(size.h);
        match side {
            FloatSide::Left => {
                if left_x > 0.0 && left_x + w > available_width {
                    left_row_top = left_row_bottom;
                    left_x = 0.0;
                }
                out.push(PositionedFloat {
                    run_index,
                    side,
                    rect: Rect { origin: Point { x: left_x, y: left_row_top }, size: Size { w, h } },
                });
                left_x += w;
                left_row_bottom = left_row_bottom.max(left_row_top + h);
            }
            FloatSide::Right => {
                if right_used > 0.0 && right_used + w > available_width {
                    right_row_top = right_row_bottom;
                    right_used = 0.0;
                }
                let x = (available_width - right_used - w).max(0.0);
                out.push(PositionedFloat {
                    run_index,
                    side,
                    rect: Rect { origin: Point { x, y: right_row_top }, size: Size { w, h } },
                });
                right_used += w;
                right_row_bottom = right_row_bottom.max(right_row_top + h);
            }
        }
    }
    out
}

/// The horizontal exclusion a line starting at vertical position `y` must
/// honor: `(offset_x, avail_width)` — `offset_x` is how far right the
/// line's own box must start (the widest LEFT float whose `[y0, y0+h)` span
/// contains `y`), and `avail_width` is `available_width` minus BOTH that
/// left exclusion and the narrowest gap left by any overlapping RIGHT float.
/// Uses a point-in-range test at the line's own starting `y` (not a
/// [y, y+line_height) range test) — a documented approximation: this
/// function is always called before a line's own height is known (the
/// height depends on what content ends up on it, and the exclusion decides
/// what CAN fit — a real interleaved solve isn't attempted here), so the
/// alternative would require re-solving once height is known. Since floats
/// are typically much taller than one text line, testing at the line's
/// start `y` alone tracks a real UA's "wrap around it" behavior closely
/// enough for the M4 fixtures. A zero-height float (`h == 0.0`) never
/// matches any `y` (empty half-open range) — correctly excluded from every
/// line, since it has no vertical extent to wrap around.
///
/// Total, bounded work: a plain loop over `floats`, itself capped at
/// [`MAX_FLOATS`] — called once per line, so the whole [`layout_runs`] call
/// costs at most `O(lines * MAX_FLOATS)`, both bounded.
fn line_exclusion(y: f32, floats: &[PositionedFloat], available_width: f32) -> (f32, f32) {
    let mut left = 0.0f32;
    let mut right = 0.0f32;
    for f in floats {
        if y >= f.rect.origin.y && y < f.rect.origin.y + f.rect.size.h {
            match f.side {
                FloatSide::Left => left = left.max(f.rect.origin.x + f.rect.size.w),
                FloatSide::Right => right = right.max(available_width - f.rect.origin.x),
            }
        }
    }
    let avail = (available_width - left - right).max(0.0);
    (left, avail)
}

/// The rightward shift (never negative) a completed line's fragments need
/// so the line reads as `text_align` says, within that line's own
/// float-aware available width `avail` (see [`line_exclusion`]) given the
/// line's used inline content width `content` (`cur_x` at the point the
/// line is closed — NOT `avail`, so a line narrower than `avail` centers/
/// right-aligns within the real available space, matching real UAs).
/// `Left`/`Justify` never shift (v0 treats `Justify` as `Left` — see the
/// module doc comment). A line wider than `avail` (the unbreakable-word
/// overflow case `layout_runs` already documents as total) would make the
/// raw `Center`/`Right` offset negative — clamped to `0.0` so an overflowing
/// line stays flush at its ordinary left base rather than being shoved
/// further left/off-line by a negative shift. Both inputs arrive already
/// finite/non-negative (`avail` from [`line_exclusion`], `content` from
/// [`clamp_dim`]-derived word widths), so no NaN/inf seam here.
fn align_offset(text_align: TextAlign, avail: f32, content: f32) -> f32 {
    let raw = match text_align {
        TextAlign::Center => (avail - content) / 2.0,
        TextAlign::Right => avail - content,
        TextAlign::Left | TextAlign::Justify => 0.0,
    };
    raw.max(0.0)
}

/// Shift every fragment already accumulated for the line about to close by
/// `align_offset(text_align, avail, content)` — added to each
/// [`PositionedRun::x`], which is relative to the line box's own left edge
/// ([`LineBox::rect`]'s `origin.x`, itself the float-exclusion base from
/// [`line_exclusion`]): the alignment shift stacks on top of that base
/// rather than replacing it. A no-op (skips the loop) when the offset is
/// exactly `0.0` — the overwhelmingly common `Left` case — so left-aligned
/// output is byte-identical to before this shift existed.
fn apply_line_align(positioned: &mut [PositionedRun], text_align: TextAlign, avail: f32, content: f32) {
    let offset = align_offset(text_align, avail, content);
    if offset > 0.0 {
        for r in positioned.iter_mut() {
            r.x += offset;
        }
    }
}

/// Break `runs` into lines that fit within `available_width` — placing any
/// floated replaced atoms first (see [`place_floats`]) and shortening every
/// line whose vertical span overlaps one (see [`line_exclusion`]) so text
/// wraps around it — using `metrics` for text advances. Total over any
/// input: empty `runs`, empty/whitespace-only text, non-finite/negative
/// `available_width`, single words (or replaced atoms) wider than the
/// available width (they overflow their line rather than panicking or
/// splitting), floats with no following text, many floats, a float wider
/// than the container, and a float with zero/huge/NaN intrinsic size are all
/// handled — see this module's doc comment and [`place_floats`]/
/// [`line_exclusion`]'s own totality notes.
///
/// An empty (or all-whitespace, all-floated-with-nothing-else) `runs` slice
/// with no floats produces zero lines and a zero [`Size`] — a documented
/// scope call (brief allows either "one empty line or zero lines"; zero
/// keeps empty text nodes from consuming vertical space, matching most
/// engines' handling of whitespace-only text nodes). If floats ARE present
/// even with no other content, they are still placed and returned (the
/// "floats with no following text" totality case) — the returned `size`
/// reflects their footprint so the containing block still reserves room for
/// them.
pub fn layout_runs<M: Metrics>(
    runs: &[InlineRun],
    available_width: f32,
    text_align: TextAlign,
    metrics: &M,
) -> InlineLayout {
    let available_width = if available_width.is_finite() && available_width > 0.0 { available_width } else { 0.0 };

    let float_specs = collect_float_specs(runs);
    let placed_floats = place_floats(&float_specs, available_width);

    let clusters = cluster(tokenize(runs));
    if clusters.is_empty() {
        let floats_bottom = placed_floats.iter().map(|f| f.rect.origin.y + f.rect.size.h).fold(0.0f32, f32::max);
        let floats_right = placed_floats.iter().map(|f| f.rect.origin.x + f.rect.size.w).fold(0.0f32, f32::max);
        return InlineLayout {
            size: Size { w: floats_right, h: floats_bottom },
            lines: Vec::new(),
            floats: placed_floats,
        };
    }

    let mut lines: Vec<LineBox> = Vec::new();
    let mut y = 0.0f32;
    let (mut line_offset_x, mut line_avail_width) = line_exclusion(y, &placed_floats, available_width);

    // Current line accumulator state.
    let mut cur_x = 0.0f32;
    let mut cur_positioned: Vec<PositionedRun> = Vec::new();
    let mut open: Option<PositionedRun> = None;
    let mut max_ascent = 0.0f32;
    let mut max_descent = 0.0f32;
    let mut max_line_height = 0.0f32;
    let mut max_width = 0.0f32;

    fn close_run(open: &mut Option<PositionedRun>, cur_positioned: &mut Vec<PositionedRun>) {
        if let Some(r) = open.take() {
            cur_positioned.push(r);
        }
    }

    for c in &clusters {
        let (space_before, words) = match c {
            Cluster::Break { run } => {
                // Forced break (`<br>`): end the current line unconditionally
                // — regardless of remaining width — then resume on a fresh
                // one. Mirrors the ordinary wrap logic just below (same
                // line-push/reset/re-exclude shape), except it always fires,
                // even when `cur_positioned` is still empty (a bare `<br>`,
                // or two in a row) — in that case there's no accumulated
                // ascent/descent/line-height to derive a sensible line box
                // from, so fall back to the break's own run style via
                // `resolved_line_height`/`metrics.ascent`, matching how a
                // real UA still reserves a blank line's worth of height for
                // an empty line broken by `<br>`.
                close_run(&mut open, &mut cur_positioned);
                max_width = max_width.max(line_offset_x + cur_x);
                let had_content = max_line_height > 0.0 || max_ascent > 0.0 || max_descent > 0.0;
                let (line_height, baseline) = if had_content {
                    let line_height = max_line_height.max(max_ascent + max_descent);
                    let baseline = max_ascent + (line_height - (max_ascent + max_descent)) / 2.0;
                    (line_height, baseline)
                } else {
                    let style = &runs[*run].style;
                    let line_height = clamp_dim(resolved_line_height(style, metrics));
                    let ascent = clamp_dim(metrics.ascent(style.font_size));
                    let descent = clamp_dim(metrics.descent(style.font_size));
                    let baseline = ascent + (line_height - (ascent + descent)) / 2.0;
                    (line_height, baseline)
                };
                apply_line_align(&mut cur_positioned, text_align, line_avail_width, cur_x);
                lines.push(LineBox {
                    rect: Rect { origin: Point { x: line_offset_x, y }, size: Size { w: cur_x, h: line_height } },
                    baseline,
                    runs: std::mem::take(&mut cur_positioned),
                });
                y += line_height;
                cur_x = 0.0;
                max_ascent = 0.0;
                max_descent = 0.0;
                max_line_height = 0.0;
                let (offset, avail) = line_exclusion(y, &placed_floats, available_width);
                line_offset_x = offset;
                line_avail_width = avail;
                continue;
            }
            Cluster::Words { space_before, words } => (*space_before, words),
        };

        let cluster_width: f32 = words.iter().map(|w| word_metrics(runs, w, metrics).0).sum();
        let cluster_width = clamp_dim(cluster_width);
        let space_style = &runs[words[0].run].style;
        let space_w = if space_before { clamp_dim(metrics.advance(' ', space_style.font_size)) } else { 0.0 };

        let would_add = if cur_x > 0.0 { space_w } else { 0.0 } + cluster_width;
        if cur_x > 0.0 && cur_x + would_add > line_avail_width {
            // Wrap: flush the current line, start a fresh one.
            close_run(&mut open, &mut cur_positioned);
            max_width = max_width.max(line_offset_x + cur_x);
            let line_height = max_line_height.max(max_ascent + max_descent);
            let baseline = max_ascent + (line_height - (max_ascent + max_descent)) / 2.0;
            apply_line_align(&mut cur_positioned, text_align, line_avail_width, cur_x);
            lines.push(LineBox {
                rect: Rect { origin: Point { x: line_offset_x, y }, size: Size { w: cur_x, h: line_height } },
                baseline,
                runs: std::mem::take(&mut cur_positioned),
            });
            y += line_height;
            cur_x = 0.0;
            max_ascent = 0.0;
            max_descent = 0.0;
            max_line_height = 0.0;
            let (offset, avail) = line_exclusion(y, &placed_floats, available_width);
            line_offset_x = offset;
            line_avail_width = avail;
        }

        let use_space = cur_x > 0.0 && space_before;
        for (wi, w) in words.iter().enumerate() {
            let (word_w, ascent, descent, line_h) = word_metrics(runs, w, metrics);
            // A leading space only ever precedes the cluster's first word —
            // subsequent words within a glued cluster never get one (there
            // was none in the source, that's why they're glued).
            let leading = if wi == 0 && use_space { " " } else { "" };
            let leading_w = if !leading.is_empty() { space_w } else { 0.0 };

            let start_x = cur_x;
            cur_x += leading_w + word_w;

            max_ascent = max_ascent.max(ascent);
            max_descent = max_descent.max(descent);
            max_line_height = max_line_height.max(line_h);

            match &runs[w.run].content {
                InlineContent::Text(_) => match &mut open {
                    Some(o) if o.run_index == w.run => {
                        o.text.push_str(leading);
                        o.text.push_str(&w.text);
                        o.width = cur_x - o.x;
                    }
                    _ => {
                        close_run(&mut open, &mut cur_positioned);
                        let mut text = String::new();
                        text.push_str(leading);
                        text.push_str(&w.text);
                        open = Some(PositionedRun { run_index: w.run, text, x: start_x, width: cur_x - start_x });
                    }
                },
                // An atom never merges with a neighbor (its run index is
                // unique to it — see `Word`'s doc comment) and never carries
                // the leading space in its own box (that space has no pixel
                // footprint to paint for an image): the atom's `x` starts
                // AFTER the leading space, and its `width` is exactly its
                // own clamped intrinsic width, not `cur_x - start_x`.
                InlineContent::Replaced { .. } => {
                    close_run(&mut open, &mut cur_positioned);
                    open = Some(PositionedRun {
                        run_index: w.run,
                        text: String::new(),
                        x: start_x + leading_w,
                        width: word_w,
                    });
                    close_run(&mut open, &mut cur_positioned);
                }
            }
        }
    }

    // Flush the final line.
    close_run(&mut open, &mut cur_positioned);
    if !cur_positioned.is_empty() {
        max_width = max_width.max(line_offset_x + cur_x);
        let line_height = max_line_height.max(max_ascent + max_descent);
        let baseline = max_ascent + (line_height - (max_ascent + max_descent)) / 2.0;
        apply_line_align(&mut cur_positioned, text_align, line_avail_width, cur_x);
        lines.push(LineBox {
            rect: Rect { origin: Point { x: line_offset_x, y }, size: Size { w: cur_x, h: line_height } },
            baseline,
            runs: cur_positioned,
        });
        y += line_height;
    }

    let floats_bottom = placed_floats.iter().map(|f| f.rect.origin.y + f.rect.size.h).fold(0.0f32, f32::max);
    let floats_right = placed_floats.iter().map(|f| f.rect.origin.x + f.rect.size.w).fold(0.0f32, f32::max);
    max_width = max_width.max(floats_right);
    let total_h = y.max(floats_bottom);

    InlineLayout { size: Size { w: max_width, h: total_h }, lines, floats: placed_floats }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::computed::LineHeight as LH;

    /// Every glyph advances 10px; ascent 8px, descent 2px, line-height 10px
    /// (matching ascent+descent, so baseline math is exactly assertable).
    struct FixedMetrics;

    impl Metrics for FixedMetrics {
        fn ascent(&self, _size_px: f32) -> f32 {
            8.0
        }
        fn descent(&self, _size_px: f32) -> f32 {
            2.0
        }
        fn line_height(&self, _size_px: f32) -> f32 {
            10.0
        }
        fn advance(&self, _ch: char, _size_px: f32) -> f32 {
            10.0
        }
    }

    fn run(text: &str) -> InlineRun {
        InlineRun { content: InlineContent::Text(text.to_string()), style: ComputedStyle::default(), interactive: None }
    }

    fn run_with(text: &str, mut f: impl FnMut(&mut ComputedStyle)) -> InlineRun {
        let mut style = ComputedStyle::default();
        f(&mut style);
        InlineRun { content: InlineContent::Text(text.to_string()), style, interactive: None }
    }

    /// A non-floated replaced atom (M4 part 2): sits inline like a word.
    fn atom(w: f32, h: f32) -> InlineRun {
        InlineRun {
            content: InlineContent::Replaced { intrinsic: Size { w, h }, image: None },
            style: ComputedStyle::default(),
            interactive: None,
        }
    }

    /// A floated replaced atom (M4 part 3): pulled out of line flow.
    fn float_atom(w: f32, h: f32, side: CssFloat) -> InlineRun {
        let mut style = ComputedStyle::default();
        style.float = side;
        InlineRun { content: InlineContent::Replaced { intrinsic: Size { w, h }, image: None }, style, interactive: None }
    }

    #[test]
    fn empty_runs_produce_zero_lines() {
        let out = layout_runs::<FixedMetrics>(&[], 1000.0, TextAlign::Left, &FixedMetrics);
        assert_eq!(out.lines.len(), 0);
        assert_eq!(out.size, Size { w: 0.0, h: 0.0 });
    }

    #[test]
    fn whitespace_only_run_produces_zero_lines() {
        let runs = [run("   \t\n  ")];
        let out = layout_runs(&runs, 1000.0, TextAlign::Left, &FixedMetrics);
        assert_eq!(out.lines.len(), 0);
    }

    #[test]
    fn single_line_fits() {
        // "ab cd" -> 5 chars * 10px = 50px, well within 1000px.
        let runs = [run("ab cd")];
        let out = layout_runs(&runs, 1000.0, TextAlign::Left, &FixedMetrics);
        assert_eq!(out.lines.len(), 1);
        let line = &out.lines[0];
        assert_eq!(line.runs.len(), 1);
        assert_eq!(line.runs[0].text, "ab cd");
        assert_eq!(line.runs[0].x, 0.0);
        assert_eq!(line.baseline, 8.0);
        assert_eq!(line.rect.size.h, 10.0);
        // width: "ab"(20) + space(10) + "cd"(20) = 50
        assert_eq!(out.size.w, 50.0);
        assert_eq!(out.size.h, 10.0);
    }

    #[test]
    fn wraps_at_the_right_word() {
        // Three words "aa bb cc", each word = 20px, space = 10px.
        // available_width = 45px: "aa bb" = 20+10+20 = 50 > 45, so "aa"(20)
        // fits alone (20 <= 45), then "bb" needs 20+10=30 more -> 20+30=50 >
        // 45, wraps. Line 1: "aa". Line 2 starts with "bb"; "bb cc" = 20+10+
        // 20=50 > 45 too, so line 2: "bb", line 3: "cc".
        let runs = [run("aa bb cc")];
        let out = layout_runs(&runs, 45.0, TextAlign::Left, &FixedMetrics);
        assert_eq!(out.lines.len(), 3);
        assert_eq!(out.lines[0].runs[0].text, "aa");
        assert_eq!(out.lines[1].runs[0].text, "bb");
        assert_eq!(out.lines[2].runs[0].text, "cc");
        for line in &out.lines {
            assert_eq!(line.runs[0].x, 0.0);
        }
    }

    #[test]
    fn multiple_wraps_with_exact_line_count_and_positions() {
        // Four words of 2 chars each = 20px, available width 50px fits
        // exactly two words + one space (20+10+20=50) per line.
        let runs = [run("aa bb cc dd")];
        let out = layout_runs(&runs, 50.0, TextAlign::Left, &FixedMetrics);
        assert_eq!(out.lines.len(), 2);

        let l0 = &out.lines[0];
        assert_eq!(l0.runs.len(), 1);
        assert_eq!(l0.runs[0].text, "aa bb");
        assert_eq!(l0.runs[0].x, 0.0);
        assert_eq!(l0.runs[0].width, 50.0);
        assert_eq!(l0.baseline, 8.0);
        assert_eq!(l0.rect.origin.y, 0.0);

        let l1 = &out.lines[1];
        assert_eq!(l1.runs.len(), 1);
        assert_eq!(l1.runs[0].text, "cc dd");
        assert_eq!(l1.runs[0].x, 0.0);
        assert_eq!(l1.rect.origin.y, 10.0);

        assert_eq!(out.size.w, 50.0);
        assert_eq!(out.size.h, 20.0);
    }

    #[test]
    fn unbreakable_overflow_does_not_panic() {
        // A single "word" of 20 chars (200px) with only 30px available: it
        // must not split, and must not panic — it just overflows its line.
        let runs = [run("aaaaaaaaaaaaaaaaaaaa")];
        let out = layout_runs(&runs, 30.0, TextAlign::Left, &FixedMetrics);
        assert_eq!(out.lines.len(), 1);
        assert_eq!(out.lines[0].runs[0].text, "aaaaaaaaaaaaaaaaaaaa");
        assert_eq!(out.lines[0].runs[0].width, 200.0);
        assert!(out.size.w >= 200.0);
    }

    #[test]
    fn overlong_word_among_others_gets_its_own_line() {
        let runs = [run("hi aaaaaaaaaaaaaaaaaaaa bye")];
        let out = layout_runs(&runs, 30.0, TextAlign::Left, &FixedMetrics);
        // "hi" alone (20px), long word alone (200px), "bye" alone (30px).
        assert_eq!(out.lines.len(), 3);
        assert_eq!(out.lines[0].runs[0].text, "hi");
        assert_eq!(out.lines[1].runs[0].text, "aaaaaaaaaaaaaaaaaaaa");
        assert_eq!(out.lines[2].runs[0].text, "bye");
    }

    #[test]
    fn per_run_x_positions_across_multiple_source_runs() {
        // Two source runs sharing one line: "foo" (run 0) then " bar" via a
        // second run " bar" (run 1) — space belongs to run 1's leading text.
        let runs = [run("foo"), run(" bar")];
        let out = layout_runs(&runs, 1000.0, TextAlign::Left, &FixedMetrics);
        assert_eq!(out.lines.len(), 1);
        let positioned = &out.lines[0].runs;
        assert_eq!(positioned.len(), 2);
        assert_eq!(positioned[0].run_index, 0);
        assert_eq!(positioned[0].text, "foo");
        assert_eq!(positioned[0].x, 0.0);
        assert_eq!(positioned[1].run_index, 1);
        assert_eq!(positioned[1].text, " bar");
        assert_eq!(positioned[1].x, 30.0); // after "foo"(30) + no extra: space is part of run 1's text
    }

    #[test]
    fn glued_cross_run_word_stays_on_one_line() {
        // No whitespace between "bold" (run 0) and "text" (run 1): one
        // unbreakable visual word, must never split across lines even
        // though as separate PositionedRuns.
        let runs = [run("bold"), run("text")];
        let out = layout_runs(&runs, 70.0, TextAlign::Left, &FixedMetrics); // "boldtext" = 80px > 70px
        assert_eq!(out.lines.len(), 1, "glued word must not split even though it overflows");
        assert_eq!(out.lines[0].runs.len(), 2);
        assert_eq!(out.lines[0].runs[0].text, "bold");
        assert_eq!(out.lines[0].runs[0].x, 0.0);
        assert_eq!(out.lines[0].runs[1].text, "text");
        assert_eq!(out.lines[0].runs[1].x, 40.0);
    }

    #[test]
    fn negative_or_nan_available_width_does_not_panic() {
        for w in [-10.0, f32::NAN, f32::NEG_INFINITY, 0.0] {
            let runs = [run("aa bb cc")];
            let out = layout_runs(&runs, w, TextAlign::Left, &FixedMetrics);
            // Every word overflows its own (zero-width) line rather than panicking.
            assert_eq!(out.lines.len(), 3);
        }
    }

    #[test]
    fn explicit_line_height_overrides_metrics() {
        let runs = [run_with("hi", |s| s.line_height = LH::Px(40.0))];
        let out = layout_runs(&runs, 1000.0, TextAlign::Left, &FixedMetrics);
        assert_eq!(out.lines.len(), 1);
        assert_eq!(out.lines[0].rect.size.h, 40.0);
        // Half-leading centers the 10px (ascent+descent) glyph box in the
        // 40px line box: extra 30px split 15/15, baseline = ascent(8) + 15.
        assert_eq!(out.lines[0].baseline, 23.0);
    }

    #[test]
    fn deeply_nested_many_runs_stays_total() {
        let runs: Vec<InlineRun> = (0..500).map(|i| run(&format!("w{i} "))).collect();
        let out = layout_runs(&runs, 200.0, TextAlign::Left, &FixedMetrics);
        assert!(!out.lines.is_empty());
        assert!(out.size.h > 0.0);
    }

    // -----------------------------------------------------------------
    // M4 part 2: non-floated inline replaced atoms.
    // -----------------------------------------------------------------

    #[test]
    fn non_floated_replaced_atom_sits_inline_between_words() {
        // "hi <atom 15x8> bye" all on one line: "hi"(20) + space(10) +
        // atom(15) + space(10) + "bye"(30) = 85, well within 1000px.
        let runs = [run("hi "), atom(15.0, 8.0), run(" bye")];
        let out = layout_runs(&runs, 1000.0, TextAlign::Left, &FixedMetrics);
        assert_eq!(out.lines.len(), 1);
        let positioned = &out.lines[0].runs;
        assert_eq!(positioned.len(), 3, "text, atom, text — three positioned runs on one line");
        assert_eq!(positioned[0].run_index, 0);
        assert_eq!(positioned[0].text, "hi");
        assert_eq!(positioned[1].run_index, 1);
        assert_eq!(positioned[1].text, "", "an atom carries no glyph text");
        assert_eq!(positioned[1].width, 15.0, "atom width is its own intrinsic width, not glyph-measured");
        // "hi" ends at x=20; a space (10px) precedes the atom -> atom.x = 30.
        assert_eq!(positioned[1].x, 30.0);
        assert_eq!(positioned[2].run_index, 2);
        assert_eq!(positioned[2].text, " bye");
    }

    #[test]
    fn tall_inline_atom_grows_the_line_box() {
        // The atom (h=50) is far taller than the text's own ascent+descent
        // (8+2=10) or the fixed 10px line-height, so its ascent (=50) alone
        // sets the line's baseline (matching the atom's own bottom edge),
        // and the line's total height is that baseline PLUS the tallest
        // descent of anything else sharing the line ("hi"'s descent, 2px) —
        // real inline layout: an image doesn't shrink a shorter neighbor's
        // descent away, it just becomes the line's dominant ascent.
        let runs = [run("hi "), atom(15.0, 50.0)];
        let out = layout_runs(&runs, 1000.0, TextAlign::Left, &FixedMetrics);
        assert_eq!(out.lines.len(), 1);
        assert_eq!(out.lines[0].baseline, 50.0, "baseline sits at the atom's bottom edge");
        assert_eq!(out.lines[0].rect.size.h, 52.0, "line height = atom ascent(50) + text descent(2)");
    }

    #[test]
    fn atom_wider_than_available_width_overflows_its_own_line_not_split() {
        let runs = [atom(500.0, 20.0)];
        let out = layout_runs(&runs, 30.0, TextAlign::Left, &FixedMetrics);
        assert_eq!(out.lines.len(), 1);
        assert_eq!(out.lines[0].runs[0].width, 500.0);
    }

    #[test]
    fn atom_wraps_to_its_own_line_when_it_does_not_fit_after_text() {
        // "aa"(20px) then an atom(15px): together 20+10(space)+15=45 > 40.
        let runs = [run("aa "), atom(15.0, 8.0)];
        let out = layout_runs(&runs, 40.0, TextAlign::Left, &FixedMetrics);
        assert_eq!(out.lines.len(), 2);
        assert_eq!(out.lines[0].runs[0].text, "aa");
        assert_eq!(out.lines[1].runs[0].run_index, 1);
        assert_eq!(out.lines[1].runs[0].x, 0.0, "wrapped atom starts a fresh line at x=0");
    }

    #[test]
    fn atom_with_nonfinite_or_negative_intrinsic_does_not_panic() {
        for (w, h) in [(f32::NAN, f32::INFINITY), (-5.0, -5.0), (f32::NEG_INFINITY, f32::NAN)] {
            let runs = [atom(w, h)];
            let out = layout_runs(&runs, 100.0, TextAlign::Left, &FixedMetrics);
            assert_eq!(out.lines.len(), 1);
            assert!(out.lines[0].runs[0].width.is_finite());
            assert!(out.size.w.is_finite() && out.size.h.is_finite());
        }
    }

    #[test]
    fn glued_word_next_to_atom_with_no_whitespace_stays_together() {
        // No whitespace between "x" and the atom -> unbreakable cluster.
        let runs = [run("x"), atom(60.0, 8.0)];
        let out = layout_runs(&runs, 50.0, TextAlign::Left, &FixedMetrics); // "x"(10) + atom(60) = 70 > 50
        assert_eq!(out.lines.len(), 1, "glued atom+text must not split even though it overflows");
        assert_eq!(out.lines[0].runs.len(), 2);
        assert_eq!(out.lines[0].runs[0].x, 0.0);
        assert_eq!(out.lines[0].runs[1].x, 10.0, "atom starts right after the glued text, no space inserted");
    }

    // -----------------------------------------------------------------
    // M4 part 3: floats + exclusion.
    // -----------------------------------------------------------------

    #[test]
    fn left_float_offsets_and_shortens_overlapping_lines() {
        // A 40x30 left float, then enough text to span past its bottom.
        // Available width 100: overlapping lines get 60px (100-40), fitting
        // two 20px words + one 10px space (50px) each; three lines of that
        // (30px of height) exactly reach the float's y=30 bottom, so the
        // fourth line starts right at y=30 and must return to full width.
        let runs = [float_atom(40.0, 30.0, CssFloat::Left), run("aa bb cc dd ee ff gg hh")];
        let out = layout_runs(&runs, 100.0, TextAlign::Left, &FixedMetrics);
        assert_eq!(out.floats.len(), 1);
        assert_eq!(out.floats[0].side, FloatSide::Left);
        assert_eq!(out.floats[0].rect, Rect { origin: Point { x: 0.0, y: 0.0 }, size: Size { w: 40.0, h: 30.0 } });

        // First line (y=0, inside the float's [0,30) span): offset 40,
        // avail 60 -> "aa bb"(20+10+20=50) fits, "cc" doesn't (50+10+20=80>60).
        assert_eq!(out.lines[0].rect.origin.x, 40.0);
        assert_eq!(out.lines[0].runs[0].text, "aa bb");
        assert_eq!(out.lines[1].rect.origin.x, 40.0);
        assert_eq!(out.lines[1].runs[0].text, "cc dd");
        assert_eq!(out.lines[2].rect.origin.x, 40.0);
        assert_eq!(out.lines[2].runs[0].text, "ee ff");

        // A line starting past y=30 (the float's bottom) returns to the
        // full 100px width and x=0.
        let below = out.lines.iter().find(|l| l.rect.origin.y >= 30.0).expect("a line below the float");
        assert_eq!(below.rect.origin.x, 0.0);
        assert_eq!(below.runs[0].text, "gg hh");
    }

    #[test]
    fn right_float_only_shortens_available_width_not_the_start_x() {
        let runs = [float_atom(40.0, 30.0, CssFloat::Right), run("aa bb cc dd")];
        let out = layout_runs(&runs, 100.0, TextAlign::Left, &FixedMetrics);
        assert_eq!(out.floats[0].side, FloatSide::Right);
        assert_eq!(out.floats[0].rect.origin.x, 60.0); // 100 - 40
        assert_eq!(out.lines[0].rect.origin.x, 0.0, "a right float never offsets the line's start x");
        assert_eq!(out.lines[0].runs[0].text, "aa bb"); // same 60px effective width as the left-float case
    }

    #[test]
    fn floats_with_no_following_text_are_still_placed_and_sized() {
        let runs = [float_atom(40.0, 30.0, CssFloat::Left)];
        let out = layout_runs(&runs, 100.0, TextAlign::Left, &FixedMetrics);
        assert_eq!(out.lines.len(), 0, "no text/atoms -> zero lines, per the empty-runs scope call");
        assert_eq!(out.floats.len(), 1, "the float itself must still be placed and returned");
        assert_eq!(out.size, Size { w: 40.0, h: 30.0 }, "size reflects the float's own footprint");
    }

    #[test]
    fn multiple_same_side_floats_stack_then_wrap_to_a_new_row() {
        // Three 40px-wide left floats in a 100px container: first two fit
        // side by side (40+40=80<=100), the third doesn't (80+40=120>100)
        // and drops to a new row below the tallest of the first two.
        let runs = [
            float_atom(40.0, 20.0, CssFloat::Left),
            float_atom(40.0, 50.0, CssFloat::Left),
            float_atom(40.0, 10.0, CssFloat::Left),
        ];
        let out = layout_runs(&runs, 100.0, TextAlign::Left, &FixedMetrics);
        assert_eq!(out.floats.len(), 3);
        assert_eq!(out.floats[0].rect.origin, Point { x: 0.0, y: 0.0 });
        assert_eq!(out.floats[1].rect.origin, Point { x: 40.0, y: 0.0 });
        assert_eq!(out.floats[2].rect.origin, Point { x: 0.0, y: 50.0 }, "wraps below the taller of the first row");
    }

    #[test]
    fn alternating_left_and_right_floats_do_not_panic() {
        let runs = [
            float_atom(20.0, 10.0, CssFloat::Left),
            float_atom(20.0, 10.0, CssFloat::Right),
            float_atom(20.0, 10.0, CssFloat::Left),
            float_atom(20.0, 10.0, CssFloat::Right),
        ];
        let out = layout_runs(&runs, 100.0, TextAlign::Left, &FixedMetrics);
        assert_eq!(out.floats.len(), 4);
        for f in &out.floats {
            assert!(f.rect.origin.x.is_finite() && f.rect.origin.y.is_finite());
        }
    }

    #[test]
    fn float_wider_than_container_clamps_to_full_width() {
        let runs = [float_atom(500.0, 20.0, CssFloat::Left), run("hi")];
        let out = layout_runs(&runs, 100.0, TextAlign::Left, &FixedMetrics);
        assert_eq!(out.floats[0].rect.size.w, 100.0, "clamped to the full container width");
        // Text has zero effective width on the overlapping line -> it still
        // gets placed (first-item-on-line overflow rule), not lost.
        assert_eq!(out.lines.len(), 1);
        assert_eq!(out.lines[0].runs[0].text, "hi");
    }

    #[test]
    fn float_with_nonfinite_or_zero_intrinsic_does_not_panic_or_hang() {
        for (w, h) in [(f32::NAN, f32::INFINITY), (0.0, 0.0), (-1.0, -1.0), (f32::NEG_INFINITY, f32::NAN)] {
            let runs = [float_atom(w, h, CssFloat::Left), run("hi there")];
            let out = layout_runs(&runs, 100.0, TextAlign::Left, &FixedMetrics);
            assert!(out.size.w.is_finite() && out.size.h.is_finite());
            for f in &out.floats {
                assert!(f.rect.size.w.is_finite() && f.rect.size.h.is_finite());
            }
        }
    }

    #[test]
    fn many_floats_are_bounded_and_do_not_hang() {
        let mut runs: Vec<InlineRun> = (0..2000).map(|_| float_atom(5.0, 5.0, CssFloat::Left)).collect();
        runs.push(run("done"));
        let out = layout_runs(&runs, 100.0, TextAlign::Left, &FixedMetrics);
        assert!(out.floats.len() <= MAX_FLOATS, "float placement must be bounded, not O(input)");
        assert!(out.size.w.is_finite() && out.size.h.is_finite());
    }

    #[test]
    fn clear_with_no_float_does_not_panic() {
        let runs = [run_with("hi", |s| s.clear = crate::style::computed::Clear::Left)];
        let out = layout_runs(&runs, 100.0, TextAlign::Left, &FixedMetrics);
        assert_eq!(out.lines.len(), 1);
        assert_eq!(out.lines[0].runs[0].text, "hi");
    }

    /// Code review coverage gap: `available_width == 0` combined with a
    /// float present — the classic "float starves the line to nothing and
    /// the engine spins trying to lay out the rest" hang in real engines.
    /// `layout_runs` has no while/retry construct (every loop here is a
    /// bounded `for` over an already-finite slice — clusters, words, or
    /// `MAX_FLOATS`-capped floats), so this is provably not a hang by
    /// construction, but it's cheap insurance against a future regression
    /// that reintroduces a retry loop.
    #[test]
    fn zero_width_container_with_a_float_present_returns_promptly() {
        let runs = [float_atom(10.0, 10.0, CssFloat::Left), run("aa bb cc")];
        let out = layout_runs(&runs, 0.0, TextAlign::Left, &FixedMetrics);
        // The float itself still gets placed (clamped to the zero-width
        // container per `place_floats`'s own clamp), and every word still
        // lands somewhere (first-item-on-line overflow rule) rather than
        // being lost or looping.
        assert_eq!(out.floats.len(), 1);
        assert_eq!(out.floats[0].rect.size.w, 0.0);
        assert_eq!(out.lines.len(), 3, "each word overflows its own zero-width line, none dropped");
        for line in &out.lines {
            assert!(line.rect.size.w.is_finite());
        }
    }

    // -----------------------------------------------------------------
    // text-align: center/right (packet `text-align`).
    // -----------------------------------------------------------------

    #[test]
    fn text_align_center_offsets_the_line_by_half_the_slack() {
        // "hi there" = "hi"(20) + space(10) + "there"(50) = 80px content in
        // a 200px available width -> offset = (200-80)/2 = 60.
        let runs = [run("hi there")];
        let out = layout_runs(&runs, 200.0, TextAlign::Center, &FixedMetrics);
        assert_eq!(out.lines.len(), 1);
        assert_eq!(out.lines[0].runs[0].x, 60.0);
        assert_eq!(out.lines[0].rect.size.w, 80.0, "reported line content width is unshifted");
    }

    #[test]
    fn text_align_right_offsets_the_line_by_the_full_slack() {
        // Same 80px content in 200px available -> offset = 200-80 = 120.
        let runs = [run("hi there")];
        let out = layout_runs(&runs, 200.0, TextAlign::Right, &FixedMetrics);
        assert_eq!(out.lines.len(), 1);
        assert_eq!(out.lines[0].runs[0].x, 120.0);
    }

    #[test]
    fn text_align_left_is_unchanged_offset_zero() {
        let runs = [run("hi there")];
        let out = layout_runs(&runs, 200.0, TextAlign::Left, &FixedMetrics);
        assert_eq!(out.lines[0].runs[0].x, 0.0);
    }

    #[test]
    fn text_align_center_multi_run_shifts_every_fragment_on_the_line() {
        // Two source runs sharing one line ("foo"(30) + " bar"(40) = 70px
        // content) in a 100px available width -> offset = (100-70)/2 = 15.
        let runs = [run("foo"), run(" bar")];
        let out = layout_runs(&runs, 100.0, TextAlign::Center, &FixedMetrics);
        assert_eq!(out.lines.len(), 1);
        let positioned = &out.lines[0].runs;
        assert_eq!(positioned.len(), 2);
        assert_eq!(positioned[0].x, 15.0, "first fragment shifted by the offset");
        assert_eq!(positioned[1].x, 45.0, "second fragment keeps its relative spacing, shifted by the same offset");
    }

    #[test]
    fn text_align_center_each_wrapped_line_centers_independently() {
        // Two words of 20px each with a 10px space, available width 50px:
        // each word lands alone on its own line (20+10+20=50 > wait check
        // below) -- pick widths so each line has different content width so
        // the independence is actually observable.
        // "aaaa"(40) then "bb"(20): "aaaa bb" = 40+10+20=70 > 60, so "aaaa"
        // alone on line 1 (offset (60-40)/2=10), "bb" alone on line 2
        // (offset (60-20)/2=20).
        let runs = [run("aaaa bb")];
        let out = layout_runs(&runs, 60.0, TextAlign::Center, &FixedMetrics);
        assert_eq!(out.lines.len(), 2);
        assert_eq!(out.lines[0].runs[0].text, "aaaa");
        assert_eq!(out.lines[0].runs[0].x, 10.0);
        assert_eq!(out.lines[1].runs[0].text, "bb");
        assert_eq!(out.lines[1].runs[0].x, 20.0);
    }

    #[test]
    fn text_align_offset_never_goes_negative_when_line_overflows_avail() {
        // A single unbreakable 200px word in a 30px available width: the
        // line overflows avail, so the raw center/right offset would be
        // negative -- clamped to 0, staying at the ordinary left base.
        let runs = [run("aaaaaaaaaaaaaaaaaaaa")]; // 20 chars * 10px = 200px
        for align in [TextAlign::Center, TextAlign::Right] {
            let out = layout_runs(&runs, 30.0, align, &FixedMetrics);
            assert_eq!(out.lines.len(), 1);
            assert_eq!(out.lines[0].runs[0].x, 0.0, "overflowing line never shifts negative");
        }
    }

    #[test]
    fn text_align_center_measures_within_the_float_reduced_available_width() {
        // A 40x30 left float in a 100px container leaves 60px avail for the
        // first line. "aa bb"(20+10+20=50px content) centered in that 60px
        // -> offset = (60-50)/2 = 5, ADDED ON TOP of the float's own 40px
        // line_offset_x base (per-fragment x is relative to the line box's
        // own left edge, which already sits at line_offset_x=40).
        let runs = [float_atom(40.0, 30.0, CssFloat::Left), run("aa bb")];
        let out = layout_runs(&runs, 100.0, TextAlign::Center, &FixedMetrics);
        assert_eq!(out.lines.len(), 1);
        assert_eq!(out.lines[0].rect.origin.x, 40.0, "float exclusion base unchanged");
        assert_eq!(out.lines[0].runs[0].x, 5.0, "alignment offset measured within the reduced 60px avail width");
    }
}
