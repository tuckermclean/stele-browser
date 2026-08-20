# CSS positioning (Acid2 Packet 1) — design

**Date:** 2026-08-19
**Status:** approved design, pre-implementation
**Program:** Acid2 roadmap Packet 1 of 7 (`docs/superpowers/specs/2026-08-19-acid2-roadmap.md`) — the spine.

## Goal

Support CSS `position: relative | absolute | fixed` (+ `static`) with `top`/`right`/`bottom`/`left` offsets, **Acid2-sufficient**, by plumbing the properties through the existing style pipeline onto **taffy 0.13's native positioning** (`Style.position` + `Style.inset`) — reusing the proven layout substrate rather than building positioning from scratch. Add bespoke CSS 2.1 correction *only where Acid2 exposes a taffy gap* (YAGNI — the roadmap's "the test defines the dialect growth").

This is a **C2 charter amendment**: positioning is core document-web CSS (the polite web's native constituency), dialect *growth*, not the C3 uninvited-computation heresy.

## Non-negotiables this design serves

- **1.44 MB floppy = 1,474,560 bytes.** No new dependency (taffy is already a dep; positioning is plumbing + a little CSS parsing). Report the i486 delta from the CI `stele-i486` artifact; expected small.
- **Goldens byte-compared; pixel-verify before blessing.** Positioning micro-fixtures get PNG (+ tty where meaningful) goldens; measure each programmatically before blessing (AGENTS.md §4). Existing goldens must not move (this adds behavior only where `position` is set — a `static` document is unchanged).
- **Test-first, root-cause-first.** Each behavior lands with a failing test first.
- **No JavaScript / no uninvited computation (C3):** untouched — layout/style only.
- **CI compiles; we don't build i486 locally.**
- **Parsing is TOTAL:** unknown/malformed `position`/offset values degrade to the initial value (`static`/`auto`), never a panic.

## Current state (ground-truthed)

- **taffy is crates.io `0.13`** (`Cargo.toml`), already driving block flow (degenerate column flex), floats, grid, tables. It natively supports `Position::{Relative, Absolute}` and `Style.inset: Rect<LengthPercentageAuto>` (a repo diagnosis doc already references `taffy::Style::position = Position::Absolute`). **taffy has no `Static`/`Fixed`** — those are the engine's to map.
- **Property pipeline (the groove positioning follows):**
  - `src/style/value.rs` `apply_property(name, tokens, &mut ComputedStyle) -> bool` dispatches `display`/`float`/`clear`/… (a partial `"top"` arm already exists — reconcile with it).
  - `src/style/cascade.rs` cascades each property (own/inherited).
  - `src/style/computed.rs` `ComputedStyle` holds the fields (`display`, `float`, `clear`, `margin: Edges<LengthPercentageAuto>`, …); `Edges<T>` and `LengthPercentageAuto` already exist.
  - `src/style/ua.rs` supplies UA defaults.
  - `src/layout/block.rs` `base_style` already wires `ComputedStyle.float`/`.clear` onto taffy's `Style` — the injection point for `position`/`inset`.
- **taffy's containing-block behavior is the key risk** (see §Design step 2): taffy positions an `Absolute` child relative to its **parent**; CSS 2.1 uses the **nearest positioned ancestor**. Acid2's dominant pattern is `position:relative` parent wrapping `position:absolute` children (CB == direct parent) — which taffy handles — but any skip-level CB is where a bespoke correction may be needed.

## Design

### 1. Style + parse

- **`computed.rs`:** add to `ComputedStyle`:
  - `pub position: Position` — new enum `pub enum Position { Static, Relative, Absolute, Fixed }`, default `Static`, **non-inherited**.
  - `pub inset: Edges<LengthPercentageAuto>` — `top`/`right`/`bottom`/`left`, default `auto` each. (Reuse the existing `Edges`/`LengthPercentageAuto`, as `margin` does.)
- **`value.rs` `apply_property`:** add arms:
  - `"position"` → keyword → `Position` (`static`/`relative`/`absolute`/`fixed`; unknown → `Static`).
  - `"top"`/`"right"`/`"bottom"`/`"left"` → `LengthPercentageAuto` (length | percentage | `auto`) written into the matching `inset` edge. (Reconcile with the existing partial `"top"` handling.)
- **`cascade.rs`:** cascade `position` (own) and each `inset` edge (own).
- **`ua.rs`:** no change needed (default `Static`/`auto` is the initial value); confirm no UA rule needs positioning.

### 2. Taffy plumbing (`block.rs` `base_style`)

Map the computed values onto taffy's `Style`:
- `Position::Static` → taffy `Position::Relative` with **insets left `auto`** (in-flow, no offset — taffy's default).
- `Position::Relative` → taffy `Position::Relative` with `Style.inset` = the computed insets (offset from the in-flow position, box still occupies flow space — CSS relative).
- `Position::Absolute` → taffy `Position::Absolute` + `Style.inset` (out of flow; taffy places it against its containing block).
- `Position::Fixed` → taffy `Position::Absolute` + `Style.inset`, **anchored to the initial containing block (the viewport root)** — since there is no scrolling in the static render, fixed ≡ absolute-to-viewport. Implementation: ensure the fixed box's effective CB is the root container (see the CB note below).

**Containing block (the risk to verify, per Approach A):** in CSS, a non-`static` ancestor establishes the CB for its absolute descendants. In taffy, marking an ancestor `Position::Relative` (as CSS `relative`/`static`-with-CB-intent does) makes it a positioning context for its absolute children. The plan **verifies against Acid2's actual boxes** that taffy's placement matches; the common `relative`-parent/`absolute`-child pattern is expected to work natively. If Acid2 has an absolute whose CB is not its direct parent (skip-level), add a **minimal bespoke correction**: resolve the nearest positioned ancestor and offset the absolute fragment accordingly in a post-taffy pass (mirroring the existing float/margin pre/post-pass pattern in `block.rs`). Do not build this speculatively.

### 3. Paint order

Positioned boxes paint **after** in-flow siblings (CSS 2.1 default, no `z-index` yet — that's Packet 2). Ensure the `Fragment` list `emit` order places positioned fragments after in-flow content within their stacking level. If taffy's emit already yields document order and positioned boxes are out-of-flow, confirm they land after in-flow; add an emit-order tweak only if a fixture shows a wrong overlap.

### 4. Testing / fixtures

- **Unit (pure, CI):** `ComputedStyle`→taffy-`Style` mapping — `base_style` sets `Position::Absolute`/`Relative` and `inset` correctly for each `position` value; `apply_property` parses `position` + the four offsets (incl. `auto`, percentage, negative length) totally.
- **Golden micro-fixtures** (`fixtures/pos-*.html`, PNG goldens, pixel-verified):
  - `pos-absolute.html` — an absolute box at `top`/`left` offsets inside a `relative` parent (CB == parent).
  - `pos-relative.html` — a relative box offset from its flow position (siblings unaffected).
  - `pos-fixed.html` — a fixed box anchored to the viewport corner.
  - `pos-auto-inset.html` — absolute with `auto` insets (static position — stays at its in-flow origin).
  - `pos-nested.html` — absolute inside a relative inside static flow (CB resolution).
- Bless goldens from the CI `stele-host`/`renders` artifact after measuring them correct (no local i486 build).

### 5. Charter / decisions

- Amend `stele-charter.md` "What Stele Speaks": `position` (static/relative/absolute/fixed) + `top/right/bottom/left` enter the dialect (C2 amendment, Acid2 Packet 1).
- `DECISIONS.md`: entry — taffy-native positioning vs bespoke; the choice (taffy-native, Acid2-sufficient, bespoke CB correction only if needed); the containing-block risk + revisit trigger.

## Out of scope (YAGNI — other packets)

- `z-index` / stacking contexts / paint-order among overlapping positioned boxes → **Packet 2**.
- `min`/`max-width`/`height`, `overflow`, `background-position` → **Packet 5**.
- Generated content, `data:` URIs, `<object>` → Packets 3/4/6.
- `:hover`/dynamic behavior — never (static reference only).
- General CSS 2.1 positioning beyond what Acid2 exercises (percentage-inset edge cases, `direction`-dependent `right`/`bottom` static positions) unless Acid2 needs them.
