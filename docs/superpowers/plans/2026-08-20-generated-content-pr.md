# Generated content (::before/::after + content) — Acid2 Packet 3 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Support `::before`/`::after` + the `content` property, synthesizing pseudo-element boxes in the
style→box-tree pipeline so they lay out and paint like real boxes.

**Architecture:** Selector gains a `pseudo_element` field (parsed, no longer dropped); `content` becomes a
computed property (`Content { Normal, None, Str, Url }`); a new `cascade_pseudo` walk produces per-element
`before`/`after` `ComputedStyle`s (inheriting from the element, kept only when `content` generates a box);
`build_box_tree` threads those and synthesizes a generated `Container(+Text)` child prepended/appended per
element. Generated boxes reuse the existing `base_style`/`emit` path, so P1 positioning + P2 z-index apply
for free.

**Tech Stack:** Rust; existing style pipeline (`selector.rs`/`parser.rs`/`value.rs`/`computed.rs`/`cascade.rs`)
+ box-tree (`box_tree.rs`). No new dependency.

**Spec:** `docs/superpowers/specs/2026-08-20-generated-content-design.md`

## Global Constraints

- **1.44 MB floppy = 1,474,560 bytes.** No new dependency. Report the i486 delta from the CI `stele-i486`.
- **Golden impact = exactly one re-bless (`httpforever`)**: its `.hero::before/::after { content:"" }` motif
  boxes now render. No other fixture uses a pseudo-element selector. A `::before`/`::after`-free document is
  byte-identical, and every `build_box_tree` caller that passes `&[]` for pseudo styles generates nothing.
- **Parsing is TOTAL:** malformed `content` → `normal` (⇒ no box); unknown `::x` → rule dropped; never panic.
- **`content:""` (empty string) GENERATES A BOX** (empty box, full box model) — distinct from `content:none`.
- **No local i486 builds** — CI compiles/tests; PNG goldens blessed from the CI artifact, pixel-verified
  (controller, AGENTS.md §4). No JavaScript / no uninvited computation (C3): generated content is inert/static.

---

### Task 1: Selector pseudo-elements (`::before`/`::after` parse, no longer dropped)

**Files:**
- Modify: `src/style/selector.rs` (add `PseudoElement` + `Selector.pseudo_element`)
- Modify: `src/style/parser.rs` (parse `::before`/`::after`/`:before`/`:after` → set it, keep supported)
- Test: unit tests in `src/style/parser.rs`

**Interfaces:**
- Produces: `stele::style::selector::PseudoElement { Before, After }` (derive `Debug,Clone,Copy,PartialEq,Eq`);
  `Selector.pseudo_element: Option<PseudoElement>`. Task 3 consumes it.

- [ ] **Step 1: Write failing tests** (in `parser.rs` tests, mirroring the existing selector-parse tests near
      line 951 which asserts `p::before` currently parses/drops). Assert `p::before { color: red }` now yields
      a supported rule whose `selector.pseudo_element == Some(PseudoElement::Before)`; `p::after` → `After`;
      legacy `p:before` → `Before`; `p:after` → `After`; `p::unknownpseudo` → still dropped/unsupported; a
      plain `p` → `pseudo_element == None`.
- [ ] **Step 2: Verify fail** — CI: `cargo test --lib style::parser` → FAIL (`PseudoElement`/`pseudo_element`
      undefined).
- [ ] **Step 3: Implement**
  - `selector.rs`: `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum PseudoElement { Before, After }`;
    add `pub pseudo_element: Option<PseudoElement>` to `Selector` (default `None` — update its constructor/
    `Default`). In `specificity()`, count a `Some(_)` pseudo-element as one element (add 1 to the element
    count; grep the existing `specificity` impl and mirror how element count is tallied).
  - `parser.rs` (~line 397, the `Token::Colon` arm): detect `::before`/`::after` (double colon) and legacy
    single-colon `:before`/`:after`. On match, set `selector.pseudo_element = Some(...)` and DO NOT set
    `supported = false`; require it to be the last simple selector (a following simple selector ⇒ drop).
    Keep every other `::x` and unknown pseudo unsupported exactly as today. Handle the single-colon
    `:before`/`:after` by name in the pseudo-*class* match block (add `"before"`/`"after"` arms that set the
    pseudo_element instead of a `Pseudo`), and the double-colon path likewise (currently it just flags
    unsupported — branch on the following ident name).
- [ ] **Step 4: Verify pass** — CI: `cargo test --lib style::parser` green; existing parser tests unchanged.
- [ ] **Step 5: Commit** — `feat(css): parse ::before/::after pseudo-elements on selectors (Acid2 P3)`

---

### Task 2: The `content` property

**Files:**
- Modify: `src/style/computed.rs` (`Content` enum + `ComputedStyle.content` + `generates_box`)
- Modify: `src/style/value.rs` (`Declarations.content` + `"content"` parse arm + `ov!`)
- Modify: `src/style/cascade.rs` (`content: own!(content)`)
- Test: unit tests in `src/style/value.rs`

**Interfaces:**
- Produces: `Content { Normal, None, Str(String), Url(String) }` (derive `Debug,Clone,PartialEq,Eq`);
  `impl Content { pub fn generates_box(&self) -> bool }`; `ComputedStyle.content: Content` (default `Normal`,
  non-inherited). Task 3/4 consume it.

- [ ] **Step 1: Write failing tests** (in `value.rs` tests, mirroring the `z-index` parse tests): `content`
      parses `normal`→Normal, `none`→None, `"hi"`→Str("hi"), `""`→Str(""), `url(x.png)`→Url("x.png"),
      garbage→unset; `Content::Str("".into()).generates_box() == true`, `Content::Normal.generates_box() ==
      false`, `Content::None.generates_box() == false`, `Content::Url("x".into()).generates_box() == true`.
- [ ] **Step 2: Verify fail** — CI: FAIL (`Content`/`content` undefined).
- [ ] **Step 3: Implement**
  - `computed.rs`:
    ```rust
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Content { Normal, None, Str(String), Url(String) }
    impl Content {
        /// `content:""` (empty string) STILL generates a box — only `normal`/`none` suppress it.
        pub fn generates_box(&self) -> bool { matches!(self, Content::Str(_) | Content::Url(_)) }
    }
    ```
    add `pub content: Content` to `ComputedStyle` and `content: Content::Normal` to its `Default`.
    (`Content` is not `Copy` — it owns a `String`; that's fine, it's cloned like other owned computed fields
    e.g. `background_color`/`FontFamily` — grep how a non-`Copy` computed field is handled in `resolve`.)
  - `value.rs`: `pub content: Option<Content>` on `Declarations`; `ov!(content)`; import `Content`. The
    `"content"` arm — the tokenizer yields `Token::Ident`, `Token::Str`/`Token::String` (grep the tokenizer
    for the string-literal token name), and `Token::Function`/`url()` (grep how `background-image: url(...)`
    is parsed — reuse that url extraction). Shape:
    ```rust
    "content" => match tokens.first() {
        Some(Token::Ident(s)) if s.eq_ignore_ascii_case("normal") => { d.content = Some(Content::Normal); true }
        Some(Token::Ident(s)) if s.eq_ignore_ascii_case("none")   => { d.content = Some(Content::None);   true }
        Some(/* string-literal token */ s) => { d.content = Some(Content::Str(s.clone())); true }
        Some(/* url(...) token, via the existing bg-image url helper */) => { d.content = Some(Content::Url(u)); true }
        _ => false,
    }
    ```
    (Match the EXACT token spellings the tokenizer uses for a quoted string and for `url(...)`; if
    `background-image` already has a `token → url String` helper, call it. If the empty string `""`
    tokenizes to a `Str("")`, `Content::Str("")` falls out naturally.)
  - `cascade.rs`: `content: own!(content)` (non-inherited, next to `z_index`/`position`).
- [ ] **Step 4: Verify pass** — CI: `cargo test --lib style` green.
- [ ] **Step 5: Commit** — `feat(css): parse+cascade the content property (normal|none|string|url) (Acid2 P3)`

---

### Task 3: `cascade_pseudo` — per-element `::before`/`::after` computed styles

**Files:**
- Modify: `src/style/cascade.rs` (`PseudoStyles`, `cascade_pseudo`, exclude pseudo rules from element fold)
- Test: unit tests in `src/style/cascade.rs`

**Interfaces:**
- Consumes: `Selector.pseudo_element` (T1), `ComputedStyle.content`/`generates_box` (T2).
- Produces: `pub struct PseudoStyles { pub before: Option<ComputedStyle>, pub after: Option<ComputedStyle> }`
  (derive `Debug, Clone, Default`); `pub fn cascade_pseudo(dom: &Dom, author_sheets: &[Stylesheet]) ->
  Vec<PseudoStyles>` (indexed by `NodeId`, length `dom.len()`). Task 4 consumes it.

- [ ] **Step 1: Write failing tests** (in `cascade.rs` tests, mirroring the existing `cascade` tests that use
      `cascade(&d, &sheets)` + `find(&d, "tag")`): for a DOM `<p>` with author `p::before { content:"x";
      color: red }`, `cascade_pseudo(&d, &sheets)[find(p)].before` is `Some` with `content == Str("x")` and
      `color == red`, inheriting the element's font-size; `p::after` with `content:none` → `after == None`;
      an element with no pseudo rule → both `None`; AND assert the element's OWN `cascade(...)` style does NOT
      pick up the `p::before` `color` (exclusion).
- [ ] **Step 2: Verify fail** — CI: FAIL (`cascade_pseudo`/`PseudoStyles` undefined).
- [ ] **Step 3: Implement**
  - Add `pub struct PseudoStyles { pub before: Option<ComputedStyle>, pub after: Option<ComputedStyle> }`
    with `#[derive(Debug, Clone, Default)]`.
  - In `fold_matching_declarations` (element's own fold), skip candidate rules whose
    `selector.pseudo_element.is_some()` — for BOTH the UA and author loops (filter the `parser::matching_rules`
    output). This keeps `p::before` declarations off the `p` element.
  - `cascade_pseudo`: walk the DOM exactly like `cascade` (reuse the same ancestor-stack/`ElementInfo` walk —
    factor a shared helper if the walk is duplicated, else mirror it). For each ELEMENT node, for each
    `which in [Before, After]`: collect matching author rules with `selector.pseudo_element == Some(which)`,
    fold their `Declarations` (same overlay/precedence as `fold_matching_declarations` but pseudo-filtered),
    `resolve` a `ComputedStyle` with **the element's own computed style as the `parent`** (generated content
    inherits from the originating element), and store `Some(style)` iff `style.content.generates_box()`, else
    `None`. Non-element nodes (text) get `PseudoStyles::default()`.
    (The element's own computed style is available — either compute the main `cascade` first and pass it in,
    or compute both in one walk. Simplest: `cascade_pseudo` internally calls/reuses the per-element resolved
    style as the inheritance parent. Keep `cascade`'s public signature unchanged.)
- [ ] **Step 4: Verify pass** — CI: `cargo test --lib style::cascade` green; existing cascade tests unchanged
      (excluding pseudo rules changes nothing today — no pseudo rules survive parse until T1, and no existing
      fixture/test declares one except httpforever, which is a golden re-bless).
- [ ] **Step 5: Commit** — `feat(css): cascade_pseudo — ::before/::after computed styles per element (Acid2 P3)`

---

### Task 4: Box-tree synthesis + pipeline wiring

**Files:**
- Modify: `src/layout/box_tree.rs` (`build_box_tree`/`build_node` gain `pseudo: &[PseudoStyles]`; synthesize)
- Modify: `src/browser.rs`, `src/main.rs`, `src/frames.rs` (call `cascade_pseudo`, pass it through)
- Test: `src/layout/box_tree.rs` unit tests

**Interfaces:**
- Consumes: `cascade_pseudo` / `PseudoStyles` (T3), `Content` (T2).

- [ ] **Step 1: Write failing test** (in `box_tree.rs` tests, mirroring the existing `build_box_tree` test at
      ~line 1484): build a DOM `<p>x</p>` + a `PseudoStyles` for the `p` with `before = Some(style{content:
      Str("B"), ..})`; assert the built `p` `LayoutNode`'s FIRST child is a generated `Container` whose own
      first child is `Text("B")`; with `after = Some(content: Str("A"))`, the LAST child is `Text("A")`'s
      container; with both `None`, `p` has only its real child.
- [ ] **Step 2: Verify fail** — CI: FAIL (`build_box_tree` arity / synthesis missing).
- [ ] **Step 3: Implement**
  - Add `pseudo: &[PseudoStyles]` param to `build_box_tree` and thread it into `build_node` (same slice, same
    `id` indexing as `styles`). Add a helper:
    ```rust
    /// Synthesize a generated-content box for a `::before`/`::after` whose pseudo
    /// style generates a box (`content.generates_box()`), else `None`. A `Str`
    /// becomes a `Container` (the pseudo box, carrying its own bg/border/position/
    /// size) wrapping a `Text` child; empty string ⇒ empty text child (box still
    /// present). `Url` is parsed but not rendered (Acid2 P3 scope note) ⇒ `None`.
    fn generated_node(ps: &ComputedStyle) -> Option<LayoutNode> {
        match &ps.content {
            Content::Str(s) => Some(LayoutNode {
                style: ps.clone(),
                content: BoxContent::Container,
                children: vec![LayoutNode { style: ps.clone(), content: BoxContent::Text(s.clone()),
                                            children: Vec::new(), interactive: None }],
                interactive: None,
            }),
            Content::Url(_) | Content::Normal | Content::None => None,
        }
    }
    ```
  - In `build_node`'s ELEMENT arm, after assembling `children`, prepend `generated_node(before)` (if
    `pseudo.get(id).and_then(|p| p.before.as_ref())` is `Some` and yields a node) and append
    `generated_node(after)`. Only elements get generated content (text nodes never).
  - Wire the ~10 `build_box_tree(...)` call sites: the REAL pipeline (`browser.rs:1496`, `main.rs` ×several
    that render, `frames.rs:415`) computes `let pseudo = cascade::cascade_pseudo(&dom, &sheets);` alongside
    its existing `cascade(...)` and passes `&pseudo`. TEST/`--dump-text`-only callers that don't need
    generated content pass `&[]` (empty slice ⇒ `pseudo.get(id)` is `None` ⇒ nothing generated). Grep each
    call site; match the `sheets`/`author_sheets` variable already in scope there.
- [ ] **Step 4: Verify pass** — CI: `cargo test --lib layout::box_tree` + `cargo test --lib` green. Existing
      goldens: only `httpforever` changes (verified in Task 5); all `&[]`-passing callers unchanged.
- [ ] **Step 5: Commit** — `feat(layout): synthesize ::before/::after generated boxes in the box tree (Acid2 P3)`

---

### Task 5: Generated-content fixtures + accept.sh wiring + controller bless

**Files:**
- Create: `fixtures/gc-before-string.html`, `fixtures/gc-empty-box.html`, `fixtures/gc-none.html`
- Modify: `accept.sh` (A5k/A5l/A5m PNG-golden blocks, mirroring the A5h `z-order` block)
- Bless: `goldens/gc-*.png` + re-bless `goldens/httpforever.{light,dark}.png` (**controller**, from CI render)

- [ ] **Step 1: Write the fixtures**

`fixtures/gc-before-string.html`:
```html
<html><body><p style="font-family:monospace">MID</p><style>p::before{content:"["}p::after{content:"]"}</style></body></html>
```

`fixtures/gc-empty-box.html` (the Acid2/httpforever pattern — empty content, styled box):
```html
<html><body><div style="position:relative;width:120px;height:60px"><style>div::before{content:"";position:absolute;top:10px;left:10px;width:40px;height:40px;background:red}</style></div></body></html>
```

`fixtures/gc-none.html` (negative control — no box generated):
```html
<html><body><p>plain</p><style>p::before{content:none}span::after{content:"never"}</style></body></html>
```

- [ ] **Step 2: Wire into accept.sh** — mirror the A5h `z-order` block EXACTLY (grep `A5h`/`Z_ORDER` in
      `accept.sh`): add A5k (`gc-before-string`), A5l (`gc-empty-box`), A5m (`gc-none`), each with its
      `goldens/gc-*.png`, tmp `/tmp/stele_a5{k,l,m}.png`, following the blessed/compare/missing structure.
- [ ] **Step 3: Push; render via CI** (implementer stops here; blessing is controller work).
- [ ] **Step 4 (CONTROLLER): bless the goldens, pixel-verified** — measure each `gc-*.png`: `gc-before-string`
      shows `[MID]` (generated `[`/`]` bracket the text, in order); `gc-empty-box` shows a red 40×40 square at
      (10,10)+margin inside the relative div (empty generated box carries its box model); `gc-none` shows only
      `plain` (no generated box). Then re-render + re-verify `httpforever.{light,dark}` (its `.hero::before/
      ::after` motif now paints — confirm it's a sensible decorative overlay, not a regression), and re-bless.
      Copy verified PNGs into `goldens/`. Never rubber-stamp (AGENTS.md §4).
- [ ] **Step 5: Commit fixtures + accept.sh** — `test(css): generated-content micro-fixtures + accept.sh wiring (Acid2 P3)`

---

### Task 6: Charter amendment + DECISIONS + JOURNAL

**Files:** `stele-charter.md`, `DECISIONS.md`, `JOURNAL.md`

- [ ] **Step 1: Charter** — C2 "What Stele Speaks": add `::before`/`::after` + `content` (string/empty/url) as
      a C2 amendment (Acid2 Packet 3), mirroring the P1/P2 entries.
- [ ] **Step 2: DECISIONS** — prepend the next D-number under a `## Style — generated content (Acid2 Packet 3)`
      heading: generated content via a second `cascade_pseudo` walk (not a `cascade`-signature change) + box-
      tree synthesis; `content:""` generates a box; pseudo rules excluded from the element's own style;
      generated boxes reuse `base_style`/`emit` so P1 positioning + P2 z-index apply for free; the
      `content:url()` image-render DEFERRAL + revisit trigger (P7 verify).
- [ ] **Step 3: JOURNAL** — append: Acid2 Packet 3 landed; the verified fixtures; the httpforever re-bless
      (motif boxes now render); i486 size delta; note the Acid2 face gains generated boxes.
- [ ] **Step 4: Commit** — `docs(css): charter C2 + DECISIONS + JOURNAL for generated content (Acid2 P3)`

---

## Self-Review

**1. Spec coverage:** §1 selector/parse → T1; §2 `content` → T2; §3 `cascade_pseudo` + element-fold exclusion
→ T3; §4 box-tree synthesis → T4; §5 wiring → T4; §6 fixtures/tests → T1–T4 (unit) + T5 (goldens); §7
charter/decisions → T6. `content:url()` deferral recorded in spec + T6 DECISIONS. ✓

**2. Placeholder scan:** no TBD; each step has concrete code or an explicit "grep the existing idiom" pointer
where the exact tokenizer/url spellings must be copied, not guessed (string-literal token name, url() helper,
non-`Copy` computed-field handling). ✓

**3. Type consistency:** `PseudoElement` (T1) → `Selector.pseudo_element` → consumed by `cascade_pseudo` (T3);
`Content`/`generates_box` (T2) → `ComputedStyle.content` → `cascade_pseudo` gate (T3) + `generated_node` (T4);
`PseudoStyles`/`cascade_pseudo` (T3) → `build_box_tree` param (T4). All consistent. ✓
