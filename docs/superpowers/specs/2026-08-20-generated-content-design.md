# Generated content — `:before`/`:after` + `content` (Acid2 Packet 3) — design

**Date:** 2026-08-20
**Status:** approved design, pre-implementation
**Program:** Acid2 roadmap Packet 3 of 7 (`docs/superpowers/specs/2026-08-19-acid2-roadmap.md`).

## Goal

Support CSS generated content: the `::before` / `::after` pseudo-elements and the `content` property
(`normal | none | <string> | url(<url>)`), synthesizing pseudo-element boxes in the style→box-tree pipeline
so they lay out and paint like real boxes. **Acid2-sufficient** — Acid2 builds parts of the face from
generated content (`:before`/`:after` ×4, `content:` ×2), typically **empty or short strings on positioned,
background-styled boxes** (the same shape httpforever's `.hero::before/::after { content:""; position:
absolute; inset:0 }` decorative motif already uses — currently dropped at parse time).

This is a **C2 charter amendment** (dialect growth): generated content is core document-web CSS.

## Non-negotiables this design serves

- **1.44 MB floppy = 1,474,560 bytes.** No new dependency (selector + style + box-tree plumbing). Report the
  i486 delta from the CI `stele-i486` artifact.
- **Goldens byte-compared; pixel-verify before blessing.** New generated-content micro-fixtures get PNG
  goldens, measured programmatically before blessing (AGENTS.md §4). **Golden impact:** exactly ONE existing
  golden moves — `httpforever` (its `.hero::before/::after` empty-motif boxes, currently dropped because
  `::before` parses to `supported = false`, now render). No other fixture uses a pseudo-element selector
  (grep-verified). A `::before`/`::after`-free document is byte-identical.
- **Test-first, root-cause-first.** Each behavior lands with a failing test first.
- **No JavaScript / no uninvited computation (C3):** untouched — style/box-tree only; generated content is
  author-declared, inert, and static.
- **Parsing is TOTAL:** unknown/malformed `content` degrades to `normal` (⇒ no box); an unknown `::x`
  pseudo-element still drops the rule (`supported = false`), never a panic.

## Current state (ground-truthed)

- **`::before`/`::after` are parsed but DISCARDED.** `parser.rs` (~line 397): a `::` pseudo-element sets
  `supported = false` (the whole rule is dropped); a single-colon `:before`/`:after` falls through the
  pseudo-*class* match to `_ => supported = false`. `selector::Pseudo` has only `Link`/`Visited`/`Root`
  (pseudo-classes). `Selector` is `{ compounds: Vec<Compound> }`.
- **`content` is not parsed** (no `"content"` arm in `value.rs`).
- **The cascade** (`cascade::cascade(dom, author_sheets) -> Vec<ComputedStyle>`, indexed by `NodeId`) folds
  matching rules per element via `parser::matching_rules` + `fold_matching_declarations`. ~10 callers (mostly
  tests + `browser.rs`); its return type is load-bearing, so leave it unchanged.
- **The box tree** (`box_tree::build_box_tree(dom, styles, images) -> Option<LayoutNode>` → `build_node`)
  turns each DOM node into a `LayoutNode` (`Container` + children, or `Text`). ~10 call sites (browser.rs,
  frames.rs, main.rs ×7, one test). Generated boxes are synthesized HERE, as prepended/appended children.
- **Images** are fetched/decoded into a `NodeId -> Rc<RgbaImage>` map (`images::collect_images`). Generated
  boxes have no `NodeId` — see the `content: url()` scope note.

## Design

### 1. Selector: pseudo-elements (`selector.rs`, `parser.rs`)

- **`selector.rs`:** add `pub enum PseudoElement { Before, After }` and `pub pseudo_element:
  Option<PseudoElement>` on `Selector` (a pseudo-element attaches to the selector's SUBJECT — the rightmost
  compound — and there is at most one). `Selector::matches`/`Compound::matches` are UNCHANGED (a
  pseudo-element selector still matches its subject *element*; the pseudo-element only routes the
  declarations to a generated box). Specificity: a pseudo-element counts as one element (fold into the
  element count — CSS 2.1 §6.4.3).
- **`parser.rs`:** when the subject compound ends in `::before`/`::after` (double colon) or the legacy
  `:before`/`:after` (single colon), set `selector.pseudo_element = Some(Before|After)` and keep the rule
  SUPPORTED (remove the `supported = false` for these). Any other `::x` or unknown pseudo stays unsupported.
  A pseudo-element must be the last simple selector; anything after it drops the rule.

### 2. `content` property (`value.rs`, `computed.rs`, `cascade.rs`)

- **`computed.rs`:** `pub enum Content { Normal, None, Str(String), Url(String) }` (default `Normal`,
  non-inherited); `pub content: Content` on `ComputedStyle`. Add `impl Content { pub fn generates_box(&self)
  -> bool }` (`Str(_) | Url(_) => true`; `Normal | None => false`) — note `Str(String::new())` (`content:""`)
  DOES generate a box (empty box with full box model; distinct from `none`).
- **`value.rs`:** `Declarations.content: Option<Content>`; `apply_property` `"content"` arm — `normal` ⇒
  `Normal`; `none` ⇒ `None`; a string token ⇒ `Str(s)` (including the empty string `""`); `url(<u>)` ⇒
  `Url(u)`; anything else ⇒ unset (⇒ `normal`). Add to the `ov!` overlay list.
- **`cascade.rs`:** cascade `content` as an own (non-inherited) property (`content: own!(content)`).

### 3. Cascade of pseudo-element styles (`cascade.rs`)

- **Exclude pseudo rules from the element's own style:** in `fold_matching_declarations` (the element's own
  fold), skip any matched author/UA rule whose `selector.pseudo_element.is_some()` — otherwise a
  `p::before { color:red }` rule would leak `color` onto the `p` element. (Today these rules don't exist in
  the sheet at all, dropped at parse; enabling them in §1 makes this exclusion load-bearing.)
- **Produce pseudo-element styles:** add `pub struct PseudoStyles { pub before: Option<ComputedStyle>, pub
  after: Option<ComputedStyle> }` and `pub fn cascade_pseudo(dom, author_sheets) -> Vec<PseudoStyles>`
  (indexed by `NodeId`, `PseudoStyles::default()` = both `None` for non-elements / no match). For each
  element and each of `{Before, After}`: fold the matching author rules whose `pseudo_element == that`,
  `resolve` a `ComputedStyle` with the ELEMENT's own computed style as the inherited parent (generated
  content inherits from the originating element — CSS 2.1 §12.1), and keep it `Some(..)` **only if**
  `style.content.generates_box()` (else the pseudo-element produces no box — `content: normal|none`).
  Reuse the existing matching/folding/`resolve` machinery; `cascade_pseudo` is a second, cheap DOM walk so
  `cascade`'s hot signature stays untouched.

### 4. Box-tree synthesis (`box_tree.rs`)

- Thread a `pseudo: &[PseudoStyles]` slice into `build_box_tree` and `build_node` (a new parameter; ~10 call
  sites — real pipeline passes the `cascade_pseudo` result, tests/`--dump-text` pass `&[]` ⇒ no generated
  content, byte-identical to today).
- In `build_node` for an ELEMENT, after building its real children: if `pseudo.get(id).before` is `Some(ps)`,
  synthesize a generated `LayoutNode` and PREPEND it; if `.after` is `Some(ps)`, synthesize and APPEND.
  The generated node:
  - `content: Str(s)` ⇒ a `Container` `LayoutNode` styled by the pseudo `ComputedStyle`, with a single
    `Text(s)` child (empty string ⇒ empty text child; the Container still carries the pseudo box's
    background/border/position/size — this is httpforever's and Acid2's empty-colored-box pattern).
  - `content: Url(u)` ⇒ see scope note (parsed; rendering deferred).
  The generated node participates in layout/paint/positioning/z-index exactly like a real box (it flows
  through the same `base_style`/`emit`, so P1 positioning and P2 z-index already apply to it for free).

### 5. Wiring (`browser.rs`, `main.rs`, `frames.rs`)

Each real place that calls `cascade` + `build_box_tree` also calls `cascade_pseudo` and passes the result.
Frames resolve against their own frame document. This is mechanical param-threading; group it with §4.

### 6. Testing / fixtures

- **Unit (pure, CI):** `parser` parses `::before`/`::after`/`:before`/`:after` into `pseudo_element` and
  keeps the rule supported; `value` parses `content` totally (`normal`/`none`/string/`""`/`url()`/garbage);
  `cascade_pseudo` yields a `before`/`after` style only when `content` generates a box, inheriting from the
  element; `fold_matching_declarations` excludes pseudo rules from the element's own style; a `box_tree`
  test that a `::before` with `content:"x"` prepends a generated `Container(+Text)` child (and `::after`
  appends), while `content:none`/no-rule prepends nothing.
- **Golden micro-fixtures** (`fixtures/gc-*.html`, PNG goldens, pixel-verified):
  - `gc-before-string.html` — `::before { content:"[" }` / `::after { content:"]" }` bracket an element's
    text (generated strings appear before/after the real content, in order).
  - `gc-empty-box.html` — `::before { content:""; display:block; width/height; background }` — an empty
    generated box renders as a colored rectangle (the Acid2 / httpforever pattern; proves `content:""` ≠
    `none`).
  - `gc-none.html` — `::before { content:none }` (and an element with no pseudo rule) generate NO box
    (negative control; nothing added).
- **Re-bless** `httpforever.{light,dark}` (its `.hero::before/::after` motif boxes now render) — controller,
  pixel-verified (AGENTS.md §4), same discipline as P1/P2.

### 7. Charter / decisions

- Amend `stele-charter.md` "What Stele Speaks": `::before`/`::after` + `content` (string/empty/url) enter the
  dialect (C2 amendment, Acid2 Packet 3).
- `DECISIONS.md`: entry — generated content via a second `cascade_pseudo` walk + box-tree synthesis (not a
  cascade-signature change); `content:""` generates a box; the `content: url()` deferral + revisit trigger.

## Out of scope (YAGNI — other packets or never)

- **`content: url()` image RENDERING is DEFERRED** (parsed totally in §2, but the generated image is not
  painted). Reason: images are keyed by `NodeId` and generated boxes have none, so painting a pseudo-element
  image needs a separate `(NodeId, PseudoElement)`-keyed fetch/decode path — build it only if Acid2's
  generated content actually uses `url()` (verify at P7 assembly; record a FINDING, no speculative wiring).
  Acid2's and httpforever's generated content is strings/empty boxes.
- Other pseudo-elements (`::first-line`, `::first-letter`, `::marker`, `::selection`) — not in Acid2.
- `content` counters/`attr()`/quotes (`open-quote`, `counter()`) — not in Acid2.
- `:hover`/dynamic generated content — never (static reference only).
