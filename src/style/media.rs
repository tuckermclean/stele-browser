//! `@media` query parsing + evaluation (M5). `style::parser` now PARSES an
//! `@media` block's condition and nested rules (instead of discarding them,
//! see `parser::Stylesheet::media_rules`) but never evaluates it — that needs
//! a viewport, and the frozen `parse(css) -> Stylesheet` signature carries
//! none. This module is the missing evaluation step, kept entirely outside
//! `parse`/`cascade` so neither frozen signature has to grow a viewport
//! parameter: [`flatten_media`] is a PRE-PASS callers run between `parse`
//! and `cascade`, folding every matching `@media` block's rules into an
//! ordinary media-free `Stylesheet` that `cascade` then sees exactly as if
//! the `@media` had never existed.
//!
//! ## Curated subset (brief-style scoping, charter C2's ignore-unknown/
//! fail-closed treaty applied to media queries)
//!
//! - Media types: `all`, `screen` (both match — this is a screen renderer);
//!   `print` never matches (we never render to a print medium); any other
//!   named type (`speech`, `braille`, ...) is unsupported and never matches.
//!   A leading `not`/`only` is tokenized but `not` negation isn't
//!   implemented (out of scope) — a `not`-prefixed query item fails closed
//!   (never matches) rather than silently inverting incorrectly.
//! - Features: `(min-width: <px>)`, `(max-width: <px>)`, `(width: <px>)`,
//!   combined with `and` (AND within one comma-separated item). Only `px`
//!   lengths (and unitless `0`) are recognized — `em`/`%`/`pt` in a media
//!   feature is unsupported (a viewport has no font-size/parent to resolve
//!   `em`/`%` against the way a property value does) and fails closed.
//! - A comma-separated list of items is OR: the query matches if ANY item
//!   matches.
//! - Height features (`min-height`/`max-height`) are OUT OF SCOPE: every
//!   render mode in this codebase lays out at a content-driven,
//!   effectively-unbounded viewport height (`HEADLESS_VIEWPORT_HEIGHT` /
//!   `SUBDOC_VIEWPORT_HEIGHT` = 100,000px — see `main.rs`/`frames.rs`), so a
//!   height media feature would almost always evaluate against that
//!   sentinel rather than anything a document author could reason about.
//!   Width is the one dimension every render mode actually varies (tty
//!   `--cols`, PNG width, framebuffer width, frame region width).
//! - Any query that doesn't parse into a recognized type/feature shape
//!   (bad syntax, an empty condition, an unrecognized feature name, a
//!   trailing token after a well-formed feature) fails closed: it never
//!   matches, rather than defaulting to "matches everything" — the C2 way,
//!   applied to media the same way it's already applied to declarations and
//!   selectors elsewhere in this module tree.
//!
//! Total: [`MediaQuery::parse`] and [`MediaQuery::matches`] never panic on
//! any token stream / viewport width, including empty, garbage, or
//! pathologically long input — the parser below only ever advances an index
//! or breaks out of its loops, never loops unboundedly.

use crate::style::parser::{Stylesheet, StyleRule};
use crate::style::tokenizer::Token;

/// The `<media-type>` half of one comma-separated query item. `All` is both
/// the explicit `all` keyword AND the implicit default when a query item
/// opens straight with a feature (e.g. `(min-width: 800px)` with no leading
/// type keyword — very common, real CSS shorthand for "all and (...)").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaType {
    All,
    Screen,
    Print,
}

/// One `(feature: value)` test, value already resolved to px (brief above:
/// px/unitless-0 only).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Feature {
    MinWidth(f32),
    MaxWidth(f32),
    Width(f32),
}

impl Feature {
    fn matches(&self, viewport_width_px: f32) -> bool {
        match *self {
            Feature::MinWidth(v) => viewport_width_px >= v,
            Feature::MaxWidth(v) => viewport_width_px <= v,
            // Exact `width` match: real CSS also does exact float
            // comparison here; a tiny epsilon just absorbs f32 rounding
            // noise from unit conversions elsewhere in the pipeline.
            Feature::Width(v) => (viewport_width_px - v).abs() < 0.5,
        }
    }
}

/// One comma-separated query item: `[not|only] <type> [and (<feature>)]*`
/// or a bare `(<feature>) [and (<feature>)]*` (implicit `all`). `supported`
/// is the fail-closed flag (charter C2): set the moment anything in this
/// item falls outside the curated subset above, so [`QueryItem::matches`]
/// can short-circuit to "never" without needing to represent "unsupported"
/// as its own `Feature`/`MediaType` variant.
#[derive(Debug, Clone)]
struct QueryItem {
    media_type: MediaType,
    features: Vec<Feature>,
    supported: bool,
}

impl QueryItem {
    fn matches(&self, viewport_width_px: f32) -> bool {
        if !self.supported {
            return false;
        }
        if self.media_type == MediaType::Print {
            return false;
        }
        self.features.iter().all(|f| f.matches(viewport_width_px))
    }
}

/// A parsed `@media` condition: a comma-separated OR of [`QueryItem`]s.
/// `Default` (empty `items`) never matches anything — the same fail-closed
/// shape [`QueryItem::supported`] uses, just at the top level (an `@media`
/// whose condition tokens were empty/entirely unparseable).
#[derive(Debug, Clone, Default)]
pub(crate) struct MediaQuery {
    items: Vec<QueryItem>,
}

impl MediaQuery {
    /// Parse the raw token slice between `@media` and its `{`/`;` (may be
    /// empty, may be garbage — never panics). Comma-splits at paren-depth 0
    /// into independent OR'd items, each parsed by [`parse_item`].
    pub(crate) fn parse(tokens: &[Token]) -> MediaQuery {
        let items = split_top_level_commas(tokens).into_iter().map(|g| parse_item(&g)).collect();
        MediaQuery { items }
    }

    /// Does this query match a screen render at `viewport_width_px`? OR
    /// across every comma-separated item.
    pub(crate) fn matches(&self, viewport_width_px: f32) -> bool {
        self.items.iter().any(|item| item.matches(viewport_width_px))
    }
}

/// Split `tokens` on top-level (paren-depth 0) commas. The curated feature
/// grammar never itself contains a comma inside `(...)`, but this tracks
/// depth anyway rather than assuming that, so a hostile/garbage condition
/// with stray parens still can't misparse a comma inside them as a query
/// separator.
fn split_top_level_commas(tokens: &[Token]) -> Vec<Vec<Token>> {
    let mut groups = Vec::new();
    let mut current: Vec<Token> = Vec::new();
    let mut depth = 0i32;
    for t in tokens {
        match t {
            Token::LParen => {
                depth += 1;
                current.push(t.clone());
            }
            Token::RParen => {
                depth -= 1;
                current.push(t.clone());
            }
            Token::Comma if depth <= 0 => {
                groups.push(std::mem::take(&mut current));
            }
            _ => current.push(t.clone()),
        }
    }
    groups.push(current);
    groups
}

/// A media-feature value: only `px` dimensions and unitless `0` (brief
/// above) — `em`/`%`/`pt` have no viewport-relative meaning here.
fn feature_length_px(t: &Token) -> Option<f32> {
    match t {
        Token::Dimension(v, unit) if unit.eq_ignore_ascii_case("px") => Some(*v),
        Token::Number(v) if *v == 0.0 => Some(0.0),
        _ => None,
    }
}

/// Parse one comma-separated query item's tokens (whitespace not yet
/// stripped) into a [`QueryItem`]. Never panics: every loop below either
/// strictly advances its index or breaks, bounded by `tokens.len()`, so
/// even a pathological/garbage token stream terminates.
fn parse_item(tokens: &[Token]) -> QueryItem {
    let toks: Vec<Token> = tokens.iter().filter(|t| **t != Token::Whitespace).cloned().collect();
    let len = toks.len();
    let mut i = 0usize;
    let mut supported = true;
    let mut negated = false;
    let mut media_type = MediaType::All;
    let mut type_seen = false;

    // Leading `only`/`not` keyword(s) — `only` is a no-op we skip past;
    // `not` marks the item as negated (unsupported: negation fails closed
    // below rather than being evaluated, since real inversion isn't
    // implemented).
    while let Some(Token::Ident(kw)) = toks.get(i) {
        let lower = kw.to_ascii_lowercase();
        if lower == "only" {
            i += 1;
        } else if lower == "not" {
            negated = true;
            i += 1;
        } else {
            break;
        }
    }

    // Optional media type keyword.
    if let Some(Token::Ident(name)) = toks.get(i) {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "all" => {
                media_type = MediaType::All;
                i += 1;
                type_seen = true;
            }
            "screen" => {
                media_type = MediaType::Screen;
                i += 1;
                type_seen = true;
            }
            "print" => {
                media_type = MediaType::Print;
                i += 1;
                type_seen = true;
            }
            // `and` here means the item opened straight with a feature
            // (implicit `all`) — leave type_seen false, don't consume it;
            // the feature loop below handles the leading `and` itself.
            "and" => {}
            _ => {
                // Unrecognized type keyword (e.g. `speech`) — fails closed.
                supported = false;
                i += 1;
                type_seen = true;
            }
        }
    }

    // A recognized type keyword (or the implicit-`all` bare-feature case) is
    // optionally followed by `and (<feature>)` clauses; the loop below
    // consumes any `and` itself (uniformly, whether it's the one right
    // after the type keyword or one joining two feature clauses), so no
    // separate consumption happens here.
    let mut features = Vec::new();
    loop {
        // Skip `and` token(s) joining the type/previous feature to the next
        // feature clause — tracked so a DANGLING `and` (consumed here but
        // with nothing recognizable following it, e.g. `"screen and"`) is
        // caught below rather than silently treated as "no more features".
        let mut consumed_and = false;
        while let Some(Token::Ident(k)) = toks.get(i) {
            if k.eq_ignore_ascii_case("and") {
                i += 1;
                consumed_and = true;
            } else {
                break;
            }
        }
        if i >= len {
            if consumed_and {
                supported = false; // dangling `and` with nothing after it
            }
            break;
        }
        if toks.get(i) != Some(&Token::LParen) {
            supported = false; // stray token where a feature or EOF was expected
            break;
        }
        i += 1;
        let name = match toks.get(i) {
            Some(Token::Ident(n)) => n.to_ascii_lowercase(),
            _ => {
                supported = false;
                break;
            }
        };
        i += 1;
        if toks.get(i) != Some(&Token::Colon) {
            supported = false;
            break;
        }
        i += 1;
        let value = toks.get(i).and_then(feature_length_px);
        i += 1;
        if toks.get(i) != Some(&Token::RParen) {
            supported = false;
            break;
        }
        i += 1;

        match (name.as_str(), value) {
            ("min-width", Some(v)) => features.push(Feature::MinWidth(v)),
            ("max-width", Some(v)) => features.push(Feature::MaxWidth(v)),
            ("width", Some(v)) => features.push(Feature::Width(v)),
            _ => supported = false, // unknown feature name or unparseable value
        }
        if !supported {
            break;
        }
    }

    if i < len {
        supported = false; // trailing junk after the last recognized feature
    }
    if negated {
        supported = false; // `not` negation: unimplemented, fails closed
    }
    if !type_seen && features.is_empty() {
        // Nothing recognizable at all (empty or pure-garbage condition).
        supported = false;
    }

    QueryItem { media_type, features, supported }
}

/// Pre-pass (M5): fold every `@media` block in `sheet` whose query matches
/// `viewport_width_px` into an ordinary, media-free `Stylesheet` — the
/// counters (`ignored_declarations`/`media_at_rules`/`ignored_at_rules`)
/// pass through unchanged (this is purely a `rules`/`media_rules`
/// transform, not a re-parse), and the output's own `media_rules` is always
/// empty (nothing left to flatten a second time).
///
/// `cascade` never has to know `@media` existed: every [`StyleRule`],
/// whether originally top-level or nested inside a matching `@media` block,
/// already carries a correct GLOBAL `order` (the parser shares one counter
/// across both — see `parser::parse`'s doc comments), so folding a matching
/// block's rules into `rules` (in any order — `cascade` doesn't scan
/// `rules` positionally, it sorts candidates by that `order` field) makes a
/// later-in-source matching `@media` rule naturally outrank an earlier
/// equal-specificity top-level rule on a cascade tie, exactly as if the
/// `@media` block had never existed and its declarations had been written
/// inline at that point in the source.
///
/// Total and bounded: one linear pass over `sheet.media_rules`, no
/// recursion — a sheet with thousands of `@media` blocks (or thousands of
/// rules inside one) flattens in time proportional to their total size.
pub(crate) fn flatten_media(sheet: &Stylesheet, viewport_width_px: f32) -> Stylesheet {
    let mut rules: Vec<StyleRule> = sheet.rules.clone();
    for m in &sheet.media_rules {
        if m.query.matches(viewport_width_px) {
            rules.extend(m.rules.iter().cloned());
        }
    }
    Stylesheet {
        ignored_declarations: sheet.ignored_declarations,
        media_at_rules: sheet.media_at_rules,
        ignored_at_rules: sheet.ignored_at_rules,
        media_rules: Vec::new(),
        rules,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::tokenizer::tokenize;

    fn toks(css: &str) -> Vec<Token> {
        tokenize(css)
    }

    fn query(condition: &str) -> MediaQuery {
        MediaQuery::parse(&toks(condition))
    }

    #[test]
    fn max_width_matches_narrower_viewport_not_wider() {
        let q = query("(max-width: 500px)");
        assert!(q.matches(320.0));
        assert!(!q.matches(640.0));
    }

    #[test]
    fn min_width_matches_wider_viewport_not_narrower() {
        let q = query("(min-width: 800px)");
        assert!(q.matches(1024.0));
        assert!(!q.matches(640.0));
    }

    #[test]
    fn exact_width_matches_only_that_width() {
        let q = query("(width: 640px)");
        assert!(q.matches(640.0));
        assert!(!q.matches(641.0));
        assert!(!q.matches(320.0));
    }

    #[test]
    fn screen_and_max_width_requires_both() {
        let q = query("screen and (max-width: 500px)");
        assert!(q.matches(320.0));
        assert!(!q.matches(640.0));
    }

    #[test]
    fn all_type_is_equivalent_to_no_type() {
        let with_all = query("all and (min-width: 800px)");
        let bare = query("(min-width: 800px)");
        for w in [320.0, 640.0, 800.0, 1024.0] {
            assert_eq!(with_all.matches(w), bare.matches(w));
        }
    }

    #[test]
    fn comma_separated_list_is_or() {
        let q = query("(max-width: 500px), (min-width: 1200px)");
        assert!(q.matches(320.0)); // first branch
        assert!(q.matches(1600.0)); // second branch
        assert!(!q.matches(800.0)); // neither branch
    }

    #[test]
    fn print_never_matches_a_screen_render() {
        let q = query("print");
        assert!(!q.matches(320.0));
        assert!(!q.matches(1600.0));
        assert!(!q.matches(100_000.0));
    }

    #[test]
    fn print_with_a_matching_width_feature_still_never_matches() {
        let q = query("print and (max-width: 5000px)");
        assert!(!q.matches(320.0));
    }

    #[test]
    fn unknown_media_type_never_matches() {
        let q = query("speech");
        assert!(!q.matches(320.0));
        assert!(!q.matches(1600.0));
    }

    #[test]
    fn unknown_feature_name_fails_closed() {
        let q = query("(orientation: landscape)");
        assert!(!q.matches(320.0));
        assert!(!q.matches(1600.0));
    }

    #[test]
    fn malformed_condition_never_matches_and_never_panics() {
        for cond in ["", "!!!garbage!!!", "(", ")", "(min-width", "min-width: 800px)", "screen and", "and and and"] {
            let q = query(cond);
            assert!(!q.matches(320.0), "for {cond:?}");
            assert!(!q.matches(1_000_000.0), "for {cond:?}");
        }
    }

    #[test]
    fn not_prefix_negation_is_unsupported_and_fails_closed() {
        // Real CSS: `not screen and (max-width: 500px)` would match a WIDE
        // viewport (negation). Negation isn't implemented, so this must
        // never match ANY width, rather than silently inverting wrong.
        let q = query("not screen and (max-width: 500px)");
        assert!(!q.matches(320.0));
        assert!(!q.matches(1600.0));
    }

    #[test]
    fn huge_query_list_does_not_panic() {
        let cond = (0..5000).map(|i| format!("(min-width: {i}px)")).collect::<Vec<_>>().join(", ");
        let q = query(&cond);
        assert!(q.matches(1_000_000.0));
    }

    // ---- T1b: prefers-color-scheme / ColorScheme --------------------------
    // (packet t1b-color-scheme: httpforever.com-style no-JS pages toggle dark
    // mode purely via a `data-theme` attribute a theme-toggle script sets,
    // with NO `@media (prefers-color-scheme)` fallback -- `--color-scheme`
    // gives Stele a way to pick a scheme anyway. This block covers the
    // standards-correct primitive: the media feature itself.)

    #[test]
    fn prefers_color_scheme_dark_matches_only_under_dark_scheme() {
        let q = query("(prefers-color-scheme: dark)");
        assert!(q.matches(320.0, ColorScheme::Dark));
        assert!(!q.matches(320.0, ColorScheme::Light));
    }

    #[test]
    fn prefers_color_scheme_light_matches_only_under_light_scheme() {
        let q = query("(prefers-color-scheme: light)");
        assert!(q.matches(320.0, ColorScheme::Light));
        assert!(!q.matches(320.0, ColorScheme::Dark));
    }

    #[test]
    fn color_scheme_parse_auto_resolves_to_light() {
        // No OS/JS signal exists in this renderer -- `auto` resolving to
        // `Light` is the documented, honest default (see `ColorScheme::parse`'s
        // own doc comment).
        assert_eq!(ColorScheme::parse("auto"), ColorScheme::Light);
        assert_eq!(ColorScheme::parse("AUTO"), ColorScheme::Light);
    }

    #[test]
    fn color_scheme_parse_dark_and_light_case_insensitively() {
        assert_eq!(ColorScheme::parse("dark"), ColorScheme::Dark);
        assert_eq!(ColorScheme::parse("Dark"), ColorScheme::Dark);
        assert_eq!(ColorScheme::parse("DARK"), ColorScheme::Dark);
        assert_eq!(ColorScheme::parse("light"), ColorScheme::Light);
        assert_eq!(ColorScheme::parse("Light"), ColorScheme::Light);
    }

    #[test]
    fn color_scheme_parse_garbage_falls_back_to_light_not_a_panic() {
        for v in ["", "!!!garbage!!!", "darkness", " dark ", "\0\0\0"] {
            let _ = ColorScheme::parse(v); // must not panic
        }
        assert_eq!(ColorScheme::parse("nonsense"), ColorScheme::Light);
        assert_eq!(ColorScheme::parse(""), ColorScheme::Light);
    }

    #[test]
    fn prefers_color_scheme_unknown_value_fails_closed() {
        let q = query("(prefers-color-scheme: turquoise)");
        assert!(!q.matches(320.0, ColorScheme::Light));
        assert!(!q.matches(320.0, ColorScheme::Dark));
    }

    #[test]
    fn prefers_color_scheme_combines_with_width_via_and() {
        let q = query("(min-width: 800px) and (prefers-color-scheme: dark)");
        assert!(q.matches(1024.0, ColorScheme::Dark));
        assert!(!q.matches(1024.0, ColorScheme::Light));
        assert!(!q.matches(320.0, ColorScheme::Dark));
    }

    #[test]
    fn prefers_color_scheme_malformed_value_does_not_panic() {
        for cond in ["(prefers-color-scheme:)", "(prefers-color-scheme: )", "(prefers-color-scheme: 800px)", "(prefers-color-scheme"] {
            let q = query(cond);
            assert!(!q.matches(320.0, ColorScheme::Light), "for {cond:?}");
            assert!(!q.matches(320.0, ColorScheme::Dark), "for {cond:?}");
        }
    }

    // ---- flatten_media ----------------------------------------------------

    use crate::style::parser::parse;

    #[test]
    fn flatten_media_includes_matching_block_rules_at_narrow_viewport() {
        let sheet = parse("@media (max-width: 500px) { p { color: red } } p { color: blue }");
        let narrow = flatten_media(&sheet, 320.0);
        assert!(narrow.media_rules.is_empty());
        // Two `p` rules now live in `rules`: the flattened @media one plus
        // the original top-level one.
        assert_eq!(narrow.rules.len(), 2);
    }

    #[test]
    fn flatten_media_excludes_non_matching_block_rules_at_wide_viewport() {
        let sheet = parse("@media (max-width: 500px) { p { color: red } } p { color: blue }");
        let wide = flatten_media(&sheet, 640.0);
        assert_eq!(wide.rules.len(), 1); // only the top-level rule
    }

    #[test]
    fn flatten_media_preserves_counters() {
        let sheet = parse("@media (max-width: 500px) { p { color: red } } @font-face { }");
        let flat = flatten_media(&sheet, 320.0);
        assert_eq!(flat.media_at_rules, sheet.media_at_rules);
        assert_eq!(flat.ignored_at_rules, sheet.ignored_at_rules);
    }

    #[test]
    fn flatten_media_respects_color_scheme() {
        let sheet = parse("@media (prefers-color-scheme: dark) { p { color: red } } p { color: blue }");
        let dark = flatten_media(&sheet, 320.0, ColorScheme::Dark);
        assert_eq!(dark.rules.len(), 2, "the dark-gated block should be included under a dark scheme");
        let light = flatten_media(&sheet, 320.0, ColorScheme::Light);
        assert_eq!(light.rules.len(), 1, "the dark-gated block should be excluded under a light scheme");
    }

    #[test]
    fn flatten_of_a_huge_sheet_is_bounded_and_does_not_panic() {
        let mut css = String::new();
        for i in 0..2000 {
            css.push_str(&format!("@media (min-width: {i}px) {{ .c{i} {{ color: red; }} }}\n"));
        }
        let sheet = parse(&css);
        let flat = flatten_media(&sheet, 1_000_000.0);
        assert_eq!(flat.rules.len(), 2000); // every block matches at 1,000,000px
    }
}
