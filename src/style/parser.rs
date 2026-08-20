//! CSS parsing (P2, Wave 1). Full syntax is parsed; unknown declarations are
//! counted and dropped (the IGNORE-UNKNOWN treaty, charter C2). Selectors in
//! scope: element, `.class`, `#id`, descendant, grouping, `a:link`/`:visited`
//! (brief §4), `:root` (packet T1a), plus (packet T1a) the curated
//! exact-match attribute selector `[attr=value]`/`[attr="value"]` — see
//! `parse_attr_selector`'s doc comment for exactly which attribute-selector
//! shapes are and aren't in scope.

use crate::style::media::MediaQuery;
use crate::style::selector::{Compound, ElementInfo, Pseudo, PseudoElement, Selector};
use crate::style::tokenizer::{tokenize, Token};
use crate::style::value::{self, Declarations};

/// One parsed `@media` block: its condition (an unevaluated [`MediaQuery`] —
/// evaluating it against a viewport is `style::media::flatten_media`'s job,
/// not the parser's, since `parse`'s frozen signature carries no viewport)
/// and the ordinary [`StyleRule`]s from its body, parsed exactly like
/// top-level rules (same selector/declaration grammar, same recovery).
///
/// `source_index` is the GLOBAL rule-order counter's value at the point the
/// `@media` keyword was encountered — every [`StyleRule`] in `rules` below
/// was itself assigned an `order` from that SAME shared counter (see
/// `parse`'s doc comment), so a flattened block's rules slot into the
/// correct position relative to top-level rules purely via their own
/// `order` fields; `source_index` is kept on `MediaRule` itself mainly for
/// debuggability/symmetry with the design brief, not because flattening
/// needs to consult it.
#[derive(Debug, Clone)]
pub(crate) struct MediaRule {
    pub query: MediaQuery,
    pub rules: Vec<StyleRule>,
    #[allow(dead_code)]
    pub source_index: u32,
}

/// One rule after comma-grouping has been expanded: a single selector, its
/// source-order index (for cascade tie-breaking), and the declarations it
/// carries. `Stylesheet` owns a flat `Vec` of these — see the module doc for
/// why the shape here is P2's to define.
#[derive(Debug, Clone)]
pub(crate) struct StyleRule {
    pub selector: Selector,
    pub order: u32,
    pub declarations: Declarations,
}

/// A parsed stylesheet — rules plus the count of declarations parsed-then-
/// ignored (feeds the future Provenance pane / `--stats`). P2 fills this in.
#[derive(Debug, Clone, Default)]
pub struct Stylesheet {
    /// Declarations parsed successfully but outside the curated set, or
    /// syntactically broken beyond recovery (brief §10 error recovery).
    pub ignored_declarations: u32,
    /// `@media` blocks encountered while parsing (count only — see
    /// `media_rules` below for the actual parsed condition + nested rules).
    pub media_at_rules: u32,
    /// Any other at-rule (`@import`, `@font-face`, `@keyframes`, …): parsed
    /// syntactically and discarded — none are in the curated dialect (§4).
    pub ignored_at_rules: u32,
    pub(crate) rules: Vec<StyleRule>,
    /// M5: `@media` blocks, parsed but NOT yet evaluated against a
    /// viewport — `parse`'s frozen signature carries no viewport, so a
    /// `Stylesheet` fresh out of `parse` still behaves exactly as it did
    /// pre-M5 if fed straight to `cascade` (these never leak into `rules`).
    /// `style::media::flatten_media` is the separate pre-pass that
    /// evaluates each query against a real viewport width and folds the
    /// matching ones into an ordinary media-free `Stylesheet`.
    pub(crate) media_rules: Vec<MediaRule>,
}

/// Parse a stylesheet. Total: never panics, on any input. Full CSS syntax is
/// tokenized and walked; only the curated declarations (brief §4) survive
/// into `rules` — everything else is counted (charter C2's ignore-unknown
/// treaty) and dropped. Recovery is per brief §10: a bad declaration skips to
/// the next `;`; a bad rule (no `{` reachable) skips to the next `}`.
pub fn parse(css: &str) -> Stylesheet {
    let tokens = tokenize(css);
    let mut sheet = Stylesheet::default();
    let mut pos = 0usize;
    let mut order = 0u32;
    let len = tokens.len();

    while pos < len {
        skip_ws(&tokens, &mut pos);
        if pos >= len {
            break;
        }
        match &tokens[pos] {
            Token::AtKeyword(name) => {
                let is_media = name.eq_ignore_ascii_case("media");
                pos += 1;
                if is_media {
                    parse_media_at_rule(&tokens, &mut pos, &mut sheet, &mut order);
                } else {
                    skip_at_rule_body(&tokens, &mut pos, &mut sheet);
                }
            }
            // A stray close-brace at the top level: nothing to close, drop it.
            Token::RBrace => pos += 1,
            _ => {
                let rules = parse_rule(&tokens, &mut pos, &mut sheet, &mut order);
                sheet.rules.extend(rules);
            }
        }
    }
    sheet
}

/// Parse an inline `style="..."` attribute's value into a [`Declarations`]
/// block (M5) — the same declaration-block grammar/recovery
/// `parse_declaration_block` already gives rule bodies (brief §4's curated
/// property set, charter C2's ignore-unknown treaty, brief §10 recovery: a
/// bad declaration skips to the next `;`), just fed raw property:value
/// pairs with no enclosing `{ }` and no selector. `parse_declaration_block`
/// already tolerates running off the end of its token stream without ever
/// seeing a `}` (`*pos >= len` is checked first thing every loop iteration)
/// so it needs no changes to serve this second caller.
///
/// The `Stylesheet` `parse_declaration_block` writes its
/// `ignored_declarations` counter into is discarded here: inline style has
/// no natural home for that stat yet (no per-element provenance surface
/// exists in this packet) — it is still counted, just not reported anywhere,
/// same as the counter on a `Stylesheet` nobody ever inspects.
///
/// Total: never panics on any input, including empty/garbage strings — the
/// underlying tokenizer and `parse_declaration_block` are already total.
pub(crate) fn parse_inline(css: &str) -> Declarations {
    let tokens = tokenize(css);
    let mut pos = 0usize;
    let mut discarded = Stylesheet::default();
    parse_declaration_block(&tokens, &mut pos, &mut discarded)
}

/// Every rule in `sheet` whose selector matches `target` (given its ancestor
/// chain), unordered. The cascade needs to merge these against matches from
/// *other* sheets (UA vs. every author sheet) before sorting by precedence —
/// sorting per-sheet here would let a later sheet win regardless of
/// specificity, which is wrong (specificity is compared globally within an
/// origin; only ties fall back to source order — see `cascade::visit`).
pub(crate) fn matching_rules<'a>(sheet: &'a Stylesheet, ancestors: &[ElementInfo], target: &ElementInfo) -> Vec<&'a StyleRule> {
    sheet.rules.iter().filter(|r| r.selector.matches(ancestors, target)).collect()
}

fn skip_ws(tokens: &[Token], pos: &mut usize) {
    while *pos < tokens.len() && tokens[*pos] == Token::Whitespace {
        *pos += 1;
    }
}

/// Consume one NON-media at-rule's body after its keyword: either up to and
/// including a top-level `;`, or — if a `{` appears first — a balanced-brace
/// block. Never panics; if neither terminator appears, consumes to EOF.
/// `@media` is handled separately by [`parse_media_at_rule`] (it needs to
/// keep the block contents, not discard them); this always counts against
/// `ignored_at_rules` — also used for a nested at-rule found INSIDE an
/// `@media` body (nested `@media`/other at-rules are out of the curated
/// scope; see `parse_media_body`).
fn skip_at_rule_body(tokens: &[Token], pos: &mut usize, sheet: &mut Stylesheet) {
    let len = tokens.len();
    while *pos < len {
        match &tokens[*pos] {
            Token::Semicolon => {
                *pos += 1;
                sheet.ignored_at_rules += 1;
                return;
            }
            Token::LBrace => {
                *pos += 1;
                let mut depth = 1i32;
                while *pos < len && depth > 0 {
                    match &tokens[*pos] {
                        Token::LBrace => depth += 1,
                        Token::RBrace => depth -= 1,
                        _ => {}
                    }
                    *pos += 1;
                }
                sheet.ignored_at_rules += 1;
                return;
            }
            _ => *pos += 1,
        }
    }
    // Ran off the end without a terminator — still counts; nothing left to skip.
    sheet.ignored_at_rules += 1;
}

/// Parse one `@media` block after its keyword has already been consumed:
/// the condition tokens up to `{`/`;`, and — if a `{` was actually reached —
/// its balanced-brace body, parsed as ordinary rules via
/// [`parse_media_body`] and stored (with the unevaluated [`MediaQuery`]) as
/// a new [`MediaRule`] on `sheet`. `media_at_rules` is bumped either way
/// (malformed input — no reachable `{`, e.g. `@media screen;` or a
/// truncated `@media` at EOF — still counts, matching the pre-M5 counting
/// behavior, but stores no `MediaRule`: brief §10 recovery, never a panic).
fn parse_media_at_rule(tokens: &[Token], pos: &mut usize, sheet: &mut Stylesheet, order: &mut u32) {
    let len = tokens.len();
    let cond_start = *pos;
    while *pos < len && tokens[*pos] != Token::LBrace && tokens[*pos] != Token::Semicolon {
        *pos += 1;
    }
    let condition_tokens = tokens[cond_start..*pos].to_vec();
    let source_index = *order;

    if *pos >= len || tokens[*pos] == Token::Semicolon {
        if *pos < len {
            *pos += 1; // consume the ';'
        }
        sheet.media_at_rules += 1;
        return;
    }

    // tokens[*pos] == LBrace
    *pos += 1;
    let body_start = *pos;
    let mut depth = 1i32;
    while *pos < len && depth > 0 {
        match &tokens[*pos] {
            Token::LBrace => depth += 1,
            Token::RBrace => depth -= 1,
            _ => {}
        }
        *pos += 1;
    }
    // If the block closed cleanly, exclude the closing '}' itself; if it ran
    // off the end unbalanced, the body is everything remaining (tolerated).
    let body_end = if depth == 0 { *pos - 1 } else { *pos };
    let body_tokens = tokens[body_start..body_end].to_vec();

    let query = MediaQuery::parse(&condition_tokens);
    let rules = parse_media_body(&body_tokens, sheet, order);
    sheet.media_at_rules += 1;
    sheet.media_rules.push(MediaRule { query, rules, source_index });
}

/// Parse an `@media` block's body tokens (already extracted, balanced) as a
/// sequence of ordinary rules — same selector/declaration-block grammar and
/// recovery as the top level (`parse`'s own loop), sharing the SAME `order`
/// counter so every rule in the whole stylesheet (top-level or nested) gets
/// a globally comparable source-order value (see `MediaRule`'s doc comment
/// for why that's what lets `flatten_media` skip any position-based
/// reordering). A nested at-rule inside the block (including a nested
/// `@media`) is out of the curated scope — tokenized defensively via
/// `skip_at_rule_body` (never a panic) and counted as an ignored at-rule,
/// never treated as a second `MediaRule`.
fn parse_media_body(tokens: &[Token], sheet: &mut Stylesheet, order: &mut u32) -> Vec<StyleRule> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    let len = tokens.len();
    while pos < len {
        skip_ws(tokens, &mut pos);
        if pos >= len {
            break;
        }
        match &tokens[pos] {
            Token::AtKeyword(_) => {
                pos += 1;
                skip_at_rule_body(tokens, &mut pos, sheet);
            }
            Token::RBrace => pos += 1,
            _ => {
                let rules = parse_rule(tokens, &mut pos, sheet, order);
                out.extend(rules);
            }
        }
    }
    out
}

/// Parse one rule: a selector list (comma-separated), then a `{ ... }`
/// declaration block. If no `{` is reachable before a boundary that can't be
/// part of a selector (`}`, `;`, or EOF), the whole prelude is a bad rule —
/// recover by skipping to the next `}` per brief §10.
///
/// Returns the parsed [`StyleRule`]s (one per selector in a comma-grouped
/// list; empty on a bad rule) rather than pushing them directly onto a
/// `Stylesheet`'s `rules`, so the same function serves both the top-level
/// parse loop (destination: `sheet.rules`) and an `@media` body
/// (destination: that block's own `Vec`, later folded in — or not — by
/// `style::media::flatten_media`) without aliasing `sheet` mutably twice.
fn parse_rule(tokens: &[Token], pos: &mut usize, sheet: &mut Stylesheet, order: &mut u32) -> Vec<StyleRule> {
    let len = tokens.len();
    let selectors = parse_selector_list(tokens, pos);
    skip_ws(tokens, pos);

    if *pos >= len || tokens[*pos] != Token::LBrace {
        while *pos < len && tokens[*pos] != Token::RBrace {
            *pos += 1;
        }
        if *pos < len {
            *pos += 1; // consume the recovering '}'
        }
        return Vec::new();
    }

    *pos += 1; // consume '{'
    let decls = parse_declaration_block(tokens, pos, sheet);

    let this_order = *order;
    *order += 1;
    selectors
        .into_iter()
        .map(|sel| StyleRule { selector: sel, order: this_order, declarations: decls.clone() })
        .collect()
}

fn parse_selector_list(tokens: &[Token], pos: &mut usize) -> Vec<Selector> {
    let mut selectors = vec![parse_selector(tokens, pos)];
    loop {
        skip_ws(tokens, pos);
        if *pos < tokens.len() && tokens[*pos] == Token::Comma {
            *pos += 1;
            skip_ws(tokens, pos);
            selectors.push(parse_selector(tokens, pos));
        } else {
            break;
        }
    }
    selectors
}

/// Parse one selector (up to `{`/`}`/`;`/`,`/EOF). Constructs outside brief
/// §4's scope (child/sibling combinators, attribute selectors, pseudo-
/// elements, unknown pseudo-classes, …) mark the selector `supported: false`
/// but never abort the parse — the surrounding rule's declarations still get
/// counted correctly, the selector just never matches (charter C2 applied to
/// selectors, not just declarations).
#[allow(unused_assignments)] // `flush!()`'s reset-to-false is always immediately followed by a fresh true
fn parse_selector(tokens: &[Token], pos: &mut usize) -> Selector {
    let len = tokens.len();
    let mut compounds: Vec<Compound> = Vec::new();
    let mut supported = true;
    let mut cur = Compound::default();
    let mut cur_has_content = false;
    let mut pending_descendant = false;
    let mut pseudo_element: Option<PseudoElement> = None;

    macro_rules! flush {
        () => {
            if cur_has_content {
                compounds.push(std::mem::take(&mut cur));
                cur_has_content = false;
            }
        };
    }

    while *pos < len {
        match &tokens[*pos] {
            Token::Whitespace => {
                if cur_has_content {
                    pending_descendant = true;
                }
                *pos += 1;
            }
            Token::LBrace | Token::RBrace | Token::Semicolon | Token::Comma => break,
            Token::Ident(name) => {
                // A pseudo-element (`::before`/`::after`) must be the last
                // simple selector — anything else after it (e.g. the `span`
                // in `p::before span`) makes the whole selector unsupported
                // rather than silently attaching the pseudo-element to the
                // wrong compound.
                if pseudo_element.is_some() {
                    supported = false;
                }
                if pending_descendant {
                    flush!();
                    pending_descendant = false;
                }
                cur.element = Some(name.to_ascii_lowercase());
                cur_has_content = true;
                *pos += 1;
            }
            Token::Star => {
                if pseudo_element.is_some() {
                    supported = false;
                }
                if pending_descendant {
                    flush!();
                    pending_descendant = false;
                }
                cur.element = None;
                cur_has_content = true;
                *pos += 1;
            }
            Token::Dot => {
                if pseudo_element.is_some() {
                    supported = false;
                }
                *pos += 1;
                if let Some(Token::Ident(name)) = tokens.get(*pos) {
                    if pending_descendant {
                        flush!();
                        pending_descendant = false;
                    }
                    cur.classes.push(name.to_ascii_lowercase());
                    cur_has_content = true;
                    *pos += 1;
                } else {
                    supported = false;
                }
            }
            Token::Hash(id) => {
                if pseudo_element.is_some() {
                    supported = false;
                }
                if pending_descendant {
                    flush!();
                    pending_descendant = false;
                }
                cur.id = Some(id.to_ascii_lowercase());
                cur_has_content = true;
                *pos += 1;
            }
            Token::Colon => {
                // A pseudo-element must be the last simple selector; a
                // further `:pseudo`/`::pseudo` after one already seen (e.g.
                // `p::before:hover`) is unsupported. (The colon that sets
                // `pseudo_element` itself hits this arm too, but at that
                // point `pseudo_element` is still `None`, so it's unaffected.)
                if pseudo_element.is_some() {
                    supported = false;
                }
                *pos += 1;
                let double = if tokens.get(*pos) == Some(&Token::Colon) {
                    *pos += 1;
                    true
                } else {
                    false
                };
                if let Some(Token::Ident(name)) = tokens.get(*pos) {
                    if pending_descendant {
                        flush!();
                        pending_descendant = false;
                    }
                    match name.to_ascii_lowercase().as_str() {
                        // pseudo-ELEMENTS (both `::before` and legacy
                        // `:before`) — supported, routed to a generated box
                        // via `pseudo_element` (NOT `supported = false`).
                        "before" => pseudo_element = Some(PseudoElement::Before),
                        "after" => pseudo_element = Some(PseudoElement::After),
                        // pseudo-CLASSES — single-colon only.
                        "link" if !double => cur.pseudo.push(Pseudo::Link),
                        "visited" if !double => cur.pseudo.push(Pseudo::Visited),
                        "root" if !double => cur.pseudo.push(Pseudo::Root),
                        _ => supported = false,
                    }
                    cur_has_content = true;
                    *pos += 1;
                    if tokens.get(*pos) == Some(&Token::LParen) {
                        // Functional pseudo-class (`:nth-child(...)`, etc.) —
                        // unsupported; skip its balanced parens.
                        supported = false;
                        *pos += 1;
                        let mut depth = 1i32;
                        while *pos < len && depth > 0 {
                            match &tokens[*pos] {
                                Token::LParen => depth += 1,
                                Token::RParen => depth -= 1,
                                _ => {}
                            }
                            *pos += 1;
                        }
                    }
                } else {
                    supported = false;
                }
            }
            Token::Function(_) => {
                supported = false;
                *pos += 1;
                let mut depth = 1i32;
                while *pos < len && depth > 0 {
                    match &tokens[*pos] {
                        Token::LParen | Token::Function(_) => depth += 1,
                        Token::RParen => depth -= 1,
                        _ => {}
                    }
                    *pos += 1;
                }
            }
            Token::Delim('>') | Token::Delim('+') | Token::Delim('~') => {
                flush!();
                supported = false;
                pending_descendant = false;
                *pos += 1;
            }
            Token::Delim('[') => {
                if pseudo_element.is_some() {
                    supported = false;
                }
                *pos += 1;
                if let Some((attr_name, attr_value, next)) = parse_attr_selector(tokens, *pos) {
                    // The curated exact-match attribute-selector form
                    // (packet T1a) — see `Compound::attrs`'s own doc comment
                    // in `selector.rs` for the full rationale/scope.
                    if pending_descendant {
                        flush!();
                        pending_descendant = false;
                    }
                    cur.attrs.push((attr_name, attr_value));
                    cur_has_content = true;
                    *pos = next;
                } else {
                    // Every other attribute-selector shape stays out of
                    // scope (unchanged pre-T1a behavior): parsed defensively
                    // but marked unsupported so the selector simply never
                    // matches (charter C2).
                    supported = false;
                    while *pos < len && tokens[*pos] != Token::Delim(']') {
                        *pos += 1;
                    }
                    if *pos < len {
                        *pos += 1;
                    }
                    cur_has_content = true;
                }
            }
            _ => {
                supported = false;
                *pos += 1;
            }
        }
    }
    flush!();
    let has_compounds = !compounds.is_empty();
    Selector {
        compounds,
        supported: supported && has_compounds,
        pseudo_element,
    }
}

/// Parse `[name="value"]` / `[name=value]` starting at `tokens[start]` (just
/// past the already-consumed `[`) — the ONLY attribute-selector shape this
/// curated dialect recognizes (packet T1a — just enough for attribute-scoped
/// custom-property overrides like `html[data-theme="dark"]`, this packet's
/// own theming-setup test). Returns the lowercased attribute name (HTML
/// attribute NAMES are case-insensitive, matching `AttrMap::get`'s own
/// case-insensitive lookup), the value verbatim (quoted or bare — HTML
/// attribute VALUES are case-sensitive, no `i`-flag support), and the index
/// just past the matching `]`; `None` for anything else (presence-only
/// `[name]`, `~=`/`^=`/`$=`/`*=` operators, a missing `=`/value/`]`, ...) —
/// the caller then falls back to the pre-existing "parse defensively, mark
/// unsupported" path. Total: only ever reads forward from `start`, tolerates
/// running off the end of `tokens` without panicking.
fn parse_attr_selector(tokens: &[Token], start: usize) -> Option<(String, String, usize)> {
    let len = tokens.len();
    let skip_ws_at = |mut i: usize| -> usize {
        while i < len && tokens[i] == Token::Whitespace {
            i += 1;
        }
        i
    };
    let mut i = skip_ws_at(start);
    let name = match tokens.get(i) {
        Some(Token::Ident(s)) => s.to_ascii_lowercase(),
        _ => return None,
    };
    i = skip_ws_at(i + 1);
    if tokens.get(i) != Some(&Token::Delim('=')) {
        return None;
    }
    i = skip_ws_at(i + 1);
    let value = match tokens.get(i) {
        Some(Token::Str(s)) => s.clone(),
        Some(Token::Ident(s)) => s.clone(),
        _ => return None,
    };
    i = skip_ws_at(i + 1);
    if tokens.get(i) != Some(&Token::Delim(']')) {
        return None;
    }
    Some((name, value, i + 1))
}

/// Parse a `{ ... }` declaration block; `*pos` starts just past the `{` and
/// ends just past the matching `}` (or at EOF, tolerated). Each declaration
/// that fails to parse (bad property/colon/value) or names a property
/// outside the curated set counts against `sheet.ignored_declarations` and
/// recovers by skipping to the next `;`.
fn parse_declaration_block(tokens: &[Token], pos: &mut usize, sheet: &mut Stylesheet) -> Declarations {
    let mut decls = Declarations::default();
    let len = tokens.len();
    loop {
        skip_ws(tokens, pos);
        if *pos >= len {
            return decls;
        }
        match &tokens[*pos] {
            Token::RBrace => {
                *pos += 1;
                return decls;
            }
            Token::Semicolon => {
                *pos += 1;
            }
            Token::Ident(name) => {
                // Custom-property names (`--name`, packet T1a) are
                // CASE-SENSITIVE — unlike every ordinary CSS property name,
                // which this parser has always folded to lowercase (real
                // CSS property names are ASCII-case-insensitive; custom
                // property names deliberately are NOT, per spec). Check the
                // RAW (pre-lowercase) token text for the `--` prefix before
                // deciding whether to lowercase at all.
                let is_custom = name.starts_with("--");
                let name = if is_custom { name.clone() } else { name.to_ascii_lowercase() };
                *pos += 1;
                skip_ws(tokens, pos);
                if *pos < len && tokens[*pos] == Token::Colon {
                    *pos += 1;
                } else {
                    sheet.ignored_declarations += 1;
                    skip_to_decl_boundary(tokens, pos);
                    continue;
                }
                skip_ws(tokens, pos);
                let value_start = *pos;
                // Paren/function depth tracking (Acid2 eyes, Milestone B
                // pt.1): mirrors the selector parser's own precedent
                // earlier in this file (~line 448/474, skipping a
                // functional pseudo-class's balanced parens). Without it, a
                // raw `;` inside an unquoted `url(data:image/png;base64,…)`
                // -- an ordinary `Token::Semicolon`, since the tokenizer has
                // no special `url-token` lexing state -- would truncate the
                // declaration's value before the url's own closing `)` is
                // ever reached. Only exit on `;`/`}` when `depth == 0`; an
                // unterminated `url(` (depth never returns to 0) still
                // terminates the loop via `*pos < len`, so this stays total
                // on hostile/unbalanced input.
                let mut depth = 0i32;
                while *pos < len
                    && !(depth == 0 && (tokens[*pos] == Token::Semicolon || tokens[*pos] == Token::RBrace))
                {
                    match &tokens[*pos] {
                        Token::Function(_) | Token::LParen => depth += 1,
                        Token::RParen => depth = (depth - 1).max(0),
                        _ => {}
                    }
                    *pos += 1;
                }
                let value_tokens: Vec<Token> = tokens[value_start..*pos].iter().filter(|t| **t != Token::Whitespace).cloned().collect();
                if *pos < len && tokens[*pos] == Token::Semicolon {
                    *pos += 1;
                }
                if is_custom {
                    // A custom-property declaration (packet T1a): stored
                    // raw (never eagerly parsed/applied — see
                    // `Declarations::custom`'s own doc comment) and never
                    // counted against `ignored_declarations`, matching how
                    // every RECOGNIZED declaration (custom or not) is
                    // treated — only genuinely unparseable/unknown input
                    // counts against that stat.
                    decls.custom.push((name.into_boxed_str(), value_tokens));
                } else if value_tokens.iter().any(|t| matches!(t, Token::Function(f) if f.eq_ignore_ascii_case("var"))) {
                    // An ordinary property whose value contains `var()`
                    // ANYWHERE (including nested inside another function,
                    // e.g. `rgb(var(--r), 0, 0)` — the token stream is
                    // flat, so this `any` check finds it regardless of
                    // nesting) can't be resolved yet: defer it to cascade
                    // time (packet T1a) instead of calling `apply_property`
                    // now. Not counted against `ignored_declarations` either
                    // — whether it ultimately applies is decided later by
                    // `cascade::resolve`, exactly like any other
                    // successfully-parsed declaration.
                    decls.deferred.push((name.into_boxed_str(), value_tokens));
                } else if !value::apply_property(&name, &value_tokens, &mut decls) {
                    sheet.ignored_declarations += 1;
                }
            }
            _ => {
                sheet.ignored_declarations += 1;
                skip_to_decl_boundary(tokens, pos);
            }
        }
    }
}

/// Resync to the next declaration boundary after an unrecognized/malformed
/// leading token (no colon-shaped `name: value` at all). Gets the IDENTICAL
/// paren/function-depth treatment as `parse_declaration_block`'s own
/// value-boundary scan right above (don't let the two drift) — an
/// unrecognized declaration immediately followed by one whose value is a
/// semicolon-bearing `url(data:...)` must not resync into the middle of
/// that url's own content.
fn skip_to_decl_boundary(tokens: &[Token], pos: &mut usize) {
    let len = tokens.len();
    let mut depth = 0i32;
    while *pos < len
        && !(depth == 0 && (tokens[*pos] == Token::Semicolon || tokens[*pos] == Token::RBrace))
    {
        match &tokens[*pos] {
            Token::Function(_) | Token::LParen => depth += 1,
            Token::RParen => depth = (depth - 1).max(0),
            _ => {}
        }
        *pos += 1;
    }
    if *pos < len && tokens[*pos] == Token::Semicolon {
        *pos += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::media::ColorScheme;
    use crate::style::computed::*;
    use crate::surface::Color;

    fn find(dom: &crate::dom::Dom, tag: &str) -> Option<crate::dom::NodeId> {
        find_all(dom, tag).into_iter().next()
    }

    fn find_all(dom: &crate::dom::Dom, tag: &str) -> Vec<crate::dom::NodeId> {
        let mut out = Vec::new();
        fn walk(dom: &crate::dom::Dom, id: crate::dom::NodeId, tag: &str, out: &mut Vec<crate::dom::NodeId>) {
            if let Some(el) = dom.node(id).element() {
                if el.name.as_str() == tag {
                    out.push(id);
                }
                for &c in &el.children {
                    walk(dom, c, tag, out);
                }
            }
        }
        walk(dom, dom.root(), tag, &mut out);
        out
    }

    #[test]
    fn parsing_a_clean_rule_ignores_nothing() {
        let sheet = parse("p { color: red; }");
        assert_eq!(sheet.ignored_declarations, 0);
        assert_eq!(sheet.rules.len(), 1);
    }

    #[test]
    fn element_selector_matches() {
        let dom = crate::dom::parser::parse("<p>hi</p>");
        let sheet = parse("p { color: red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn class_selector_matches() {
        let dom = crate::dom::parser::parse(r#"<p class="a">hi</p>"#);
        let sheet = parse(".a { color: red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn id_selector_matches() {
        let dom = crate::dom::parser::parse(r#"<p id="x">hi</p>"#);
        let sheet = parse("#x { color: red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn descendant_selector_matches_only_nested() {
        let dom = crate::dom::parser::parse(r#"<div><p>in</p></div><p>out</p>"#);
        let sheet = parse("div p { color: red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let ps = find_all(&dom, "p");
        assert_eq!(ps.len(), 2);
        assert_eq!(styles[ps[0]].color, Color::rgb(255, 0, 0));
        assert_eq!(styles[ps[1]].color, Color::BLACK);
    }

    #[test]
    fn grouped_selectors_both_match() {
        let dom = crate::dom::parser::parse("<p>a</p><span>b</span>");
        let sheet = parse("p, span { color: red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        let span = find(&dom, "span").unwrap();
        assert_eq!(styles[p].color, Color::rgb(255, 0, 0));
        assert_eq!(styles[span].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn pseudo_link_matches_anchor_with_href_only() {
        let dom = crate::dom::parser::parse(r#"<a href="x">l</a><a>nohref</a>"#);
        let sheet = parse("a:link { color: red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let anchors = find_all(&dom, "a");
        assert_eq!(styles[anchors[0]].color, Color::rgb(255, 0, 0));
        assert_ne!(styles[anchors[1]].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn pseudo_visited_never_matches_without_history() {
        let dom = crate::dom::parser::parse(r#"<a href="x">l</a>"#);
        let sheet = parse("a:visited { color: red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let a = find(&dom, "a").unwrap();
        assert_ne!(styles[a].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn specificity_id_beats_class() {
        let dom = crate::dom::parser::parse(r#"<p id="x" class="a">t</p>"#);
        let sheet = parse("#x { color: red; } .a { color: blue; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn specificity_class_beats_element() {
        let dom = crate::dom::parser::parse(r#"<p class="a">t</p>"#);
        let sheet = parse("p { color: blue; } .a { color: red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn later_source_order_wins_specificity_ties() {
        let dom = crate::dom::parser::parse("<p>t</p>");
        let sheet = parse("p { color: red; } p { color: blue; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].color, Color::rgb(0, 0, 255));
    }

    #[test]
    fn margin_shorthand_one_value_applies_to_all_edges() {
        let dom = crate::dom::parser::parse("<p>t</p>");
        let sheet = parse("p { margin: 5px; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].margin.top, LengthPercentageAuto::Px(5.0));
        assert_eq!(styles[p].margin.right, LengthPercentageAuto::Px(5.0));
        assert_eq!(styles[p].margin.bottom, LengthPercentageAuto::Px(5.0));
        assert_eq!(styles[p].margin.left, LengthPercentageAuto::Px(5.0));
    }

    #[test]
    fn margin_shorthand_four_values_map_top_right_bottom_left() {
        let dom = crate::dom::parser::parse("<p>t</p>");
        let sheet = parse("p { margin: 1px 2px 3px 4px; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].margin.top, LengthPercentageAuto::Px(1.0));
        assert_eq!(styles[p].margin.right, LengthPercentageAuto::Px(2.0));
        assert_eq!(styles[p].margin.bottom, LengthPercentageAuto::Px(3.0));
        assert_eq!(styles[p].margin.left, LengthPercentageAuto::Px(4.0));
    }

    #[test]
    fn padding_shorthand_two_values_map_vertical_horizontal() {
        let dom = crate::dom::parser::parse("<p>t</p>");
        let sheet = parse("p { padding: 2px 6px; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].padding.top, LengthPercentage::Px(2.0));
        assert_eq!(styles[p].padding.bottom, LengthPercentage::Px(2.0));
        assert_eq!(styles[p].padding.right, LengthPercentage::Px(6.0));
        assert_eq!(styles[p].padding.left, LengthPercentage::Px(6.0));
    }

    #[test]
    fn border_shorthand_solid_only() {
        let dom = crate::dom::parser::parse("<p>t</p>");
        let sheet = parse("p { border: 2px solid red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].border.top.style, BorderStyle::Solid);
        assert_eq!(styles[p].border.top.width, 2.0);
        assert_eq!(styles[p].border.top.color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn border_shorthand_non_solid_style_renders_as_none() {
        let dom = crate::dom::parser::parse("<p>t</p>");
        let sheet = parse("p { border: 2px dashed red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].border.top.style, BorderStyle::None);
    }

    #[test]
    fn color_named_hex_and_rgb_forms() {
        let dom = crate::dom::parser::parse("<p>t</p>");
        let cases = [
            ("color: red;", Color::rgb(255, 0, 0)),
            ("color: #0f0;", Color::rgb(0, 255, 0)),
            ("color: #0000ff;", Color::rgb(0, 0, 255)),
            ("color: rgb(10, 20, 30);", Color::rgb(10, 20, 30)),
        ];
        for (css, expect) in cases {
            let sheet = parse(&format!("p {{ {css} }}"));
            let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
            let p = find(&dom, "p").unwrap();
            assert_eq!(styles[p].color, expect, "for {css}");
        }
    }

    #[test]
    fn ignore_unknown_property_increments_counter() {
        let sheet = parse("p { flibbertigibbet: 1; color: red; }");
        assert_eq!(sheet.ignored_declarations, 1);
    }

    #[test]
    fn bad_shorthand_token_counts_as_ignored_and_does_not_apply_partially() {
        // `div` (unlike `p`) has no UA-sheet margin default, so a rejected
        // author declaration should leave it at the CSS initial `0`.
        let dom = crate::dom::parser::parse("<div>t</div>");
        let sheet = parse("div { margin: 1px bogus 2px 3px; }");
        assert_eq!(sheet.ignored_declarations, 1);
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let div = find(&dom, "div").unwrap();
        assert_eq!(styles[div].margin.top, LengthPercentageAuto::Px(0.0)); // CSS initial, not `1px`

        let sheet = parse("p { border: 5% solid red; }");
        assert_eq!(sheet.ignored_declarations, 1);
    }

    #[test]
    fn ignore_unknown_at_rule() {
        let sheet = parse("@font-face { font-family: X; src: url(x.woff); } p { color: red; }");
        assert_eq!(sheet.ignored_at_rules, 1);
        assert_eq!(sheet.rules.len(), 1); // the trailing rule still parses
    }

    #[test]
    fn media_query_is_parsed_and_never_leaks_into_rules_without_flattening() {
        let sheet = parse("@media (min-width: 800px) { p { color: red; } } p { color: blue; }");
        assert_eq!(sheet.media_at_rules, 1);
        // The rule inside @media must not leak into the flat rule list —
        // `cascade` reads `rules` directly and never sees `media_rules`
        // (that's `style::media::flatten_media`'s job, a separate pre-pass).
        assert_eq!(sheet.rules.len(), 1);

        let dom = crate::dom::parser::parse("<p>t</p>");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].color, Color::rgb(0, 0, 255));
    }

    #[test]
    fn media_query_is_now_stored_not_just_counted() {
        // M5: the @media block's condition + nested rule must actually be
        // STORED (not discarded like pre-M5), even though `rules` stays
        // media-free (see the test above).
        let sheet = parse("@media (min-width: 800px) { p { color: red; } } p { color: blue; }");
        assert_eq!(sheet.media_rules.len(), 1);
        let media_rule = &sheet.media_rules[0];
        assert_eq!(media_rule.rules.len(), 1);
        assert_eq!(media_rule.rules[0].declarations.color, Some(Color::rgb(255, 0, 0)));
        assert!(media_rule.query.matches(1024.0, ColorScheme::Light));
        assert!(!media_rule.query.matches(640.0, ColorScheme::Light));
    }

    #[test]
    fn grouped_selectors_inside_media_each_get_a_stored_rule() {
        let sheet = parse("@media screen { p, span { color: red; } }");
        assert_eq!(sheet.media_rules.len(), 1);
        assert_eq!(sheet.media_rules[0].rules.len(), 2);
    }

    #[test]
    fn malformed_media_at_eof_does_not_panic_and_stores_nothing() {
        for css in ["@media", "@media screen", "@media (min-width: 800px)", "@media;", "@media screen;"] {
            let sheet = parse(css);
            assert_eq!(sheet.media_at_rules, 1, "for {css:?}");
            assert!(sheet.media_rules.is_empty(), "for {css:?}");
        }
    }

    #[test]
    fn pathological_5000_media_rules_does_not_panic() {
        let mut css = String::new();
        for i in 0..5000 {
            css.push_str(&format!("@media (min-width: {i}px) {{ .c{i} {{ color: red; }} }}\n"));
        }
        let sheet = parse(&css);
        assert_eq!(sheet.media_at_rules, 5000);
        assert_eq!(sheet.media_rules.len(), 5000);
    }

    #[test]
    fn deeply_nested_garbage_braces_inside_media_does_not_panic() {
        let mut css = String::from("@media (min-width: 1px) { ");
        for _ in 0..2000 {
            css.push('{');
        }
        for _ in 0..2000 {
            css.push('}');
        }
        css.push_str(" p { color: red; } }");
        let sheet = parse(&css);
        assert_eq!(sheet.media_at_rules, 1);
    }

    #[test]
    fn malformed_declaration_is_skipped_and_rest_still_parses() {
        let dom = crate::dom::parser::parse("<p>t</p>");
        let sheet = parse("p { color : ; color: red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn malformed_rule_is_skipped_to_next_brace_and_next_rule_still_parses() {
        let dom = crate::dom::parser::parse("<p>t</p>");
        let sheet = parse("!!!broken!!! } p { color: red; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn unsupported_selector_kinds_parse_without_choking_and_do_not_match() {
        let dom = crate::dom::parser::parse("<p>t</p>");
        for css in ["p > span { color: red; }", "a[href='x'] { color: red; }", "p::before { color: red; }", "p:hover { color: red; }"] {
            let sheet = parse(css);
            let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
            let p = find(&dom, "p").unwrap();
            assert_ne!(styles[p].color, Color::rgb(255, 0, 0), "for {css}");
        }
    }

    // ---- packet P3 (generated content): `::before`/`::after` -------------

    #[test]
    fn parses_pseudo_elements_before_after_both_colon_forms() {
        use crate::style::selector::PseudoElement;
        let cases = [
            ("p::before { color: red }", Some(PseudoElement::Before)),
            ("p::after  { color: red }", Some(PseudoElement::After)),
            ("p:before  { color: red }", Some(PseudoElement::Before)), // legacy single-colon
            ("p:after   { color: red }", Some(PseudoElement::After)),
            ("p         { color: red }", None),
        ];
        for (css, want) in cases {
            let sheet = parse(css);
            let sel = &sheet.rules[0].selector;
            assert_eq!(sel.pseudo_element, want, "for {css}");
            assert!(sel.supported, "pseudo-element selector must stay supported: {css}");
        }
        // unknown pseudo-element still dropped
        let sheet = parse("p::boguspseudo { color: red }");
        assert!(!sheet.rules[0].selector.supported);
    }

    #[test]
    fn pseudo_element_must_be_last_simple_selector() {
        // `p::before span` is invalid — a pseudo-element must be the subject's last bit.
        let sheet = parse("p::before span { color: red }");
        assert!(!sheet.rules[0].selector.supported, "pseudo-element followed by more selector => dropped");
        // sanity: the valid form still parses supported
        let ok = parse("p::before { color: red }");
        assert!(ok.rules[0].selector.supported);
    }

    // ---- packet T1a: attribute selectors ([attr=value]) -----------------
    // (packet t1b-color-scheme reconciliation note: `html[data-theme="dark"]`
    // -- the no-JS theme hook `main.rs` stamps pre-cascade -- needs to
    // actually parse AND match, unlike the pre-T1a "always unsupported"
    // treatment `a[href='x']` above still (correctly) exercises for an
    // ELEMENT mismatch, not an attribute one. T1a's curated grammar covers
    // exact-match `[attr=value]` only -- no presence-only `[attr]` form --
    // so `[attr]` and every other operator fall through to the same
    // fail-closed "unsupported" path exercised below.)

    #[test]
    fn attribute_equals_selector_matches_the_stamped_root_html_element() {
        let dom = crate::dom::parser::parse(r#"<html data-theme="dark"><body><p>t</p></body></html>"#);
        let sheet = parse(r#"html[data-theme="dark"] p { color: red; }"#);
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn attribute_equals_selector_does_not_match_a_different_value() {
        let dom = crate::dom::parser::parse(r#"<html data-theme="light"><body><p>t</p></body></html>"#);
        let sheet = parse(r#"html[data-theme="dark"] p { color: red; }"#);
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_ne!(styles[p].color, Color::rgb(255, 0, 0));
    }

    #[test]
    fn attribute_selector_with_an_unsupported_operator_fails_closed() {
        // `~=` (and `^=`/`$=`/`*=`/`|=`, plus the bare presence-only
        // `[attr]` form) are outside the curated subset -- must never
        // match, same C2 fail-closed treatment as the rest of this
        // module's unsupported selector constructs.
        let dom = crate::dom::parser::parse(r#"<p class="x">t</p>"#);
        let sheet = parse(r#"p[class~="x"] { color: red; }"#);
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_ne!(styles[p].color, Color::rgb(255, 0, 0), "~= is out of the curated subset and must fail closed");
    }

    #[test]
    fn unterminated_attribute_selector_does_not_panic() {
        // Totality sweep target: a malformed/unterminated `[` must recover
        // (skip to EOF), never panic or hang.
        for css in ["p[ { color: red; }", "p[data-x", "p[data-x=", "p[data-x=\"unterminated"] {
            let _ = parse(css);
        }
    }

    #[test]
    fn does_not_panic_on_malformed_css_sweep() {
        let inputs = [
            "",
            "{",
            "}",
            "p {",
            "p color: red; }",
            "p { color",
            "@",
            "@media",
            "/* unterminated",
            "\"unterminated string",
            "p { color: red",
            ".. {}",
            "####{}",
            "p{color:red;;;;}",
            "p{}{}{}",
            ":::: {}",
            "a[href='x'] { color: red; }",
            "p > span { color: red; }",
            "\0\0\0",
            "*{color:red}",
            "@import url(x.css);",
            "@charset \"utf-8\";",
            "@media",
            "@media {",
            "@media (",
            "@media (min-width:",
            "@media screen and (max-width: 500px",
            "@media , , , { p { color: red; } }",
            "@media not screen and (max-width: 500px) { p { color: red; } }",
        ];
        for i in inputs {
            let _ = parse(i);
        }
    }

    // ---- Acid2 eyes (Milestone B pt.1): depth-aware declaration-value scan
    // A raw `;` inside an unquoted `url(data:image/png;base64,...)` must not
    // truncate the declaration -- both `parse_declaration_block`'s
    // named-declaration scan AND `skip_to_decl_boundary`'s
    // malformed-declaration resync need the identical paren/function-depth
    // tracking (see the design doc's Finding 2).

    #[test]
    fn url_with_internal_semicolon_does_not_truncate_the_declaration() {
        let dom = crate::dom::parser::parse("<div>t</div>");
        let sheet = parse("div { background: red url(data:image/png;base64,AAAA); color: green; }");
        assert_eq!(sheet.ignored_declarations, 0);
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let div = find(&dom, "div").unwrap();
        assert_eq!(styles[div].background_color, Color::rgb(255, 0, 0));
        assert_eq!(styles[div].background_image.as_deref(), Some("data:image/png;base64,AAAA"));
    }

    #[test]
    fn declaration_after_a_semicolon_bearing_url_still_applies() {
        // Same fixture shape as above, but the assertion is specifically on
        // the declaration AFTER the broken one: proves the parser resynced
        // to the url()'s own closing `)` and didn't lose or corrupt
        // whatever comes next (the "narrowly scoped" claim from the design
        // doc's Finding 2, verified as an assertion, not just a comment).
        let dom = crate::dom::parser::parse("<div>t</div>");
        let sheet = parse("div { background: red url(data:image/png;base64,AAAA); color: green; }");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let div = find(&dom, "div").unwrap();
        assert_eq!(styles[div].color, Color::rgb(0, 128, 0));
    }

    #[test]
    fn skip_to_decl_boundary_also_tracks_depth_past_an_unrecognized_leading_token() {
        // A bare number where a property name is expected is unrecognized
        // (falls to `parse_declaration_block`'s `_ =>` arm, which calls
        // `skip_to_decl_boundary` -- a SEPARATE scan site from the
        // named-declaration one exercised above). The declaration right
        // after it still contains a semicolon-bearing `url(...)` and must
        // still parse -- this exercises `skip_to_decl_boundary`'s own copy
        // of the depth-tracking fix, not the main loop's.
        let dom = crate::dom::parser::parse("<div>t</div>");
        let sheet = parse("div { 123; background: url(data:image/png;base64,AAAA); }");
        assert_eq!(sheet.ignored_declarations, 1, "the leading `123;` still counts as ignored");
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let div = find(&dom, "div").unwrap();
        assert_eq!(styles[div].background_image.as_deref(), Some("data:image/png;base64,AAAA"));
    }

    #[test]
    fn unterminated_url_function_does_not_hang_or_panic() {
        // Depth never returns to 0 (no closing paren before EOF/`}`) -- the
        // scan must still terminate via `*pos < len`/`RBrace`, matching
        // `tokenizer.rs`'s own "never panics on unterminated constructs"
        // posture at the parser layer.
        for css in [
            "div { background: url(data:image/png;base64,AAAA",
            "div { background: url(data:image/png;base64,AAAA }",
            "div { background: url(unterminated",
        ] {
            let _ = parse(css);
        }
    }

    #[test]
    fn plain_declaration_with_no_parens_still_terminates_at_its_own_semicolon() {
        // Regression/sanity: depth-tracking must not disturb the common
        // case -- depth stays 0 throughout, and the declaration still ends
        // exactly at its own `;`.
        let dom = crate::dom::parser::parse("<p>t</p>");
        let sheet = parse("p { color: red; }");
        assert_eq!(sheet.ignored_declarations, 0);
        let styles = crate::style::cascade::cascade(&dom, std::slice::from_ref(&sheet));
        let p = find(&dom, "p").unwrap();
        assert_eq!(styles[p].color, Color::rgb(255, 0, 0));
    }
}
