# Stele — v0.1 Release Report

**Stele** is a finished document-web browser for the 486: one static `i486`
musl binary that fetches, parses, styles, lays out, and renders the curated
document web — **with no JavaScript, by construction** (charter C3). This
report is the M6 release summary.

## Status

Milestones **M0–M5 complete; M6 hardening substantially complete.** 30 packet
PRs, each test-first (visible red→green in history), independently reviewed,
CI-green (in-image i486 cross-build + `qemu-i386 -cpu 486` execution), and
merged one-per-PR. 33 recorded fork decisions (`DECISIONS.md`), full narrative
in `JOURNAL.md`.

| Milestone | | |
|---|---|---|
| M0 | Toolchain: pinned nightly, `i486-monolith-linux-musl` target, qemu-486 | ✅ |
| M1 | Fetch + parse + DOM + `--dump-text` | ✅ |
| M2 | Style + block/inline layout + tty backend | ✅ |
| M3 | Tables (colspan/rowspan) + frames + forms | ✅ |
| M4 | fb backend + images + floats — **the screenshot** | ✅ |
| M5 | Dialect completeness (author CSS, flexbox, @media, details, noscript, entities, --stats) | ✅ |
| M6 | Hardening (fuzz, size gate, kitchen-sink, list markers) | ◑ core done; attestation ceremony pending |

## What Stele does

- **Fetch** — bespoke HTTP/1.1 over `std::net` (status line, folded headers,
  Content-Length + chunked bodies, redirects ≤5, cookie jar), `file://` for
  local docs. **No TLS** (charter: the proxy's job). Total on malformed
  responses.
- **Parse** — bespoke 1996-grade tag-soup HTML: full HTML 4.01 named + numeric
  (decimal & hex) entities, void/raw-text/implied-close/mis-nesting recovery,
  total (never panics; explicit-stack, no unbounded recursion).
- **Style** — bespoke CSS tokenizer + selectors + cascade with real origin &
  specificity ordering. Author `<style>` blocks **and** inline `style=` apply
  (inline is highest origin); **`@media`** width queries evaluated against the
  render viewport; the curated §4 property set (block/inline/**flexbox**/float/
  clear/tables/text/color/border/list-style/…). Unknown declarations are
  ignored **and counted** (`--stats`, charter C2).
- **Layout** — solvers over a flexbox substrate (**taffy**): block flow, a
  bespoke **inline engine** (line breaking, inline replaced atoms, `<br>`),
  **flexbox** (justify/align/grow/gap), a bespoke **table** auto-layout
  (two-pass min/max-content column solver + colspan/rowspan), **`img
  align=left` floats** with text wrap, **framesets** (recursive nested
  viewports), list-item markers, `<details>` disclosure. Total on hostile
  input: every recursive walk is depth-capped, every allocation bounded.
- **Images** — PNG / JPEG (baseline + progressive) / GIF / **animated GIF**
  decoders behind one trait, decompression-bomb-capped, wired into `<img>` and
  blitted (nearest-neighbor scale + alpha).
- **Render backends** — a **tty** text grid (`--dump-text`), a **pixel**
  raster to PNG (`--dump-png`, via an embedded bitmap font), and a real
  **Linux framebuffer** (`--render-fb`, sysfs geometry + `/dev/fb0`, no
  `unsafe`).

## The screenshots (`goldens/`)

- **`basic.png`** — a heading/paragraph/link document: the first pixels.
- **`images.png`** — THE SCREENSHOT: PNG, JPEG, GIF, and animated-GIF-frame-0
  all decoded and blitted.
- **`flex-polite.png`** — a modern no-JS blog layout in real CSS flexbox
  (`justify-content: space-between` header, `flex-grow` + fixed sidebar) —
  "modern flex on a 486," the charter's earned bonus.
- **`kitchen-sink.png`** — the everything-page (A5 coverage): inline markup,
  lists, tables, forms, inline + floated images, flexbox, details/noscript,
  `<br>`, entities — every feature rendering correctly in combination.

## Acceptance (accept.sh)

- **A1** static i386-class ELF — **PASS**.
- **A2** size ≤ 2.0 MB stripped — **PASS**, i486 binary ≈ **542 KB** (huge
  headroom); now a hard gate.
- **A3** fixture golden renders (tty + PNG, blessed under §10 discipline) —
  **PASS** across basic/tables/forms/frames/images/flex-polite/media/details/
  noscript/entities/lists/kitchen-sink.
- **A4** runs under `qemu-i386 -cpu 486` vs golden — **PASS** (executes
  486-legal code).
- **A5** kitchen-sink coverage golden — **PASS**. *(Note: the brief also
  defines an A5 instruction-speed budget (<50M instrs / <150ms); the speed
  instrumentation is a pending M6 item — see Deferred.)*
- **A6** covenant grep (no `script` variant / no JS engine anywhere) —
  **CLEAN**.
- **A7** attestation (vendored deps + `cargo-auditable` + `cargo-audit`) —
  **PENDING** (see Deferred: the audit tooling is not in the pinned build
  image).

## Robustness

- **Fuzz** — a hand-rolled deterministic mutation harness
  (`tests/fuzz_totality.rs`, ~4400 iterations over HTML byte-mutation, random
  blobs, random CSS, and image bytes) drives the whole pipeline asserting no
  panic/abort. `panic="abort"` makes any find a hard failure. It surfaced
  exactly one bug (`<br>` no-op, fixed) and nothing else.
- **Totality by construction** — every hostile-input axis is bounded and was
  caught by review before merge: unbounded recursion (parser, cascade, layout,
  box-tree, frames), decompression bombs (images), the O(n²)/O(n·k) DoS class
  (table cells, wide tables, glyph paint, frame trees, image memory), and CLI
  allocation (`--cols`, fb geometry). No `unsafe` anywhere in the codebase.

## Dependencies & licenses (charter C8)

| Crate | Role | License |
|---|---|---|
| `taffy` (0.13, trimmed features) | flexbox/block box-math | MIT |
| `png` (0.17) | PNG decode + encode | MIT / Apache-2.0 |
| `jpeg-decoder` (0.3, no rayon) | JPEG decode | MIT / Apache-2.0 |
| `gif` (0.13) | GIF decode | MIT / Apache-2.0 |
| **font8x8_basic** (embedded atlas, `src/text/glyphs.rs`) | bitmap font | **Public Domain** (github.com/dhepper/font8x8) |
| **font8x8_ext_latin** (Latin-1 supplement, `src/text/glyphs.rs`, packet t2-glyph-fallback) | bitmap font | **Public Domain** (github.com/dhepper/font8x8) |

All else is `std`. HTTP, CSS, HTML parsing, the inline/table/float/frame
layout, and the fb backend are bespoke and in-house.

## Deferred, honestly

- **A7 attestation ceremony** — `cargo vendor` + `cargo-auditable` (embed the
  dep manifest) + `cargo-audit`. The audit tools are **not in the pinned
  monolith-builder image** (M0 substrate finding, D6); wiring A7 needs those
  tools added to the image. **Operator action.**
- **A5 instruction-speed budget** — the <50M-instructions / <150ms gate needs
  `qemu` instruction-count instrumentation; not yet wired.
- **Interactive tty shell (P7b)** — raw-mode scroll / follow-links / back.
  Rendering is complete; the interactive driver is not. Link-following needs a
  small `Fragment`/`LayoutNode` freeze amendment to carry `href` provenance.
- **External `<link rel=stylesheet>` CSS** — needs a fetch pre-pass like
  images; inline `<style>` + `style=` work today.
- **Cookie-jar file persistence** (charter C6) — the jar exists and is wired
  into HTTP; cross-invocation plain-file persistence is not wired.
- **Rendering fidelity nits** — `<pre>` whitespace preserved (currently
  collapsed); `&nbsp;` as a non-collapsing space (currently collapses);
  CMYK/16-bit JPEG and APNG (Unsupported, fall back to alt). Unicode glyphs
  beyond ASCII: **narrowed** by packet t2-glyph-fallback — the atlas now
  covers Latin-1 (`U+00A0..=U+00FF`) directly, and General-Punctuation
  characters a real page actually leans on (em/en dash, curly quotes,
  ellipsis, bullet, `×`, `→` — see `text::translit`) are transliterated to
  plain ASCII at render time instead of showing tofu. Anything still outside
  BOTH (CJK, emoji, ...) is now dropped and counted (`--stats`'s
  "N missing glyphs"), not tofu'd — see `text::translit`'s own module doc.
- **Stretch (M7, untouched by design)** — Lua chair, X11 backend, Transcript
  pane, no-libc spike.

## Build & run

```
# host-native (quickest to try)
cargo build --release
./target/release/stele --headless --dump-text fixtures/kitchen-sink.html   # text
./target/release/stele --headless --dump-png  fixtures/flex-polite.html out.png   # pixels
./target/release/stele --headless --dump-png  fixtures/flex-polite.html out.png --stats  # + refused-declaration counts on stderr

# the real i486 binary (CI artifact stele-i486), on a Linux host with qemu-user:
qemu-i386 -cpu 486 ./stele --headless --dump-png fixtures/images.html shot.png

# real framebuffer (Linux VT with fbcon, or qemu-system -vga std):
stele --headless --render-fb fixtures/basic.html
```

CLI: `stele [--headless] [--dump-text <src> [--cols N]] [--dump-png <src>
<out.png>] [--render-fb <src>] [--stats]`. `<src>` is a path, `file://`, or
`http://`. No args → the M0 hello.

## License

Stele is free software under **GPL-3.0-or-later** — Copyright (C) 2026 Tucker
McLean (see `LICENSE`). Use, study, share, and modify it freely; but incorporate
it into a distributed product and that product must also be GPL-licensed with
its source available — no closed-source pilfering. Bundled `font8x8` is public
domain; `taffy`/`png`/`jpeg-decoder`/`gif` are MIT/Apache-2.0 (both compatible
with GPLv3), so the combined work is validly GPL-3.0.

---
*Fonts: font8x8_basic (Public Domain). Bespoke soul (in-house): the parser, the
CSS engine, the inline/table/float/frame layout, the tty & fb backends. The
rock does not unwind.*
