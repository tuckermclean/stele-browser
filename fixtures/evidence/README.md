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
