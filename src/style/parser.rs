//! CSS parsing (P2, Wave 1). Full syntax is parsed; unknown declarations are
//! counted and dropped (the IGNORE-UNKNOWN treaty, charter C2). Selectors in
//! scope: element, `.class`, `#id`, descendant, grouping, `a:link`/`:visited`
//! (brief §4).

/// A parsed stylesheet — rules plus the count of declarations parsed-then-
/// ignored (feeds the future Provenance pane / `--stats`). P2 fills this in.
#[derive(Debug, Clone, Default)]
pub struct Stylesheet {
    /// Declarations parsed successfully but outside the curated set.
    pub ignored_declarations: u32,
    // rules: Vec<Rule> — shape defined by P2.
}

/// Parse a stylesheet. One-shot media queries are evaluated against the surface
/// size at load (brief §4); that evaluation is P2's remit.
pub fn parse(_css: &str) -> Stylesheet {
    todo!("P2: CSS tokenizer + parser (full syntax, curated semantics)")
}
