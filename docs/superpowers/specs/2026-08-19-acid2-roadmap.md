# Acid2 conformance — program roadmap

**Date:** 2026-08-19
**Status:** north-star for a multi-packet program (7 packets, each its own spec → plan → PR)
**Goal:** render the Acid2 test (`http://acid2.acidtests.org/`) as the reference smiley — pixel-conformant — by growing the curated dialect toward CSS2.1, one shippable packet at a time.

This is a *roadmap*, not a single spec. Each packet below gets its own `docs/superpowers/specs/…-design.md` + implementation plan + PR, and each is a **C2 charter amendment** (the dialect is curated, not frozen — defined by principle). These are all legitimate *document*-web CSS (positioning, generated content, box constraints) — dialect **growth**, not the uninvited-computation heresy C3 forbids. `:hover` (the dynamic nose) is explicitly out of scope: Acid2's *static* reference is the target.

## Baseline (spike, 2026-08-19)

The current engine renders Acid2 as an **800×3976 px vertical text-flow**, not the ~150 px smiley (baseline PNG captured via the CI `renders/` gallery; the throwaway `spike/acid2` branch was deleted, `fixtures/acid2.html` returns in Packet 7 with its golden). Root cause, confirmed by the render *and* the source: **positioning is the spine of Acid2 and is entirely unsupported** — every `position:absolute/relative/fixed` box collapses into normal flow and stacks vertically (hence 3976 px where ~150 px is correct). Only the fallback prose ("Take The Acid2 Test", "Hello World!") and an `<hr>` render, all mislaid in flow.

**The ordering consequence:** the self-contained packets (`data:` URIs, `min`/`max`) compile and test in isolation but move the Acid2 render *not at all* until boxes can be *placed*. Positioning must come first.

## Gap analysis (evidence — feature usage counted in the real test)

| Feature | Engine today | Acid2 usage | Packet |
|---|---|---|---|
| Box model, `display` (block/inline/flex/grid/table), **float**, tables, `line-height` | ✅ have | float ×5, display:table ×4 | — |
| **PNG alpha** (straight-alpha RGBA via the `png` crate) | ✅ have | every face PNG is transparent | — |
| `background-color` / `background-image` | ✅ have | background ×30 (some need positioning) | see P5 note |
| **`position` (relative/absolute/fixed) + `top/left/right/bottom`** | ❌ absent | absolute ×3, relative ×3, fixed ×1, offsets ×14 | **P1** |
| **stacking / `z-index` / paint order** | ❌ absent | z-index ×2, overlapping layers | **P2** |
| **generated content** (`:before`/`:after` + `content`) | ❌ absent | `:before`/`:after` ×4, `content:` ×2 | **P3** |
| **`data:` URI scheme** | ❌ absent (file/http/https only) | `url(data:…)` ×4 | **P4** |
| **`min`/`max-width`/`height`** | ❌ absent (only `width`/`height`) | ×7 | **P5** |
| **`overflow: hidden`** (clipping) | ❌ absent | ×3 | **P5** |
| **`background-position`** | ❌ likely absent | part of the 30 background refs | **P5** |
| **`<object>` + nested fallback** | ❌ absent | ×3 (the famous fallback test) | **P6** |

## The packet program (dependency-ordered)

Each packet: `packet/<name>` off `main`, test-first, CI-driven, golden-verified, one PR, its own C2 amendment recorded in `DECISIONS.md` + the charter's "What Stele Speaks".

### P1 — CSS positioning (the spine) ← START HERE
`position: static|relative|absolute|fixed`; `top`/`right`/`bottom`/`left` offsets; containing-block resolution (nearest positioned ancestor for `absolute`; viewport for `fixed`; padding box rules); out-of-flow removal for absolute/fixed; relative offset without reflow. The largest and highest-risk packet — front-loaded. Fixtures: a handful of positioning micro-tests with goldens.

### P2 — Stacking contexts, `z-index`, paint order
Establish stacking contexts (positioned + `z-index`), paint in CSS 2.1 Appendix-E order, honor `z-index` among siblings. Pairs tightly with P1 (the face layers overlap and must composite in the right order). Fixtures: overlapping positioned boxes with z-index goldens.

### P3 — Generated content (`:before` / `:after`)
The `content` property (strings, and `content: url(...)` images); synthesize pseudo-element boxes in the style→box-tree pipeline; they participate in layout/paint like real boxes. Fixtures: `:before`/`:after` string + image goldens.

### P4 — `data:` URI scheme
A new fetch path (no socket): parse `data:[<mediatype>][;base64],<data>` → decode (base64 / percent) → bytes + content-type, feeding the existing image decoders. Self-contained; slots into the `fetch::fetch` scheme table. Fixtures: an `<img src="data:image/png;base64,…">` golden.

### P5 — `min`/`max-width`/`height`, `overflow: hidden`, `background-position`
Box-constraint clamping (min/max applied to the used width/height), `overflow: hidden` clipping (a clip rect through layout→paint), and `background-position` (offset the background paint). Grouped: each is small and box-model-local. Fixtures: min/max clamp, overflow clip, bg-position goldens.

### P6 — `<object>` with nested fallback
Parse `<object>` and its fallback cascade (render the first resolvable representation, else the nested fallback content) — Acid2's signature fallback mechanism. Fixtures: object→fallback goldens.

### P7 — Acid2 assembly + the KILL test
Re-add `fixtures/acid2.html`; produce the reference-smiley golden (pixel-verified — never rubber-stamped); wire it into `accept.sh` A5; a side-by-side vs the WaSP reference, journaled per the "world proves the hardware" doctrine (and, once passing, a screenshot for the README).

## Verification model

- **Per packet:** micro-fixtures with PNG/tty goldens (byte-compared in CI), unit-tested pure layout/paint helpers, and the discipline of measuring each golden programmatically before blessing (AGENTS.md §4 — never rubber-stamp).
- **The whole:** Packet 7's `acid2.html` golden is the conformance gate. Track progress by re-rendering the (scratch) Acid2 fixture at each packet's end and journaling how much closer the face gets.
- **Size:** each packet reports its i486 delta against the 1.44 MB floppy — Acid2 support is CSS/layout code (no new deps expected beyond what's here), but measure.

## Charter / governance

Each packet expands the curated dialect (C2) with a recorded amendment. Positioning and generated content are core document-web CSS — the "polite web's native constituency." The one deliberate exclusion is dynamic `:hover` (C3-adjacent app behavior); Acid2's static reference render is the target, and the reference smiley does not depend on hover.

## Not in scope

- `:hover`/interactivity (the dynamic nose) — static reference only.
- Any CSS beyond what Acid2 exercises (YAGNI — the test defines the dialect growth).
- The throwaway spike's `acid2.html` in `fixtures/` before P7 (it renders a useless 3976 px flow until P1–P6 land; it returns in P7 with its golden).
