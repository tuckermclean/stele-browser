//! The bespoke inline engine (P6, M2 scope): break inline-level content
//! (text runs, each carrying its own [`ComputedStyle`]) into line boxes at a
//! given available width, using [`Metrics`] for glyph advances.
//!
//! Out of scope for M2 (see packet brief): floats / `img align=left` text
//! wrapping (later milestone), text-align other than the default left flow
//! (justify/center/right are a paint-time nicety, not attempted here),
//! bidi/complex shaping (the `Metrics` seam is shaping-free by design).
//!
//! Whitespace handling: consecutive whitespace collapses to a single space
//! (CSS `white-space: normal`), matching the curated `WhiteSpace` property's
//! only two states (`Normal`/`Pre`) — `Pre` is not yet honored (documented
//! scope call: v1 always collapses; a `Pre` fast-path is a follow-up). Line
//! breaking only happens at those collapsed-space opportunities: a single
//! "word" (including one that is itself wider than the available width)
//! never splits — it just overflows onto its own line, which keeps the
//! engine total (no panics) on any input.
//!
//! An inline "word" may itself be glued together out of pieces from more
//! than one source run when no whitespace separates them in the source
//! (e.g. `<b>bold</b>text` — no space between the two). Such glued pieces
//! are tracked as one atomic unit for the fits-on-this-line decision (so the
//! visual word is never split across lines) while still being emitted as
//! separate [`PositionedRun`]s so each retains its own style.

use crate::layout::{Point, Rect, Size};
use crate::style::computed::LineHeight;
use crate::style::ComputedStyle;
use crate::text::Metrics;

/// One inline-level content run: a span of text sharing one style. Built by
/// flattening a `LayoutNode`'s inline-level content (text, and any nested
/// `display: inline` containers) in document order.
#[derive(Debug, Clone)]
pub struct InlineRun {
    pub text: String,
    pub style: ComputedStyle,
}

/// A slice of one source [`InlineRun`] that landed on one line, positioned
/// relative to the line box's left edge (`x`), with a `width` for hit-testing
/// / painting convenience.
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
/// and the baseline offset down from `rect.origin.y`.
#[derive(Debug, Clone, PartialEq)]
pub struct LineBox {
    pub rect: Rect,
    pub baseline: f32,
    pub runs: Vec<PositionedRun>,
}

/// The result of breaking `runs` into lines at some available width: the
/// overall bounding size (width = widest line, height = sum of line heights)
/// plus the lines themselves for fragment emission.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InlineLayout {
    pub size: Size,
    pub lines: Vec<LineBox>,
}

/// One maximal run of non-whitespace characters from a single source run,
/// tagged with whether whitespace preceded it in the (flattened) token
/// stream. Glued cross-run words (no whitespace at the run boundary) are
/// separate `Word`s with `space_before = false`, so they cluster together
/// below.
struct Word {
    run: usize,
    text: String,
    space_before: bool,
}

/// Split the flattened text of `runs` into whitespace-delimited `Word`s,
/// collapsing any run of whitespace (even one spanning a run boundary) into
/// a single break opportunity. Total: handles empty runs, all-whitespace
/// runs, and runs with no whitespace at all without panicking.
fn tokenize(runs: &[InlineRun]) -> Vec<Word> {
    let mut words = Vec::new();
    let mut pending_space = false;
    let mut cur: Option<(usize, String)> = None;

    let flush = |cur: &mut Option<(usize, String)>, pending_space: &mut bool, words: &mut Vec<Word>| {
        if let Some((run, text)) = cur.take() {
            words.push(Word { run, text, space_before: *pending_space });
            *pending_space = false;
        }
    };

    for (i, r) in runs.iter().enumerate() {
        for ch in r.text.chars() {
            if ch.is_whitespace() {
                flush(&mut cur, &mut pending_space, &mut words);
                pending_space = true;
            } else {
                match &mut cur {
                    Some((run, text)) if *run == i => text.push(ch),
                    _ => {
                        flush(&mut cur, &mut pending_space, &mut words);
                        cur = Some((i, ch.to_string()));
                    }
                }
            }
        }
    }
    flush(&mut cur, &mut pending_space, &mut words);
    words
}

/// A maximal group of `Word`s with no whitespace between them: the
/// line-breaking atom (never split across lines). `space_before` says
/// whether a collapsed whitespace opportunity precedes this cluster.
struct Cluster {
    space_before: bool,
    words: Vec<Word>,
}

fn cluster(words: Vec<Word>) -> Vec<Cluster> {
    let mut clusters: Vec<Cluster> = Vec::new();
    for w in words {
        if !w.space_before {
            if let Some(last) = clusters.last_mut() {
                last.words.push(w);
                continue;
            }
        }
        let space_before = w.space_before;
        clusters.push(Cluster { space_before, words: vec![w] });
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

/// Break `runs` into lines that fit within `available_width`, using `metrics`
/// for advances. Total over any input: empty `runs`, empty/whitespace-only
/// text, non-finite/negative `available_width`, and single words wider than
/// the available width (they overflow their line rather than panicking or
/// splitting) are all handled.
///
/// An empty (or all-whitespace) `runs` slice produces zero lines and a zero
/// [`Size`] — a documented scope call (brief allows either "one empty line
/// or zero lines"; zero keeps empty text nodes from consuming vertical
/// space, matching most engines' handling of whitespace-only text nodes).
pub fn layout_runs<M: Metrics>(runs: &[InlineRun], available_width: f32, metrics: &M) -> InlineLayout {
    let available_width = if available_width.is_finite() && available_width > 0.0 { available_width } else { 0.0 };

    let clusters = cluster(tokenize(runs));
    if clusters.is_empty() {
        return InlineLayout::default();
    }

    let mut lines: Vec<LineBox> = Vec::new();
    let mut y = 0.0f32;

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
        let cluster_width: f32 =
            c.words.iter().map(|w| metrics.measure(&w.text, runs[w.run].style.font_size)).sum();
        let space_style = &runs[c.words[0].run].style;
        let space_w = if c.space_before { metrics.advance(' ', space_style.font_size) } else { 0.0 };
        let space_w = if space_w.is_finite() { space_w.max(0.0) } else { 0.0 };
        let cluster_width = if cluster_width.is_finite() { cluster_width.max(0.0) } else { 0.0 };

        let would_add = if cur_x > 0.0 { space_w } else { 0.0 } + cluster_width;
        if cur_x > 0.0 && cur_x + would_add > available_width {
            // Wrap: flush the current line, start a fresh one.
            close_run(&mut open, &mut cur_positioned);
            max_width = max_width.max(cur_x);
            let line_height = max_line_height.max(max_ascent + max_descent);
            let baseline = max_ascent + (line_height - (max_ascent + max_descent)) / 2.0;
            lines.push(LineBox {
                rect: Rect { origin: Point { x: 0.0, y }, size: Size { w: cur_x, h: line_height } },
                baseline,
                runs: std::mem::take(&mut cur_positioned),
            });
            y += line_height;
            cur_x = 0.0;
            max_ascent = 0.0;
            max_descent = 0.0;
            max_line_height = 0.0;
        }

        let use_space = cur_x > 0.0 && c.space_before;
        for (wi, w) in c.words.iter().enumerate() {
            let style = &runs[w.run].style;
            let word_w = metrics.measure(&w.text, style.font_size);
            let word_w = if word_w.is_finite() { word_w.max(0.0) } else { 0.0 };
            // A leading space only ever precedes the cluster's first word —
            // subsequent words within a glued cluster never get one (there
            // was none in the source, that's why they're glued).
            let leading = if wi == 0 && use_space { " " } else { "" };
            let leading_w = if !leading.is_empty() { space_w } else { 0.0 };

            let start_x = cur_x;
            cur_x += leading_w + word_w;

            max_ascent = max_ascent.max(metrics.ascent(style.font_size).max(0.0));
            max_descent = max_descent.max(metrics.descent(style.font_size).max(0.0));
            max_line_height = max_line_height.max(resolved_line_height(style, metrics).max(0.0));

            match &mut open {
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
            }
        }
    }

    // Flush the final line.
    close_run(&mut open, &mut cur_positioned);
    if !cur_positioned.is_empty() {
        max_width = max_width.max(cur_x);
        let line_height = max_line_height.max(max_ascent + max_descent);
        let baseline = max_ascent + (line_height - (max_ascent + max_descent)) / 2.0;
        lines.push(LineBox {
            rect: Rect { origin: Point { x: 0.0, y }, size: Size { w: cur_x, h: line_height } },
            baseline,
            runs: cur_positioned,
        });
        y += line_height;
    }

    InlineLayout { size: Size { w: max_width, h: y }, lines }
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
        InlineRun { text: text.to_string(), style: ComputedStyle::default() }
    }

    fn run_with(text: &str, mut f: impl FnMut(&mut ComputedStyle)) -> InlineRun {
        let mut style = ComputedStyle::default();
        f(&mut style);
        InlineRun { text: text.to_string(), style }
    }

    #[test]
    fn empty_runs_produce_zero_lines() {
        let out = layout_runs::<FixedMetrics>(&[], 1000.0, &FixedMetrics);
        assert_eq!(out.lines.len(), 0);
        assert_eq!(out.size, Size { w: 0.0, h: 0.0 });
    }

    #[test]
    fn whitespace_only_run_produces_zero_lines() {
        let runs = [run("   \t\n  ")];
        let out = layout_runs(&runs, 1000.0, &FixedMetrics);
        assert_eq!(out.lines.len(), 0);
    }

    #[test]
    fn single_line_fits() {
        // "ab cd" -> 5 chars * 10px = 50px, well within 1000px.
        let runs = [run("ab cd")];
        let out = layout_runs(&runs, 1000.0, &FixedMetrics);
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
        let out = layout_runs(&runs, 45.0, &FixedMetrics);
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
        let out = layout_runs(&runs, 50.0, &FixedMetrics);
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
        let out = layout_runs(&runs, 30.0, &FixedMetrics);
        assert_eq!(out.lines.len(), 1);
        assert_eq!(out.lines[0].runs[0].text, "aaaaaaaaaaaaaaaaaaaa");
        assert_eq!(out.lines[0].runs[0].width, 200.0);
        assert!(out.size.w >= 200.0);
    }

    #[test]
    fn overlong_word_among_others_gets_its_own_line() {
        let runs = [run("hi aaaaaaaaaaaaaaaaaaaa bye")];
        let out = layout_runs(&runs, 30.0, &FixedMetrics);
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
        let out = layout_runs(&runs, 1000.0, &FixedMetrics);
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
        let out = layout_runs(&runs, 70.0, &FixedMetrics); // "boldtext" = 80px > 70px
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
            let out = layout_runs(&runs, w, &FixedMetrics);
            // Every word overflows its own (zero-width) line rather than panicking.
            assert_eq!(out.lines.len(), 3);
        }
    }

    #[test]
    fn explicit_line_height_overrides_metrics() {
        let runs = [run_with("hi", |s| s.line_height = LH::Px(40.0))];
        let out = layout_runs(&runs, 1000.0, &FixedMetrics);
        assert_eq!(out.lines.len(), 1);
        assert_eq!(out.lines[0].rect.size.h, 40.0);
        // Half-leading centers the 10px (ascent+descent) glyph box in the
        // 40px line box: extra 30px split 15/15, baseline = ascent(8) + 15.
        assert_eq!(out.lines[0].baseline, 23.0);
    }

    #[test]
    fn deeply_nested_many_runs_stays_total() {
        let runs: Vec<InlineRun> = (0..500).map(|i| run(&format!("w{i} "))).collect();
        let out = layout_runs(&runs, 200.0, &FixedMetrics);
        assert!(!out.lines.is_empty());
        assert!(out.size.h > 0.0);
    }
}
