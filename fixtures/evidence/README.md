# fixtures/evidence

Pixel-reference material used to certify Stele's rendering against an
external, independently-authored ground truth, rather than only against
Stele's own checked-in goldens (which prove *stability*, not *correctness*).

## css1-float-5526c

- **Source**: W3C CSS1 test suite, "display/box/float/clear test"
  (`sec5526c.htm`'s child fixture `test5526c.htm`, CSS1 §5.5.26 conformance
  test — https://www.w3.org/Style/CSS/Test/CSS1/current/sec5526c.htm).
- **Fixture**: `fixtures/css1-float-5526c.html` — an exact byte-for-byte
  copy of the W3C's `test5526c.htm` (same HTML, same inline `<style>`
  block). Not reformatted, not re-encoded: any diff against the upstream
  file would undermine its value as an independent reference.
- **Reference rendering**: `css1-float-5526c.reference.gif` — an exact
  byte-for-byte copy of the W3C's own `sec5526c.gif`, the pixel rendering
  the test's own body text calls out as ground truth ("...indistinguishably
  (to the pixel) from this reference rendering..."). This is a
  Chrome/conformant-UA rendering of the fixture above, not anything Stele
  produced.
- **What it tests**: a "Mondrian grid" built entirely from `float: left` /
  `float: right` block-level elements (`<dt>`/`<dd>` with percentage and
  `em` widths, `<li>` cards, a floated `<blockquote>` and `<h1>`) plus a
  final `clear: both` paragraph. It is a floor-level layout-fidelity target:
  a 100%-conformant CSS1 engine renders it pixel-identical to the reference
  GIF (modulo font rasterization and form-widget chrome, which the test's
  own text excuses).
- **Status**: fixture + reference are landed here so progress toward
  conformance is trackable over time, but **no golden has been blessed for
  it yet** — Stele's current float layout is known-incomplete (see
  `css1-float-5526c.diagnosis.md` in this directory), so its render is a
  vertical stack, not the reference Mondrian grid. Blessing a golden now
  would just freeze the wrong picture; the diagnosis doc has the concrete,
  staged plan for closing the gap.

## httpforever

- **Source**: https://httpforever.com/ (Scott Helme's real-world, no-JS-
  degradable marketing page for the "HTTP Forever" project).
- **Fixture**: `fixtures/httpforever.html` — the page's raw markup with its
  external `<link rel=stylesheet>` CSS INLINED into a single `<style>` block
  (see `httpforever.style.css` below for the untouched source of that CSS),
  so the fixture renders its full intended styling from a single `file://`
  fetch with no network dependency at all. This is Stele's **canonical
  dark-theme fidelity fixture**: `packet/t1a-var`, `packet/t1b-color-scheme`,
  and `packet/t1c-contrast` (CSS custom properties, `--color-scheme`-driven
  theme selection, and the contrast-repair covenant) all exist because this
  page's real-world CSS leans on every one of them at once.
- **Evidence**: `httpforever.raw.html` — an exact byte-for-byte copy of the
  page's original, un-inlined markup (still referencing the external
  `style.css`), kept as the untouched source-of-truth the inlined fixture
  was derived from. `httpforever.style.css` — the page's own external
  stylesheet, byte-for-byte, also untouched. `httpforever.chrome-reference.
  pdf` — a Chrome-rendered "print to PDF" capture of the live page, the
  external ground-truth reference this fixture's fidelity is judged against
  (same role `css1-float-5526c.reference.gif` plays above, just a
  full-page capture instead of a single conformance-test GIF).
- **Theme mechanism**: httpforever.com's dark mode is **JS-only** in its own
  right — reached in the browser via a `js/theme.js` toggle script that
  flips `html[data-theme="dark"]`, with **no** `@media (prefers-color-
  scheme: dark)` fallback anywhere in its CSS at all (grep `httpforever.
  style.css` for `data-theme` vs `prefers-color-scheme` to confirm). Since
  Stele runs no JavaScript by construction (charter C3), the ONLY way to
  reach this page's dark theme through Stele is packet t1b-color-scheme's
  `--color-scheme dark` flag, which stamps `data-theme="dark"` onto the
  root `<html>` pre-cascade — the same no-JS approximation `stamp_color_
  scheme`'s own doc comment describes. `goldens/httpforever.light.png` is
  the undecorated default render; `goldens/httpforever.dark.png` is
  `--headless --color-scheme dark --dump-png`.
- **Status**: goldens are blessed for both themes as of packet
  t1d-httpforever, and `--audit-contrast` reports zero violations against
  this fixture in BOTH themes — proving the T1a/T1b/T1c contrast covenant
  actually holds on a real, unmodified, dense real-world page, not just the
  synthetic fixtures the earlier packets shipped their own tests against.
  Per that packet's own scope note: em-dashes are still tofu (pending T2),
  the footer's link list is still jammed together (pending T3), and any
  button-shaped controls may still read as literal `[ Submit ]`-style
  fallback text (pending T4) — this packet's bar is legibility (backgrounds
  paint, the hero text is never invisible-on-its-background, zero contrast
  violations), not full-dialect fidelity. These goldens are expected to be
  re-blessed as those later packets land.
