# Stele

A **finished document-web browser for the 486**. One static `i486` musl binary that
fetches, parses, styles, lays out, and renders the document web — **with no
JavaScript, by construction.** Pronounced *STEE-lee*: a stele is a stone slab whose
entire purpose is bearing inscriptions.

Stele is deliberately *finished* software — a curated dialect, a feature set frozen by
principle, and a binary that must fit on a **1.44 MB floppy**. Its thesis, shipped as
code: Netscape died of its own bloat. A browser that refuses to become an operating
system can be small, fast, and done — fast enough to beat Navigator 4 to first paint on
a 486DX2/66 with 16 MB of RAM.

## What it renders

- **HTML 4.01 Transitional** tag soup + HTML5 semantic/sectioning elements
- **CSS1 + flexbox + CSS Grid + block floats**, custom properties, one-shot media
  queries, and the real box model (content-box default, per-side borders, margin
  collapsing, `box-sizing`)
- **Tables** (colspan/rowspan/nested), **frames/framesets**, **forms** (GET/POST, all
  input types)
- **Images**: GIF (incl. animated), JPEG (baseline + progressive), PNG
- **Cookies** (Netscape jar format — a text file you own), HTTP/1.1 + gzip
- Backends: **tty**, **framebuffer**, **X11**, plus a **headless** mode that dumps text
  or PNG (this is what the golden tests render)

## What it will never do

No JavaScript, no Java, no plugins, no updater, no telemetry. Not declined —
**unrepresentable**: the document AST is a closed sum type with no variant for
executable anything (charter C3). A page that needs an application runtime renders its
content, inert, with a status line noting what was refused. Refusal is always legible.

The full contract — the kill conditions, the covenant (C1–C11), and exactly "What Stele
Speaks" — is in **[`stele-charter.md`](stele-charter.md)**. Read it before arguing about
scope; principles live there.

## Build

Stele builds **inside the `monolith-builder` image** (pinned by digest), which carries
the one pinned nightly ([`rust-toolchain.toml`](rust-toolchain.toml)), the i486
cross-musl toolchain, and `rust-src` for `-Zbuild-std`.

The **canonical build command is documented and kept current at the top of
[`accept.sh`](accept.sh)** — read it there rather than trusting a copy that can drift.
In outline, the i486 release target is:

```sh
cargo build --release \
  --target targets/i486-monolith-linux-musl.json \
  -Zbuild-std=std,panic_abort -Zjson-target-spec
```

The host build (for tests and headless golden dumps) is a plain `cargo build --release`
(no `+toolchain` override — that would bypass the pin).

> **On modest hardware:** the i486 cross-build under `-Zbuild-std` is heavy. The project
> is set up so **CI does the building** — push a branch and let the `m0-acceptance`
> workflow compile, test, and run acceptance. See **[AGENTS.md](AGENTS.md)** for the
> CI-driven loop (including how to bless goldens without building locally).

## Run

```sh
stele --headless --dump-text <file-or-url>   # text render (the tty golden form)
stele --headless --dump-png  <file-or-url>   # pixel render → PNG
stele <url>                                  # interactive (tty / framebuffer / X11)
```

See `stele --help` / [`src/main.rs`](src/main.rs) for the current flag set.

## Acceptance is the definition of done

`./accept.sh` **is** Stele's definition of DONE — exit 0 means acceptance. It runs
checks **A1–A5**: statically-linked i386 ELF (A1), size budget (A2), fixture golden
renders — tty + PNG (A3/A5), and the i486 binary executing under `qemu-i386 -cpu 486`
(A4). `--tty-only` runs just the host golden check; `--bless` regenerates goldens
(**never bless a render you haven't verified** — see [AGENTS.md](AGENTS.md)).

**Two size numbers — know both:**

| Number | Bytes | Meaning |
|---|---|---|
| A2 gate | 2,000,000 | CI-enforced hard fail |
| The floppy | 1,474,560 | The **real** release ceiling — tighter than A2 |

Weigh every dependency, taffy feature, and decoder against the floppy, not just A2.

## Repository map

| Path | What lives there |
|---|---|
| `src/dom/` | HTML parser + the closed document AST (no script variant — C3) |
| `src/style/` | CSS: tokenizer, parser, selector, cascade, computed values, media, contrast |
| `src/layout/` | box tree, block/flex/grid/float (taffy 0.13 wiring), inline, tables |
| `src/text/` | 8×8 bitmap glyph atlas, metrics, transliteration fallback, rasterizer |
| `src/backend/` | tty, framebuffer, X11 render backends |
| `src/fetch/`, `src/img/` | HTTP/1.1 + file + cookies; GIF / JPEG / PNG decoders |
| `fixtures/` + `goldens/` | test documents + their byte-compared reference outputs |
| `tests/` | host integration tests |
| `targets/`, `accept.sh`, `.github/workflows/` | i486 target spec, acceptance harness, CI |

## The living documentation

These evolve with the project. Treat them as the source of truth over anything
summarized above:

- **[`stele-charter.md`](stele-charter.md)** — the constitution: kill conditions,
  covenant, the dialect. **Principles win here.**
- **[`stele-build-brief.md`](stele-build-brief.md)** — process & milestones (charter
  wins on principle, brief wins on process).
- **[`DECISIONS.md`](DECISIONS.md)** — every non-obvious fork: options, choice, why,
  revisit-trigger. Newest first.
- **[`JOURNAL.md`](JOURNAL.md)** — append-only narrative log.
- **[`REPORT.md`](REPORT.md)** — release summary.
- **[`AGENTS.md`](AGENTS.md)** — how to actually work in this repo (humans and AI agents
  alike). Start here if you're going to change anything.

## License

GPL-3.0-or-later. See [`LICENSE`](LICENSE).
