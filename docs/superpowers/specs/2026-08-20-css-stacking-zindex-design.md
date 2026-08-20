# CSS stacking contexts + `z-index` (Acid2 Packet 2) — design

**Date:** 2026-08-20
**Status:** approved design, pre-implementation
**Program:** Acid2 roadmap Packet 2 of 7 (`docs/superpowers/specs/2026-08-19-acid2-roadmap.md`).
Builds directly on P1 (positioning) — the face layers overlap and must composite in the right order.

## Goal

Support CSS `z-index` (`auto | <integer>`) on positioned elements and paint overlapping boxes in **CSS 2.1
Appendix-E** stacking order, **Acid2-sufficient**, by refining P1's existing `emit` paint-order partition
(`src/layout/block.rs`). No new layout math — this is a paint-ordering change plus a one-property style
addition. Reuse the proven fragment-emit seam rather than build a separate stacking-context tree.

This is a **C2 charter amendment** (dialect growth): `z-index` is core document-web CSS, the sibling of the
positioning that P1 landed.

## Non-negotiables this design serves

- **1.44 MB floppy = 1,474,560 bytes.** No new dependency (paint-order sort + one enum field). Report the
  i486 delta from the CI `stele-i486` artifact; expected tiny.
- **Goldens byte-compared; pixel-verify before blessing.** New z-index micro-fixtures get PNG goldens,
  measured programmatically before blessing (AGENTS.md §4). **Existing goldens must not move:** with no
  `z-index` declared, every positioned element is `z-index: auto` → paint layer 0 → byte-identical to P1.
- **Test-first, root-cause-first.** Each behavior lands with a failing test first.
- **No JavaScript / no uninvited computation (C3):** untouched — style/paint only.
- **Parsing is TOTAL:** unknown/malformed `z-index` degrades to the initial value (`auto`), never a panic.

## Current state (ground-truthed)

- **P1 already partitions paint order** in `emit`'s `Built::Container` arm (`src/layout/block.rs`): it emits
  the container's own box, then **static (in-flow) children in source order**, then **positioned children in
  source order** — via `built_position(b: &Built) -> Position` (`Container`/`Replaced`/`Table` → `style.position`,
  `Inline` → `Static`). This is CSS 2.1 Appendix-E steps 3–6 **without z-index**.
- `emit` recurses per child, emitting **each child's whole subtree contiguously** — so a positioned subtree
  is already painted as an atomic unit. Ordering positioned *siblings* by z-index at each container level
  therefore approximates nested stacking contexts natively (see §Design step 2's model note).
- **No `z-index` anywhere in the codebase yet** (grep-confirmed). `ComputedStyle` has `position`/`inset`
  (P1), `float`, etc.

## Design

### 1. Style + parse

- **`computed.rs`:** add to `ComputedStyle`:
  - `pub z_index: ZIndex` — new `pub enum ZIndex { Auto, Layer(i32) }`, default `Auto`, **non-inherited**.
    (`Auto` and `Layer(0)` paint in the same Appendix-E step 6; the distinction that `Layer(n)` establishes a
    stacking context while `Auto` does not is not modeled — see the model note. Deriving `Clone, Copy,
    PartialEq, Eq, Debug`.)
  - Add a helper `impl ZIndex { pub fn layer(self) -> i32 { match self { Auto => 0, Layer(n) => n } } }` — the
    paint classifier (`Auto` sorts as 0, the step-6 layer).
- **`value.rs` `apply_property`:** add arm `"z-index"` → `auto` ⇒ `ZIndex::Auto`; a valid CSS integer ⇒
  `ZIndex::Layer(n)` (accept optional leading `+`/`-`; reject non-integers like `1.5`); anything else ⇒ leave
  unset (cascades to the `auto` initial value). Mirror the totality of the `position` arm. Store on
  `Declarations` as `z_index: Option<ZIndex>`.
- **`cascade.rs`:** cascade `z_index` as an **own** (non-inherited) property, mirroring `position`.
- **`ua.rs`:** no change (initial value `auto` is correct; no UA rule needs z-index).

### 2. Paint order (`block.rs` `emit`, `Built::Container` arm)

Replace P1's two-pass `[static][positioned]` walk with the full CSS 2.1 Appendix-E order. **z-index applies
only to positioned elements** (`built_position(c) != Position::Static`); a static element's `z-index` has no
effect (it stays in the in-flow pass). Add a helper:

```rust
/// A `Built` child's paint layer for z-index ordering: a POSITIONED element's
/// computed z-index (`Auto` == 0), or `0` for a static/in-flow element (z-index
/// has no effect on non-positioned boxes). Used only to bucket/sort positioned
/// children; static children are emitted in the in-flow pass regardless.
fn z_layer(b: &Built) -> i32 { /* Container/Replaced/Table → style.z_index.layer(); Inline → 0 */ }
```

Emit order within the container (back to front), each pass emitting whole subtrees contiguously:
1. the container's own box (background + border) — **unchanged** (Appendix-E step 1).
2. **negative-z positioned children**: `built_position != Static && z_layer < 0`, **stable-sorted ascending**
   (most-negative painted first) — Appendix-E step 2, **before** in-flow content.
3. **static / in-flow children**: `built_position == Static`, **source order** — steps 3–5 (as P1).
4. **z-auto/0 positioned children**: `built_position != Static && z_layer == 0`, **source order** — step 6.
5. **positive-z positioned children**: `built_position != Static && z_layer > 0`, **stable-sorted ascending**
   (least-positive first) — step 7.

Use a **stable** sort (`slice::sort_by_key`) so equal z-index preserves source order (CSS tie-break = tree
order). **Golden-safety:** when nothing declares `z-index`, every element is `z_layer == 0`, passes 2 and 5
are empty, and passes 3+4 reproduce P1's `[static][positioned]` order exactly → byte-identical output.

**Stacking-context model (the risk to verify, mirroring P1's CB approach):** true CSS distinguishes
`z-index: auto` (no new stacking context; a positioned descendant participates in the *ancestor's* context)
from `z-index: <integer>` (establishes a context; the subtree paints atomically at that stack level). This
design treats **every positioned subtree as atomic** (emit already emits it contiguously) and orders
positioned siblings by z-index at each container level — a faithful model when z-index'd boxes are siblings
(Acid2's overlapping-layers pattern), and an approximation when an `auto`-positioned element has z-index'd
positioned descendants that should escape into the grandparent's context. **Verify against Acid2's actual
layers; add a minimal correction (hoist a descendant context to its true stacking parent) only if a fixture
shows a wrong overlap.** Do not build this speculatively (YAGNI — the test defines the dialect growth).

### 3. Testing / fixtures

- **Unit (pure, CI):** `apply_property` parses `z-index` totally (`auto`, positive/negative integer, `+N`,
  malformed → unset); `ZIndex::layer()` maps `Auto`→0 / `Layer(n)`→n; and an `emit`-level test (mirroring
  P1's `positioned_child_paints_after_static_sibling_...` in `tests/layout_block.rs`) that builds overlapping
  positioned children with different z-index and asserts the emitted `Vec<Fragment>` order (higher z-index at
  a later index; a negative-z child before the static in-flow sibling).
- **Golden micro-fixtures** (`fixtures/z-*.html`, PNG goldens, pixel-verified):
  - `z-order.html` — two overlapping `absolute` boxes; the box **later in source** has the **lower** z-index,
    so the **earlier, higher-z** box paints on top (proves z-index overrides source order).
  - `z-negative.html` — an `absolute` box with `z-index:-1` overlapping in-flow content; the negative box
    paints **behind** the in-flow content (proves step 2 < step 3).
  - `z-tie.html` — two overlapping positioned boxes with **equal** z-index; the later-in-source one paints on
    top (proves the tree-order tie-break / stable sort).
- Bless goldens from the CI `renders` artifact after measuring them correct (controller work; no local i486
  build).

### 4. Charter / decisions

- Amend `stele-charter.md` "What Stele Speaks": `z-index` (+ CSS 2.1 Appendix-E stacking order) enters the
  dialect (C2 amendment, Acid2 Packet 2).
- `DECISIONS.md`: entry — z-index via refining P1's `emit` partition (not a separate stacking-context tree);
  the atomic-positioned-subtree model + its verify-then-correct risk; the `auto`≡`0`-for-sibling-order
  simplification; revisit trigger.

## Out of scope (YAGNI — other packets or never)

- `opacity`/`transform`/`filter`/`mix-blend-mode` and the other CSS3 stacking-context triggers — Acid2 is
  CSS 2.1; only positioned + `z-index` establish contexts here.
- Flex/grid item `z-index` without positioning (CSS3) — not exercised by Acid2.
- True stacking-context escape for `z-index: auto` descendants — only if Acid2 exposes it (verify-then-correct).
- `:hover`/dynamic re-composite — never (static reference only).
