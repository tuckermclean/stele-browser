# Stele — v0.1 Release Report

**Stele** is a finished document-web browser for the 486: one static `i486`
musl binary that fetches, parses, styles, lays out, and renders the curated
document web — **with no JavaScript, by construction** (charter C3). This
report is the M6 release summary.

## Status

Milestones **M0–M5 complete; M6 substantially complete** — where "M6" means the
*build brief's* M6 (Hardening + acceptance: fuzz, size gate, A1–A7;
`stele-build-brief.md:245`), **not** the *charter's* M6 (Transcript + Provenance
keys; `stele-charter.md:228`), which has **not been started**. The charter and the
build brief number their milestones differently — this report follows the build
brief throughout, so "M6 substantially complete" is a hardening claim, not a
transcript/provenance claim. Every packet PR is test-first (visible red→green in
history), independently reviewed, CI-green (in-image i486 cross-build +
`qemu-i386 -cpu 486` execution), and merged one-per-PR. Fork decisions are recorded
in `DECISIONS.md`; the full narrative is in `JOURNAL.md`.

| Milestone | | |
|---|---|---|
| M0 | Toolchain: pinned nightly, `i486-monolith-linux-musl` target, qemu-486 | ✅ |
| M1 | Fetch + parse + DOM + `--dump-text` | ✅ |
| M2 | Style + block/inline layout + tty backend | ✅ |
| M3 | Tables (colspan/rowspan) + frames + forms | ✅ |
| M4 | fb backend + images + floats — **the screenshot** | ✅ |
| M5 | Dialect completeness (author CSS, flexbox, @media, details, noscript, entities, --stats) | ✅ |
| M6 | Hardening (fuzz, size gate, kitchen-sink, list markers) | ◑ core done; attestation ceremony pending |
| M7 (build-brief stretch) | X11 backend — self-drawn chrome, editable address bar, back/forward, GUI-vs-TTY auto-select (`src/main.rs::run_x11`) | ◑ X11 shipped; Lua chair / Transcript pane / no-libc spike untouched |
| Milestone A (Acid2) | Acid2 face **composes** in an 800×600 viewport (`--scroll-to` + fixed viewport, D64) — **not** a WaSP byte-match; fills/geometry deferred | ✅ composes (not byte-exact) |

*Milestone numbers above follow `stele-build-brief.md`, not the charter (see
Status). "Milestone A" is the Acid2 program (`DECISIONS.md` D64,
`JOURNAL.md:1373`–`1577`), tracked separately from the M-series.*

## What Stele does

- **Fetch** — bespoke HTTP/1.1 over `std::net` (status line, folded headers,
  Content-Length + chunked bodies, redirects ≤5, cookie jar), `file://` for
  local docs. **HTTPS** is served by delegating to the user's own
  `openssl s_client` — zero cryptography in the binary (charter C2 / D14,
  `src/fetch/https.rs`). Total on malformed responses.
- **Parse** — bespoke 1996-grade tag-soup HTML: full HTML 4.01 named + numeric
  (decimal & hex) entities, void/raw-text/implied-close/mis-nesting recovery,
  total (never panics; explicit-stack, no unbounded recursion).
- **Style** — bespoke CSS tokenizer + selectors + cascade with real origin &
  specificity ordering. Author `<style>` blocks, external
  `<link rel=stylesheet>` (fetched in a pre-pass, `src/stylesheets.rs`), **and**
  inline `style=` apply (inline is highest origin); **`@media`** width queries evaluated against the
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
  raster to PNG (`--dump-png`, via an embedded bitmap font), a real
  **Linux framebuffer** (`--render-fb`, sysfs geometry + `/dev/fb0`, no
  `unsafe`), and an **X11** interactive shell (`--x11`, `src/main.rs::run_x11`)
  with self-drawn chrome — back/forward/reload, editable address bar, throbber,
  no GUI toolkit (`src/backend/chrome.rs`, `src/backend/address_edit.rs`). A bare
  `stele <src>` auto-selects the X11 GUI shell when a display is present
  (`graphical_display_available`) and the interactive **tty** shell otherwise
  (`src/main.rs::run_browser`: raw-mode scroll / follow-links / back, backed by
  `src/browser.rs`).

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
- **A2** size ≤ 2.0 MB stripped — **PASS** (now a hard gate). The last
  CI-measured `stele-i486` binary is **1,377,436 bytes** (`JOURNAL.md:1605`,
  2026-08-20 Terminus packet; D66) — **93.4 % of the 1.44 MB floppy**
  (1,474,560 B), leaving only **97,124 B** of headroom. Budget future work
  against the floppy ceiling, not the looser 2.0 MB A2 gate. (Packets since —
  attestation modal, `view-source:` — were docs-/small and not re-measured; see
  `JOURNAL.md:1633`, `:1652`.)
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
| **Terminus (191-glyph subset)** (embedded atlas, `src/text/terminus_glyphs.rs`, packet #93) | bitmap font | **OFL-1.1** (terminus-font.sourceforge.net; see `third_party/terminus-font/NOTICE.md`) |

All else is `std`. HTTP, CSS, HTML parsing, the inline/table/float/frame
layout, and the fb backend are bespoke and in-house.

The full runtime dependency roster — every crate above, plus their own
transitive deps, with real per-crate SPDX ids and license text (not just this
hand-maintained summary table) — is generated straight from `cargo metadata`
by `tools/gen-attestations.py` into `src/fetch/attestations_data.rs`, and
rendered in-binary at `about:attestations` (packet/attestation-modal). Keeping
this table and that generated roster in sync is a manual step today, not
automated — flagged, not silently deferred.

## Deferred, honestly

- **A7 attestation ceremony** — `cargo vendor` + `cargo-auditable` (embed the
  dep manifest) + `cargo-audit`. The audit tools are **not in the pinned
  monolith-builder image** (M0 substrate finding, D6); wiring A7 needs those
  tools added to the image. **Operator action.**
- **A5 instruction-speed budget** — the <50M-instructions / <150ms gate needs
  `qemu` instruction-count instrumentation; not yet wired.
- **Cookie-jar file persistence** (charter C6) — the jar exists and is wired
  into HTTP; cross-invocation plain-file persistence is not wired.
- **Rendering fidelity nits** — `&nbsp;` as a non-collapsing space (currently
  collapses);
  CMYK/16-bit JPEG and APNG (Unsupported, fall back to alt). Unicode glyphs
  beyond ASCII: **narrowed** by packet t2-glyph-fallback — the atlas now
  covers Latin-1 (`U+00A0..=U+00FF`) directly, and General-Punctuation
  characters a real page actually leans on (em/en dash, curly quotes,
  ellipsis, bullet, `×`, `→` — see `text::translit`) are transliterated to
  plain ASCII at render time instead of showing tofu. Anything still outside
  BOTH (CJK, emoji, ...) is now dropped and counted (`--stats`'s
  "N missing glyphs"), not tofu'd — see `text::translit`'s own module doc.
- **Stretch (build-brief M7)** — the **X11 backend shipped** (see Render
  backends / the milestone table); still untouched: Lua chair, Transcript pane,
  no-libc spike. *(Note: `<pre>` / `white-space:pre` and the interactive tty and
  X11 shells, previously listed here as deferred, have all shipped — see What
  Stele does. The charter's own M6 — Transcript + Provenance — is the remaining
  large piece; it has not been started, see Status.)*

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
its source available — no closed-source pilfering. The bundled Terminus subset
is OFL-1.1; `taffy`/`png`/`jpeg-decoder`/`gif` and their transitive deps are
MIT/Apache-2.0/Zlib (all compatible with GPLv3), so the combined work is
validly GPL-3.0.

This same information — Stele's own license, the full generated dependency
roster, and Terminus's complete OFL-1.1 text — ships user-facing and in-binary
at `about:attestations` (packet/attestation-modal): a self-served page, no
network fetch, reachable from the `--x11` chrome's attestations affordance.

---
*Fonts: Terminus (191-glyph subset, OFL-1.1, packet #93 — see
`third_party/terminus-font/NOTICE.md`). Bespoke soul (in-house): the parser,
the CSS engine, the inline/table/float/frame layout, the tty & fb backends.
The rock does not unwind.*
