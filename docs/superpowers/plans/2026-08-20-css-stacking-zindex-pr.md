# CSS stacking + z-index (Acid2 Packet 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Support CSS `z-index` (`auto | <integer>`) on positioned elements and paint overlapping boxes in
CSS 2.1 Appendix-E stacking order, by refining P1's `emit` paint-order partition.

**Architecture:** One style property (`z_index` on `ComputedStyle`, parsed/cascaded like `position`) plus a
paint-ordering refinement in `src/layout/block.rs` `emit`: the P1 `[static][positioned]` partition becomes
the five-pass Appendix-E order `[own box][negative-z][in-flow][z-auto/0][positive-z]`, positioned children
bucketed and stably sorted by z-index. No layout-geometry change.

**Tech Stack:** Rust; taffy 0.13 (unchanged — z-index is paint-only); existing style pipeline
(`value.rs`/`cascade.rs`/`computed.rs`) and fragment-emit seam (`block.rs`).

**Spec:** `docs/superpowers/specs/2026-08-20-css-stacking-zindex-design.md`

## Global Constraints

- **1.44 MB floppy = 1,474,560 bytes.** No new dependency. Report the i486 delta from the CI `stele-i486`
  artifact.
- **Existing goldens must not move:** with no `z-index` declared, every element is `z_index: auto` →
  `z_layer == 0` → passes reproduce P1's order byte-for-byte. A `z-index`-free document is unchanged.
- **Parsing is TOTAL:** unknown/malformed `z-index` → the initial value (`auto`); never panic.
- **No local i486 builds** — CI (m0-acceptance) compiles/tests; PNG goldens blessed from the CI artifact,
  pixel-verified (controller work, AGENTS.md §4).
- **Test-first, root-cause-first.** No JavaScript / no uninvited computation (C3): style/paint only.
- **z-index affects ONLY positioned elements** (`position != static`); a static box's z-index has no effect.

---

### Task 1: `z-index` in the style pipeline (parse → cascade → computed)

**Files:**
- Modify: `src/style/computed.rs` (add `ZIndex` enum + `ComputedStyle.z_index`)
- Modify: `src/style/value.rs` (`apply_property` `"z-index"` arm + `Declarations.z_index`)
- Modify: `src/style/cascade.rs` (cascade `z_index`, own/non-inherited)
- Test: unit tests in `src/style/value.rs` (parse) — mirror the existing `position` parse tests

**Interfaces:**
- Produces: `stele::style::computed::ZIndex { Auto, Layer(i32) }` (derives `Debug,Clone,Copy,PartialEq,Eq`);
  `impl ZIndex { pub fn layer(self) -> i32 }` (`Auto`→0, `Layer(n)`→n); `ComputedStyle.z_index: ZIndex`
  (default `Auto`, non-inherited). Task 2 consumes `z_index`/`layer()`.

- [ ] **Step 1: Write the failing tests** (in `src/style/value.rs`'s test module, mirroring the `position`
      parse tests)

```rust
#[test]
fn parses_z_index_keyword_and_integers() {
    // auto keyword
    let mut d = Declarations::default();
    assert!(apply_property("z-index", &tokenize("auto"), &mut d));
    assert_eq!(d.z_index, Some(ZIndex::Auto));
    // positive integer
    let mut d = Declarations::default();
    assert!(apply_property("z-index", &tokenize("5"), &mut d));
    assert_eq!(d.z_index, Some(ZIndex::Layer(5)));
    // negative integer
    let mut d = Declarations::default();
    assert!(apply_property("z-index", &tokenize("-1"), &mut d));
    assert_eq!(d.z_index, Some(ZIndex::Layer(-1)));
    // explicit +
    let mut d = Declarations::default();
    assert!(apply_property("z-index", &tokenize("+3"), &mut d));
    assert_eq!(d.z_index, Some(ZIndex::Layer(3)));
}

#[test]
fn z_index_is_total_on_garbage() {
    // non-integer / unknown -> unset (cascades to auto), no panic
    let mut d = Declarations::default();
    assert!(!apply_property("z-index", &tokenize("1.5"), &mut d));
    assert_eq!(d.z_index, None);
    let mut d = Declarations::default();
    assert!(!apply_property("z-index", &tokenize("banana"), &mut d));
    assert_eq!(d.z_index, None);
}

#[test]
fn z_index_layer_helper() {
    assert_eq!(ZIndex::Auto.layer(), 0);
    assert_eq!(ZIndex::Layer(0).layer(), 0);
    assert_eq!(ZIndex::Layer(-2).layer(), -2);
    assert_eq!(ZIndex::Layer(7).layer(), 7);
}
```
(Use the SAME tokenizer/`Declarations` helpers the neighboring `position` parse tests use — read those first
and copy their idiom, incl. how `tokenize`/token construction is spelled in this file.)

- [ ] **Step 2: Verify fail** — CI: `cargo test --lib style::value` → FAIL (`ZIndex`/`z-index`/`z_index`
      undefined).

- [ ] **Step 3: Implement**
  - `computed.rs`: add
    ```rust
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ZIndex { Auto, Layer(i32) }
    impl ZIndex { pub fn layer(self) -> i32 { match self { ZIndex::Auto => 0, ZIndex::Layer(n) => n } } }
    ```
    and `pub z_index: ZIndex` on `ComputedStyle` with `z_index: ZIndex::Auto` in its `Default`/initializer
    (place next to `position`; match how `position`'s default is set).
  - `value.rs`: add `z_index: Option<ZIndex>` to `Declarations` (next to `position`); in `apply_property`
    add the `"z-index"` arm:
    ```rust
    "z-index" => {
        // auto | <integer>; totality: anything else leaves it unset (=> auto).
        match tokens.first() {
            Some(t) if t.is_ident("auto") /* match the file's ident test idiom */ => {
                d.z_index = Some(ZIndex::Auto); true
            }
            Some(t) => match /* the file's integer-from-token parse */ {
                Some(n) => { d.z_index = Some(ZIndex::Layer(n)); true }
                None => false,
            },
            None => false,
        }
    }
    ```
    Use whatever token→ident / token→integer helpers already exist in `value.rs` (grep for how an existing
    integer-valued or keyword property is parsed — e.g. a keyword arm like `position` for the ident test, and
    any integer parse already present; if none, parse `token.text().parse::<i32>()` guarded to reject a value
    containing `.`). Reject non-integers (`1.5`) → return `false`.
  - Add `z_index` to the `ov!`/overlay list in `value.rs` if declarations are merged there (mirror exactly
    how `position` was added to that list in P1 — grep `position` in `value.rs`).
  - `cascade.rs`: resolve `z_index` as own/non-inherited: `z_index: own!(z_index)` (mirror `position:
    own!(position)`).

- [ ] **Step 4: Verify pass** — CI: `cargo test --lib style` → the new tests pass; existing style tests
      unchanged.

- [ ] **Step 5: Commit** — `feat(css): parse+cascade z-index (auto | <integer>) (Acid2 P2)`

---

### Task 2: CSS 2.1 Appendix-E paint order in `emit` (z-index layers)

**Files:**
- Modify: `src/layout/block.rs` (`emit` `Built::Container` arm + new `z_layer` helper)
- Test: `tests/layout_block.rs` (paint-order integration test, mirroring P1's
  `positioned_child_paints_after_static_sibling_regardless_of_source_order`)

**Interfaces:**
- Consumes: `ComputedStyle.z_index`/`ZIndex::layer()` (Task 1); `built_position` (P1, already in `block.rs`).
- Produces: the five-pass emit order; no new public API.

- [ ] **Step 1: Write the failing test** (in `tests/layout_block.rs`, copying the idiom of
      `positioned_child_paints_after_static_sibling_regardless_of_source_order`)

```rust
#[test]
fn positioned_children_paint_in_z_index_then_source_order() {
    // Three absolute children, all overlapping, DISTINCT sizes so each Box
    // fragment is identifiable. Source order: A(z=1), B(z=-1), C(z auto=0).
    // Appendix-E paint order (back->front): B(neg) < in-flow(none) < C(auto/0)
    // < A(pos). So emitted Box indices must satisfy: B < C < A.
    let abs = |w: f32, h: f32, z: ZIndex| {
        let mut s = block_style();
        s.position = Position::Absolute;
        s.z_index = z;
        s.width = Dimension::Px(w);
        s.height = Dimension::Px(h);
        s.inset = Edges { top: LengthPercentageAuto::Px(0.0), left: LengthPercentageAuto::Px(0.0),
                          right: LengthPercentageAuto::Auto, bottom: LengthPercentageAuto::Auto };
        leaf_container(s)
    };
    let a = abs(111.0, 11.0, ZIndex::Layer(1));
    let b = abs(122.0, 22.0, ZIndex::Layer(-1));
    let c = abs(133.0, 33.0, ZIndex::Auto);
    let root = container(block_style(), vec![a, b, c]);
    let fragments = layout(&root, Size { w: 300.0, h: 200.0 });
    let boxes = box_fragments(&fragments);
    let idx = |w: f32, h: f32| boxes.iter().position(|f| f.rect.size.w == w && f.rect.size.h == h).unwrap();
    let (ia, ib, ic) = (idx(111.0, 11.0), idx(122.0, 22.0), idx(133.0, 33.0));
    assert!(ib < ic && ic < ia, "z-order back->front B(-1) < C(0) < A(1): B={ib} C={ic} A={ia}");
}
```
(Match the exact helper names/imports used by the sibling P1 test — `block_style`, `leaf_container`,
`container`, `box_fragments`, `layout`, `Dimension`, `Edges`, `LengthPercentageAuto`, `Position`; add
`ZIndex` to the `use stele::style::computed::{...}` line.)

- [ ] **Step 2: Verify fail** — CI: `cargo test --test layout_block positioned_children_paint_in_z_index` →
      FAIL (current emit ignores z-index; C(auto,source-last) currently paints after A).

- [ ] **Step 3: Implement** — in `emit`'s `Built::Container` arm, replace P1's two-pass loop:
```rust
Built::Container { style, children, interactive, .. } => {
    out.push(Fragment { rect: Rect { origin, size },
        kind: FragmentKind::Box { style: (*style).clone() }, interactive: interactive.clone() });
    // CSS 2.1 Appendix E stacking order (back to front). z-index affects only
    // positioned children; static children stay in the in-flow pass. emit()
    // paints each child's whole subtree contiguously (atomic), so ordering
    // positioned siblings by z-index approximates nested stacking contexts.
    let is_pos = |c: &&Built| built_position(c) != Position::Static;
    // 2. negative-z positioned, most-negative first (stable)
    let mut neg: Vec<&Built> = children.iter().filter(|c| is_pos(c) && z_layer(c) < 0).collect();
    neg.sort_by_key(|c| z_layer(c));
    for c in neg { emit(c, taffy, origin, metrics, out); }
    // 3-5. in-flow (static) children, source order
    for c in children.iter().filter(|c| built_position(c) == Position::Static) {
        emit(c, taffy, origin, metrics, out);
    }
    // 6. z-index auto/0 positioned children, source order
    for c in children.iter().filter(|c| is_pos(c) && z_layer(c) == 0) {
        emit(c, taffy, origin, metrics, out);
    }
    // 7. positive-z positioned children, least-positive first (stable)
    let mut pos: Vec<&Built> = children.iter().filter(|c| is_pos(c) && z_layer(c) > 0).collect();
    pos.sort_by_key(|c| z_layer(c));
    for c in pos { emit(c, taffy, origin, metrics, out); }
}
```
  and add the helper near `built_position`:
```rust
/// A `Built` child's z-index paint layer: a positioned element's computed
/// z-index (`Auto` == 0), or 0 for a static/inline child (z-index has no effect
/// on non-positioned boxes; static children are emitted in the in-flow pass
/// regardless). Used only to bucket/sort positioned children in `emit`.
fn z_layer(b: &Built) -> i32 {
    match b {
        Built::Container { style, .. } | Built::Replaced { style, .. } | Built::Table { style, .. } =>
            style.z_index.layer(),
        Built::Inline { .. } => 0,
    }
}
```

- [ ] **Step 4: Verify pass** — CI: `cargo test --test layout_block` (new test + P1's paint-order test both
      pass) and `cargo test --lib layout::block`. Existing A1–A5 goldens must be **byte-identical** (no
      fixture uses z-index yet → every child `z_layer==0` → passes 3+4 reproduce P1's `[static][positioned]`).

- [ ] **Step 5: Commit** — `feat(layout): CSS 2.1 Appendix-E paint order with z-index layers (Acid2 P2)`

---

### Task 3: z-index golden fixtures + accept.sh wiring + stacking verification

**Files:**
- Create: `fixtures/z-order.html`, `fixtures/z-negative.html`, `fixtures/z-tie.html`
- Modify: `accept.sh` (wire each as a PNG golden, mirroring P1's A5c–A5g `pos-*` wiring)
- Bless: `goldens/z-order.png`, `goldens/z-negative.png`, `goldens/z-tie.png` (**controller**, from CI render)

**Interfaces:**
- Consumes: Task 1+2 (z-index parsed + Appendix-E emit). No code API.

- [ ] **Step 1: Write the fixtures** (small, self-contained; each isolates one behavior)

`fixtures/z-order.html` — later-in-source box has LOWER z-index, so the earlier higher-z box wins:
```html
<html><body><div style="position:relative;width:200px;height:120px">
<div style="position:absolute;top:0;left:0;width:120px;height:80px;background:red;z-index:2"></div>
<div style="position:absolute;top:40px;left:60px;width:120px;height:80px;background:blue;z-index:1"></div>
</div></body></html>
```
(Overlap region: the RED box, z-index:2, must paint ON TOP of the blue z-index:1 box even though blue is
later in source.)

`fixtures/z-negative.html` — negative-z box paints behind in-flow content:
```html
<html><body><div style="position:relative;width:200px;height:100px;background:#cccccc">
<div style="position:absolute;top:0;left:0;width:150px;height:60px;background:green;z-index:-1"></div>
in-flow text over the negative box</div></body></html>
```
(The GREEN z-index:-1 box must paint BEHIND the gray container's in-flow text/background content.)

`fixtures/z-tie.html` — equal z-index → later-in-source paints on top:
```html
<html><body><div style="position:relative;width:200px;height:120px">
<div style="position:absolute;top:0;left:0;width:120px;height:80px;background:red;z-index:5"></div>
<div style="position:absolute;top:40px;left:60px;width:120px;height:80px;background:blue;z-index:5"></div>
</div></body></html>
```
(Equal z-index:5 → the later BLUE box paints on top of red in the overlap — tree-order tie-break.)

- [ ] **Step 2: Wire the fixtures into accept.sh** — mirror EXACTLY how P1's `pos-*` fixtures are wired
      (grep `pos-absolute` / `A5c` in `accept.sh`): add `z-order`, `z-negative`, `z-tie` as new A5 sub-labels
      (e.g. A5h–A5j), each with its `goldens/z-*.png`, following the same blessed-vs-compare-vs-missing
      structure.

- [ ] **Step 3: Push; render via CI** — the render gallery produces `renders/z-*.png`. (Implementer stops
      here for goldens; blessing is controller work.)

- [ ] **Step 4 (CONTROLLER, not the implementer): bless the goldens from the CI render, pixel-verified** —
      measure each `z-*.png` programmatically (connected-component / color bbox + overlap-band sampling):
      `z-order` red-on-top in the overlap, `z-negative` green behind the gray container content,
      `z-tie` blue-on-top in the overlap. Only then copy into `goldens/`. Never rubber-stamp (AGENTS.md §4).

- [ ] **Step 5: Stacking-context verification (the spec's risk)** — confirm taffy/emit place Acid2's
      overlapping z-index'd layers in the right order with the atomic-positioned-subtree model. If a fixture
      needs a `z-index: auto` descendant to escape into its grandparent's stacking context (skip-level
      context) and the flat per-container model misorders it, record a FINDING (do not build speculative
      hoisting). Commit fixtures + accept.sh: `test(css): z-index paint-order micro-fixtures + accept.sh wiring (Acid2 P2)`.

---

### Task 4: Charter amendment + DECISIONS + JOURNAL

**Files:**
- Modify: `stele-charter.md` (C2 "What Stele Speaks")
- Modify: `DECISIONS.md` (new D-number, newest first)
- Modify: `JOURNAL.md` (append, newest last)

- [ ] **Step 1: Charter** — in the C2 ADOPTED AMENDMENTS list, append `z-index` + CSS 2.1 Appendix-E
      stacking order as a C2 amendment (Acid2 Packet 2), mirroring the P1 positioning entry's phrasing.

- [ ] **Step 2: DECISIONS** — prepend an entry (next free D-number; match house format under a `## Layout —
      CSS stacking / z-index (Acid2 Packet 2)` heading): z-index via refining P1's `emit` partition into the
      five-pass Appendix-E order (not a separate stacking-context tree); the atomic-positioned-subtree model +
      its verify-then-correct risk (skip-level `auto` context escape); the `auto`≡`0`-for-sibling-order
      simplification; golden-safety (no z-index ⇒ byte-identical to P1); revisit trigger. Record the Task 3
      Step 5 verification result and any FINDING.

- [ ] **Step 3: JOURNAL** — append: Acid2 Packet 2 (stacking/z-index) landed via the emit-partition
      refinement; the stacking verification result; the i486 size delta from CI; a note on how much closer the
      Acid2 layers now composite (still ahead: generated content P3, data: URIs P4, min/max/overflow/bg-pos
      P5, `<object>` P6, assembly P7).

- [ ] **Step 4: Commit** — `docs(css): charter C2 amendment + DECISIONS + JOURNAL for z-index (Acid2 P2)`

---

## Self-Review

**1. Spec coverage:** §1 style+parse → Task 1; §2 paint order (five-pass Appendix-E + `z_layer`) → Task 2;
§3 fixtures/tests → Tasks 1/2 (unit) + Task 3 (goldens); §4 charter/decisions → Task 4. Stacking-context risk
(§2 model note) → Task 3 Step 5 (verify-then-correct, mirroring P1's CB verification). ✓

**2. Placeholder scan:** no TBD/TODO; every step has concrete code or an explicit "match the existing idiom"
pointer to a named P1 precedent (the one unavoidable indirection — the exact token-parse spellings live in
`value.rs` and must be copied, not guessed). ✓

**3. Type consistency:** `ZIndex { Auto, Layer(i32) }` + `layer() -> i32` defined in Task 1, consumed by
Task 2's `z_layer`/emit and Task 2's test; `ComputedStyle.z_index`, `Declarations.z_index: Option<ZIndex>`,
`built_position` (P1) all referenced consistently. ✓
