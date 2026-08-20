# CSS Positioning (Acid2 Packet 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Support CSS `position: static|relative|absolute|fixed` + `top/right/bottom/left`, Acid2-sufficient, by plumbing the properties through the style pipeline onto taffy 0.13's native positioning.

**Architecture:** Add `position` + `inset` to `ComputedStyle` (parsed/cascaded exactly like the existing `float`/`clear` and `margin`), then map them onto taffy's `Style.position`/`Style.inset` in `block.rs`'s `base_style` — taffy does the placement. Verify against Acid2's boxes; bespoke containing-block correction only if a skip-level CB gap appears (none built speculatively).

**Tech Stack:** Rust; taffy 0.13 (existing dep — native `Position::{Relative,Absolute}` + `inset`). No new deps.

**Spec:** `docs/superpowers/specs/2026-08-19-css-positioning-design.md` (read alongside). Program roadmap: `docs/superpowers/specs/2026-08-19-acid2-roadmap.md`.

## Global Constraints

- **`position`/`inset` are NON-inherited ("own") box properties** — resolved exactly like `float`/`clear`/`box_sizing`/`border_collapse` (see their `cascade::resolve` treatment). Initial values: `position: static`, all offsets `auto`.
- **`inset` is the SAME type as `margin`: `Edges<LengthPercentageAuto>`.** Parse/cascade/default/taffy-map each of `top/right/bottom/left` by MIRRORING how `margin` is already handled (same type, proven code). Do not invent new value types.
- **`position` is a keyword→enum, like `float`.** MIRROR `float`'s parse/cascade/default handling.
- **No golden churn on `static` documents.** A document with no `position`/offset declarations must render byte-identically (the new fields default to `static`/`auto`, and taffy `Position::Relative` with `auto` insets == the current in-flow behavior). Existing A1–A5 goldens must not move — if any does, root-cause; don't re-bless.
- **1.44 MB floppy = 1,474,560 bytes.** No new dep; report the i486 delta from the CI `stele-i486` artifact.
- **No local builds** (AGENTS.md §3): implementers transcribe/implement + commit; CI compiles + runs `cargo test`. New PNG goldens are blessed from the CI `stele-host`/`renders` artifact after programmatic pixel-verification (never rubber-stamped) — controller work, not the implementer's.
- **Parsing is TOTAL:** unknown `position` keyword / malformed offset → the initial value; never a panic.
- **No JavaScript / no uninvited computation (C3):** untouched.
- **Branch:** `packet/acid2-positioning`, off `main`. Conventional subjects (`feat(css):`, `test(css):`, `docs(...):`).

## File Structure

- **Modify** `src/style/computed.rs` — add `Position` enum + `position`/`inset` fields to `ComputedStyle` and its `Default`.
- **Modify** `src/style/value.rs` — `apply_property` arms for `position` + `top/right/bottom/left` (into the declared/partial style the function mutates); mirror `float`/`margin`.
- **Modify** `src/style/cascade.rs` — resolve `position`/`inset` as own (non-inherited) properties; mirror `float`.
- **Modify** `src/layout/block.rs` — `base_style` maps `position`→taffy `Style.position`, `inset`→taffy `Style.inset` (reuse the margin→taffy conversion).
- **Create** `fixtures/pos-*.html` + **Modify** `accept.sh` — positioning micro-fixtures + A5 golden wiring.
- **Modify** `stele-charter.md`, `DECISIONS.md`, `JOURNAL.md` — C2 amendment + decision + note.

---

### Task 1: `position` + `inset` in the style pipeline (parse → cascade → computed)

Add the properties end-to-end through the style layer, mirroring `float` (enum) and `margin` (edges). Pure, CI-testable.

**Files:** `src/style/computed.rs`, `src/style/value.rs`, `src/style/cascade.rs`.

**Interfaces:**
- Produces: `pub enum Position { Static, Relative, Absolute, Fixed }`; `ComputedStyle.position: Position`; `ComputedStyle.inset: Edges<LengthPercentageAuto>`.

- [ ] **Step 1: Write the failing tests**

Add to `src/style/value.rs`'s `#[cfg(test)]` module (mirror the existing `apply_property("display", …)` tests):

```rust
    #[test]
    fn apply_property_parses_position_keyword() {
        let mut d = Declared::default(); // use the SAME declared-style type the other apply_property tests use
        assert!(apply_property("position", &toks("absolute"), &mut d));
        assert_eq!(d.position, Some(Position::Absolute));
        assert!(apply_property("position", &toks("relative"), &mut d));
        assert_eq!(d.position, Some(Position::Relative));
        assert!(apply_property("position", &toks("fixed"), &mut d));
        assert_eq!(d.position, Some(Position::Fixed));
        assert!(apply_property("position", &toks("static"), &mut d));
        assert_eq!(d.position, Some(Position::Static));
    }

    #[test]
    fn apply_property_parses_inset_offsets_like_margin() {
        let mut d = Declared::default();
        assert!(apply_property("top", &toks("10px"), &mut d));
        assert!(apply_property("left", &toks("-5px"), &mut d));
        assert!(apply_property("right", &toks("auto"), &mut d));
        assert!(apply_property("bottom", &toks("50%"), &mut d));
        // Each edge parsed into the declared inset as the SAME LengthPercentageAuto
        // variants `margin` uses (Length/Percentage/Auto — match margin's names).
        assert!(d.inset_top.is_some() && d.inset_left.is_some() && d.inset_right.is_some() && d.inset_bottom.is_some());
    }
```

Note for the implementer: use whatever declared/partial-style type and `toks(...)` helper the EXISTING `apply_property` tests use (grep the test module). Whether the declared struct stores insets as one `Edges` or four separate `Option`s: FOLLOW how it stores `margin` — mirror that exactly (field names in the assert above are illustrative; match the real ones).

- [ ] **Step 2: Verify fail** — CI: `cargo test --lib style::value` → FAIL (`Position`/`position`/inset undefined).

- [ ] **Step 3: Implement**

- `computed.rs`: add
  ```rust
  /// CSS `position` (Acid2 Packet 1, C2 amendment). Non-inherited box property,
  /// same resolution shape as `float`/`clear`/`box_sizing`. `Static` is the CSS
  /// initial value. `Fixed` maps to absolute-vs-viewport in layout (no scroll in
  /// the static render — see the packet spec).
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum Position { Static, Relative, Absolute, Fixed }
  ```
  and to `ComputedStyle` (near `float`/`margin`): `pub position: Position,` and `pub inset: Edges<LengthPercentageAuto>,`. Add both to `ComputedStyle::default` — `position: Position::Static` and `inset: Edges::all(<the LengthPercentageAuto Auto variant, exactly as margin's default uses>)`.
- `value.rs` `apply_property`: add a `"position"` arm (keyword → `Position`, `static/relative/absolute/fixed`; unknown → return false, mirror `float`'s `_ => false`), and `"top"`/`"right"`/`"bottom"`/`"left"` arms that parse a `LengthPercentageAuto` into the matching declared inset edge — MIRROR the `"margin-top"`/`"margin"` edge parsing verbatim (same value grammar: length | percentage | `auto`).
- `cascade.rs`: resolve `position` and `inset` as OWN (non-inherited) properties — mirror `float`'s `own!(float)` line for `position`, and `margin`'s edge resolution for `inset`.

- [ ] **Step 4: Verify pass** — CI: `cargo test --lib style` → the two new tests pass; existing style tests unchanged.

- [ ] **Step 5: Commit**

```bash
git add src/style/computed.rs src/style/value.rs src/style/cascade.rs
git commit -m "feat(css): parse+cascade position and top/right/bottom/left (Acid2 P1)"
```

---

### Task 2: Map `position`/`inset` onto taffy (`base_style`)

Wire the computed values into taffy so it places the boxes. Pure, CI-testable at the mapping level.

**Files:** `src/layout/block.rs`.

**Interfaces:**
- Consumes: `ComputedStyle.position`/`.inset` (Task 1).

- [ ] **Step 1: Write the failing test**

Add to `src/layout/block.rs`'s `#[cfg(test)]` module (or wherever `base_style` is unit-tested; mirror an existing `base_style`/float mapping test):

```rust
    #[test]
    fn base_style_maps_position_and_inset_to_taffy() {
        let mut cs = ComputedStyle::default();
        cs.position = Position::Absolute;
        cs.inset.top = /* LengthPercentageAuto length 10px, same constructor margin tests use */;
        let ts = base_style(&cs /* + whatever other args base_style takes */);
        assert_eq!(ts.position, taffy::Position::Absolute);
        // inset.top mapped to taffy inset top == 10px, via the SAME conversion base_style
        // uses for margin — assert taffy's inset.top matches taffy's margin.top for the
        // same input value.

        let mut rel = ComputedStyle::default();
        rel.position = Position::Relative;
        assert_eq!(base_style(&rel).position, taffy::Position::Relative);

        // Static and Fixed:
        assert_eq!(base_style(&ComputedStyle::default()).position, taffy::Position::Relative); // Static -> taffy Relative, auto insets
        let mut fx = ComputedStyle::default(); fx.position = Position::Fixed;
        assert_eq!(base_style(&fx).position, taffy::Position::Absolute); // Fixed -> Absolute (viewport CB handled in layout)
    }
```

(Adapt to `base_style`'s real signature/return — grep it; if `base_style` isn't directly unit-tested today, add the smallest test that constructs a `ComputedStyle`, calls it, and asserts the taffy `Style` fields.)

- [ ] **Step 2: Verify fail** — CI: FAIL (`base_style` doesn't set position/inset yet).

- [ ] **Step 3: Implement**

In `base_style` (where it already sets `Style.float`/`.clear`/`.margin` from `ComputedStyle`), add:
```rust
    style.position = match cs.position {
        Position::Static | Position::Relative => taffy::Position::Relative,
        Position::Absolute | Position::Fixed => taffy::Position::Absolute,
    };
    // Only carry insets for non-static boxes (Static stays at its in-flow
    // position — leave taffy's default auto insets).
    if cs.position != Position::Static {
        style.inset = /* convert cs.inset (Edges<LengthPercentageAuto>) to taffy Rect<LengthPercentageAuto>
                         using the SAME per-edge conversion base_style already applies to cs.margin */;
    }
```
Find the existing `cs.margin` → `style.margin` conversion in `base_style` and reuse it verbatim for `inset` (identical `Edges<LengthPercentageAuto>` → taffy `Rect` conversion). For `Fixed`, the viewport containing block is handled by the fact that a fixed box's positioned ancestors are `relative`/`absolute`; if Acid2's fixed box needs explicit root anchoring, that surfaces in Task 3's verification (add correction only then).

- [ ] **Step 4: Verify pass** — CI: `cargo test --lib layout::block` → the mapping test passes; existing layout tests + all A1–A5 goldens unchanged (a `static` doc maps to taffy `Relative`+auto-insets == prior behavior).

- [ ] **Step 5: Commit**

```bash
git add src/layout/block.rs
git commit -m "feat(css): map position/inset onto taffy Style (Acid2 P1)"
```

---

### Task 3: Positioning golden fixtures + containing-block verification

Prove real placement with pixel-verified goldens, and verify the taffy-native containing-block behavior against Acid2's actual pattern (the spec's key risk).

**Files:** Create `fixtures/pos-absolute.html`, `fixtures/pos-relative.html`, `fixtures/pos-fixed.html`, `fixtures/pos-auto-inset.html`, `fixtures/pos-nested.html`; Modify `accept.sh` (A5 wiring).

- [ ] **Step 1: Write the fixtures** (small, self-contained; each isolates one behavior)

- `pos-absolute.html` — a `position:relative` parent (say 200×200, gray bg) containing a `position:absolute; top:20px; left:30px` child box (50×50, red). The child must land at (30,20) inside the parent, out of flow.
- `pos-relative.html` — three stacked blocks; the middle one `position:relative; top:10px; left:15px` — it shifts by (15,10) but the third block stays where it would be if the middle were static (relative keeps flow space).
- `pos-fixed.html` — a `position:fixed; top:0; right:0` box in the viewport's top-right corner.
- `pos-auto-inset.html` — a `position:absolute` box with NO offsets (auto insets) inside a relative parent — it stays at its static (in-flow) origin.
- `pos-nested.html` — `absolute` inside `relative` inside static flow: the absolute's offsets resolve against the relative ancestor's padding box, not the viewport.

- [ ] **Step 2: Wire the fixtures into accept.sh** (mirror how an existing fixture, e.g. `fixtures/grid.html` or `fixtures/hr-rule.html`, is wired for its A5 PNG golden — add each `pos-*` with its `goldens/pos-*.png`).

- [ ] **Step 3: Push; render via CI**

```bash
git add fixtures/pos-*.html accept.sh
git commit -m "test(css): positioning micro-fixtures (Acid2 P1)"
git push -u origin packet/acid2-positioning
```

- [ ] **Step 4 (CONTROLLER, not the implementer): bless the goldens from the CI render, pixel-verified**

Download the CI `renders`/`stele-host` artifact; for each `pos-*.png`, **measure it programmatically** (PIL/connected-component: is the absolute box's ink at (30,20)? is the relative box shifted (15,10) with flow preserved? is the fixed box top-right? is the auto-inset box at the static origin?) and confirm it is CORRECT, not merely produced. Copy the verified PNG into `goldens/`, commit, push; CI byte-compares. **Never bless a render you haven't measured (AGENTS.md §4).**

- [ ] **Step 5: Containing-block verification (the spec's risk)**

Render the Acid2 dominant pattern (`relative` parent + `absolute` child, which `pos-absolute.html`/`pos-nested.html` already exercise). Confirm taffy places the absolute against the nearest positioned ancestor. **If a skip-level CB (an absolute whose CB is NOT its direct parent) misplaces**, record it in the report as a finding — a bespoke nearest-positioned-ancestor correction becomes a follow-up task (do NOT build it speculatively here). Journal the verification result.

---

### Task 4: Charter amendment + DECISIONS + JOURNAL

**Files:** `stele-charter.md`, `DECISIONS.md`, `JOURNAL.md`.

- [ ] **Step 1: Charter** — in "What Stele Speaks", add `position` (static/relative/absolute/fixed) + `top/right/bottom/left` as C2 dialect amendments (Acid2 Packet 1).

- [ ] **Step 2: DECISIONS** — prepend an entry (next free D-number; match house format): taffy-native positioning vs bespoke — choice: taffy-native, Acid2-sufficient; the containing-block risk (taffy positions absolutes vs the direct parent; CSS uses nearest positioned ancestor); revisit trigger (add a bespoke nearest-positioned-ancestor pass if Acid2 or a later packet needs a skip-level CB); `Fixed`≡absolute-to-viewport simplification (no scroll in the static render).

- [ ] **Step 3: JOURNAL** — append: Acid2 Packet 1 (positioning) landed via taffy plumbing; the CB verification result; the i486 size delta from CI; a note that the Acid2 render moves from a 3976 px flow toward placed boxes (partial — images/generated-content/stacking are P2–P6).

- [ ] **Step 4: Commit**

```bash
git add stele-charter.md DECISIONS.md JOURNAL.md
git commit -m "docs(css): C2 amendment + Dnn for positioning (Acid2 P1)"
```

---

## Self-Review

**1. Spec coverage:** §1 style+parse → Task 1; §2 taffy plumbing → Task 2; §3 paint order → covered implicitly (positioned boxes out-of-flow via taffy; verified in Task 3 goldens, explicit z-index is P2); §4 fixtures/tests → Tasks 1/2 (unit) + Task 3 (goldens); §5 charter → Task 4. CB-risk (§2) → Task 3 Step 5. ✓

**2. Placeholder scan:** No "TBD"/"handle errors". The `/* … same as margin */` markers are deliberate MIRROR instructions against a named, proven property (`margin`/`float`), not gaps — the implementer reads the existing code and copies its exact types/variants. The test snippets use illustrative field names with an explicit "match the real ones" instruction because the declared-style struct's exact field layout is read at implementation time (Bash was unavailable to pin it here) — this is a bounded, named lookup, not an open placeholder.

**3. Type consistency:** `Position { Static, Relative, Absolute, Fixed }` and `ComputedStyle.position`/`.inset: Edges<LengthPercentageAuto>` are consistent across Tasks 1/2/3. `base_style` maps Static/Relative→taffy `Relative`, Absolute/Fixed→taffy `Absolute` consistently. `inset` is stated as the same type as `margin` throughout.

**4. Golden-safety invariant:** restated in Global Constraints + Tasks 2/3 — a `static` document is byte-identical (default `static`→taffy `Relative`+auto-insets == current behavior); only `pos-*` fixtures add goldens; existing A1–A5 must not move.
