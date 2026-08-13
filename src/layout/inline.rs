//! The bespoke inline engine (P6, M2 scope): break inline-level content
//! (text runs, each carrying its own [`ComputedStyle`]) into line boxes at a
//! given available width, using [`Metrics`] for glyph advances.
//!
//! RED skeleton: types are fixed so the test suite below compiles; the
//! actual line-breaking algorithm is `todo!()` pending the green commit.

use crate::layout::{Rect, Size};
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

/// Break `runs` into lines that fit within `available_width`, using `metrics`
/// for advances. See module docs; body pending (RED skeleton).
pub fn layout_runs<M: Metrics>(_runs: &[InlineRun], _available_width: f32, _metrics: &M) -> InlineLayout {
    todo!("P6: bespoke inline line-breaking (M2)")
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
