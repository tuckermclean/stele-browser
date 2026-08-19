# STELE — AUTONOMOUS BUILD BRIEF
# Drop this file into Claude Code and go. The operator will return after a
# long absence and expects: a working browser, a green acceptance script,
# and a readable account of every decision made in their absence.
# Companion document: stele-charter.md (the constitution). Where this brief
# and the charter conflict, the charter wins on principles, this brief wins
# on process.

---

## 0. MISSION & DEFINITION OF DONE

Build Stele v0.1: a document-web browser per the charter — Rust, one
static binary, tty + framebuffer backends, curated dialect (HTML 4.01 +
HTML5 semantic vocabulary, CSS1 + flexbox), images (GIF incl. animated,
JPEG incl. progressive, PNG), forms, tables, frames, cookies, headless
mode. No JavaScript engine exists in the program (charter C3: the AST has
no script variant).

DONE means `./accept.sh` exits 0, where accept.sh (you write it first,
Milestone 0) verifies at minimum:
  A1. `file target/i486/release/stele` → statically linked, i386-class.
  A2. Binary size ≤ 2.0 MB stripped (budget; record actual in REPORT.md).
  A3. Fixture corpus renders match checked-in golden images (fb backend
      into an in-memory surface — no display needed) and golden text
      dumps (tty backend). Zero diffs.
  A4. The i486 binary runs under qemu-i386 (user-mode emulation):
      `qemu-i386 stele --headless --dump-text file:fixtures/basic.html`
      produces the golden text output. (This proves the actual target
      binary executes i486-legal code, not just that host tests pass.)
  A5. Speed budget: headless first-paint pipeline (fetch→parse→style→
      layout→first render) over fixtures/kitchen-sink.html completes in
      < 50M retired instructions under `qemu-i386` with plugin/insn
      count if available, else < 150 ms host wall-clock — record both
      the metric used and the number. (Real-486 stopwatch is the
      operator's job later; your job is a regression fence.)
  A6. `cargo audit`-clean vendored deps; binary built with
      cargo-auditable; `rg -i "script" src/dom/ast.rs` shows no script
      variant (cheeky but real: the covenant grep).
  A7. All work committed; JOURNAL.md and DECISIONS.md current; REPORT.md
      written for the operator (what works, what's deferred, how to run
      it, the three most interesting bugs).

## 1. AUTONOMY PROTOCOL (how to behave while the operator is away)

- NEVER stop to ask a question. Make the call, record it in DECISIONS.md
  (one paragraph: options, choice, why, revisit-trigger), continue.
- PROCESS: this build runs as a multi-agent TDD operation — see §10 for
  the topology, review gates, and parallelization map. §10 governs HOW
  work happens; the rest of this brief governs WHAT gets built.
- Timebox everything that can wick time: any single blocked avenue gets
  max ~45 minutes of attempts before you take its fallback ladder (§6).
  Note the retreat in DECISIONS.md without shame — the operator prefers
  a finished browser on Tier B to a diary about Tier A.
- Commit at every green state, small and often, conventional messages.
  Never leave the tree red overnight (you don't know when night is;
  treat every milestone boundary as night).
- JOURNAL.md: append-only running log (timestamped) of what you did.
  DECISIONS.md: the forks taken. REPORT.md: written last, for a human.
- Scope discipline: v0.1 is Milestones 0–6 below. Do not gold-plate, do
  not refactor working code for beauty, do not implement charter items
  marked later (X11 backend, Lua chair, no-libc C1-FULL, Transcript UI
  polish) unless everything else is done and green with time to spare —
  in that order, and M7 stretch only.
- The charter's dialect discipline applies to YOU: when a fixture needs
  a feature outside the v0 dialect (§4), the fixture is wrong, not the
  dialect. Amend fixtures, not scope.

## 2. ENVIRONMENT & TOOLCHAIN

Build environment: the monolith-builder Docker image (charter C11).
First actions:
  - Locate/pull the image (check the linux-live-iso-factory repo's
    Dockerfile/Makefile for its name and registry; if unpullable from
    here, build it from that repo's Dockerfile; if that also fails,
    fallback: any Debian/Ubuntu container + document the provenance gap
    prominently in REPORT.md — do not let the container question eat
    the browser).
  - Inside it, install a PINNED Rust nightly (needed for -Zbuild-std):
    pick the newest nightly that works, pin it in rust-toolchain.toml,
    add components rust-src, and vendor all crates (`cargo vendor`,
    committed) so the build is network-free after setup.
  - cargo-auditable + cargo-audit installed and used for release builds.

Target: custom JSON, i486 floor. Start from
`rustc +nightly -Z unstable-options --print target-spec-json --target i586-unknown-linux-musl`
then modify and save as targets/i486-monolith-linux-musl.json:
  - "cpu": "i486"
  - "max-atomic-width": 32        // no CMPXCHG8B on a 486
  - "features": "-mmx,-sse,-sse2,+soft-float"  // verify; no FPU
    assumption is SAFEST for 486SX — but soft-float on musl targets
    needs matching musl; if soft-float fights you (45-min box), fall
    back to hardware float and record that 486SX support becomes a
    known gap (DX-class has the FPU; the myth's machine is a DX2).
  - crt-static enabled (target-feature=+crt-static), relocation static.
Build: `cargo build --release --target targets/i486-monolith-linux-musl.json -Zbuild-std=std,panic_abort -Zbuild-std-features=panic_immediate_abort`
(packet size-squeeze-floppy: panic_immediate_abort drops std's formatted-
panic-message machinery — dead weight under panic=abort, which never
unwinds to read it — and is what keeps the release binary under the
1.44MB floppy ceiling; release-only, never applied to host test builds)
with profile: opt-level="z", lto="fat", codegen-units=1, panic="abort",
strip=true.
Sanity gate before ANY browser code: M0 hello-world through this exact
pipeline runs under qemu-i386. If qemu-i386 is absent, install
qemu-user; if the -cpu 486 model rejects something, verify with
`qemu-i386 -cpu 486` explicitly — catching an illegal instruction here
is the whole point of A4.

## 3. REPOSITORY LAYOUT

  stele/
    rust-toolchain.toml  targets/  vendor/  .cargo/config.toml
    src/
      main.rs            // arg parsing, backend selection, headless
      fetch/             // http1.rs (via httparse), file.rs, cookies.rs
      dom/               // ast.rs (closed sum type), parser.rs (bespoke)
      style/             // css parser (full syntax, ignore-unknown),
                         // cascade, computed values (curated set)
      layout/            // taffy substrate; table.rs (column solver);
                         // inline.rs (text runs, line break, floats)
      text/              // fontdue glue, embedded font (see §5), shaping-
                         // free latin1/utf8 metrics
      img/               // gif.rs jpeg.rs png.rs via crates (§5)
      surface/           // trait Surface { put_px, blit, text_run.. }
                         //   impls: mem.rs (tests/golden), fb.rs, tty.rs
      backend/           // tty.rs (cells+escape), fb.rs (linux fbdev
                         //   ioctls via rustix), event loop, keymap
      chair/             // stub only in v0.1: --dump-ast json to stdout
                         //   (the protocol's read half; full chair later)
    fixtures/            // corpus + goldens (§7)
    accept.sh  JOURNAL.md  DECISIONS.md  REPORT.md

## 4. DIALECT v0 (implement THIS list; parse everything; ignore the rest)

HTML elements (semantics implemented): html head title body meta link*
(ignored ok) — p div span br hr — h1..h6 — a (href, name) — em strong b
i u tt code pre blockquote — ul ol li dl dt dd — table caption tr td th
(colspan rowspan align valign width) — frameset frame noframes — form
input(text password checkbox radio submit reset hidden) textarea select
option button label — img (src alt width height align incl. left/right
floats) — map area (client-side maps) — font (face size color) center —
article section nav header footer main aside figure figcaption (named
blocks) — details summary (open attr; toggle on keypress) — noscript
(FIRST-CLASS: render contents always) — script style (parsed, consumed:
style feeds CSS; script is DISCARDED AT PARSE — no AST variant).
Entities: the HTML 4.01 named set + numeric.
Tag-soup recovery: implied close (p, li, td/tr), mis-nesting tolerance
b/i overlap, unclosed-everything at EOF. Test against the gnarly
fixtures; 1996 HTML is hostile in the mundane way.

CSS (curated computed properties; ALL syntax parsed then filtered):
color background-color font-family font-size font-weight font-style
text-align text-decoration margin* padding* border*(solid only ok)
width height display(block inline none flex) float clear list-style-type
white-space(normal pre) line-height vertical-align(baseline top middle
bottom) — flex-direction flex-wrap justify-content align-items
align-self flex-grow flex-shrink flex-basis gap.
Selectors: element, .class, #id, descendant, grouping, a:link/:visited.
Media queries: evaluate min/max-width once at load against surface size.
Everything else: parsed, counted, ignored (expose count in --headless
--stats; it feeds the future Provenance pane).

Network: HTTP/1.1 GET/POST via plain TCP (rustix sockets + httparse),
gzip (miniz_oxide), Host/UA/Accept, redirects (max 5), cookies
(Netscape jar text file, domain/path rules, no third-party in v0 —
DECISIONS.md if this bites a fixture). file:// for local docs. NO TLS
(charter: proxy's job). URL parsing: bespoke minimal or a no-frills
crate — your call, record it.

## 5. CRATES (vendored; verify each actually builds for the target —
each has a fallback in §6)

taffy (flex substrate; use measure functions for text leaves) ·
httparse · miniz_oxide · fontdue · gif · jpeg-decoder · png ·
hashbrown (if std HashMap hurts size) · rustix (fb ioctls, sockets ok
via std too on Tier B — prefer std net for simplicity, rustix for fb).
FONT: embed ONE bitmap-friendly font for fb (a liberally-licensed
classic bitmap font converted to a raw glyph atlas beats shipping a
TTF + rasterizing on a 486 — decide, measure both, record; tty backend
needs no font). License-check every crate + the font into REPORT.md.

## 6. FALLBACK LADDERS (take them briskly; 45-min rule)

L1 libc tier: (A) attempt is NOT required in v0.1 — build Tier B
  (std + static musl via the custom target) from the start; no-libc is
  charter C1-FULL, later. If the custom i486 JSON itself fights
  -Zbuild-std, fall to stock i586-unknown-linux-musl target
  TEMPORARILY, keep building features, and return to the i486 target
  before acceptance (A4 must pass on -cpu 486; an i586-only binary
  fails DONE — the 486 is the constitution).
L2 soft-float: see §2; hardware float + documented 486SX gap is legal.
L3 taffy: if it won't build no_std/for-target or fights measure-fns —
  it's Tier B std so it should build — but if broken anyway: hand-roll
  the flex algorithm for the row/column subset the dialect needs
  (single-axis distribution, grow/shrink/basis, wrap). It's ~600 lines.
  Do not hand-roll first; do not fight taffy past the timebox either.
L4 image crates: any decoder that won't build → try zune-* equivalents
  → worst case defer THAT format behind a feature flag, render alt
  text, record. (GIF is the one worth extra fight; it is 1996.)
L5 fontdue/fb text: → embedded bitmap atlas path (likely better on a
  486 anyway).
L6 anything else structurally blocked: stub it behind a trait, keep the
  pipeline green, list it in REPORT.md under "Deferred honestly."

## 7. FIXTURES & GOLDENS (write these EARLY — M1 — they are the spec)

fixtures/: basic.html (headings, paras, links) · soup.html (the
tag-soup torture set) · tables.html (nested, colspan/rowspan, width
games — recreate a 1996-style table layout page) · frames.html +
children · forms.html · images.html (gif/jpeg/png + animated gif +
align=left float with wrapping text) · flex-polite.html (a modern
no-JS blog layout: semantic tags + flexbox) · media-query.html ·
kitchen-sink.html (everything; the A5 benchmark page) · noscript.html ·
entities.html. Plus 2–3 REAL 1996 pages if fetchable from the public
web during setup (save them INTO fixtures; tests never touch network).
Goldens: tty text dumps (assert exact) and fb renders as PNGs from the
mem Surface (assert pixel-exact; regenerate deliberately via
`accept.sh --bless` with a JOURNAL note each time). A local fixture
HTTP server (tiny rust bin in tests/) exercises fetch/cookies/redirects
/POST — no external network in tests, ever.

## 8. MILESTONES (each ends: green tests, commit, JOURNAL entry)

M0 Toolchain: image up, pinned nightly, i486 target JSON, hello via
   qemu-i386 -cpu 486. accept.sh skeleton with A1/A4 live.
M1 Fetch+parse+DOM: http/file fetch, parser vs soup fixtures,
   --headless --dump-text (raw text extraction), fixture server,
   goldens begin.
M2 Style+block layout+tty: CSS parse/cascade (curated set), taffy
   block flow, inline engine v1 (text runs, wrapping, no floats yet),
   tty backend interactive (scroll, follow links, back). First real
   browsing (of fixtures).
M3 Tables+frames+forms: column solver pre-pass, frameset viewports,
   form widgets + POST. soup/tables/forms goldens green.
M4 fb backend+images+floats: fbdev via rustix, mem-Surface goldens,
   decoders wired, animated GIF ticking, align=left float wrapping.
   THE SCREENSHOT: fb render of images.html — save it to REPORT.md.
M5 Dialect completeness: details/summary, media queries, noscript,
   entities, cookies jar, flex-polite.html pixel-green. --stats
   ignored-declaration counter.
M6 Hardening+acceptance: fuzz the parser (cargo-fuzz an hour or a
   hand-rolled mutation loop over fixtures if fuzzing infra fights),
   size pass (strip, audit biggest symbols), A1–A7 all green, REPORT.md.
M7 STRETCH ONLY (in order): Lua chair (mlua static or fall to the
   stdin/stdout protocol which needs no crate) · X11 backend ·
   Transcript pane · no-libc spike (spike = branch + notes, not a
   rewrite).

## 9. WHAT NOT TO DO

No TLS. No JS engine, no exceptions, no "tiny eval". No living-standard
chasing (the fixture is wrong, remember). No async runtime (this is a
single-threaded program; blocking IO is correct here). No display
required for tests (mem Surface exists so CI/you never need X/fbdev).
No network in tests. No unbounded refactors. No waiting for the
operator. The operator's parting instruction is the DHCP lecture in
reverse: you have the wire to yourself — answer confidently, log
everything, and be findable in the commit history, port by port.

## 10. MULTI-AGENT TDD PROCESS

Topology: one ORCHESTRATOR (you, reading this) + IMPLEMENTER subagents
+ REVIEWER subagents. The orchestrator plans work packets, dispatches,
merges, and personally reviews every final PR before it lands on main.
Implementers never merge their own work. Reviewers never fix; they
report (approve / request-changes with specifics).

### TDD protocol (adapted honestly for a renderer)
- Contract-testable modules (parser, CSS cascade, table column solver,
  cookie rules, URL/HTTP handling, entity decoding, line breaker given
  fixed metrics): STRICT test-first. The failing test is committed
  before or with the implementation; reviewers verify red→green is
  visible in the packet's history (spot-check: revert impl, tests must
  fail).
- Golden-image/golden-text modules (rendering, backends): pixel goldens
  cannot precede the renderer — the honest adaptation is BLESSING
  DISCIPLINE: implementer produces a render, files it as a PROPOSED
  golden with a JOURNAL note; a reviewer subagent independently
  inspects the render against the fixture's intent (and the dialect
  §4) and COUNTERSIGNS before `accept.sh --bless` admits it. An
  implementer never blesses their own goldens — an unblessed golden is
  a test that tests nothing.
- Every work packet ships with tests, or with a written reason why its
  seam is covered elsewhere. "Tested by the kitchen sink later" is not
  a reason.

### Work packets & branching
- One branch per packet (`packet/<module>-<short>`), one PR per packet,
  small. Module OWNERSHIP per wave (below): two implementers never edit
  the same src/ subtree in one wave; shared types change only in
  interface-freeze packets (serial, orchestrator-authored).
- Merge order within a wave is by dependency then readiness; rebase
  discipline on the implementers; the orchestrator resolves conflicts
  only by bouncing the packet back, never by editing in the merge.
- Reviewer checklist (every PR): covenant compliance (no script variant
  touched, dialect §4 whitelist respected, no TLS/no async/no JS
  anywhere), tests-first evidence per above, fallback-ladder use
  recorded in DECISIONS.md if taken, size delta noted if > 50 KB,
  fixtures amended only with justification (the fixture is wrong, not
  the dialect — but say why).
- FINAL PR of each milestone: orchestrator reviews personally against
  the milestone's acceptance line in §8 AND runs accept.sh; a milestone
  closes only green.

### Parallelization map (the dependency truth; do not exceed it)
SERIAL SPINE: M0 (toolchain) → INTERFACE FREEZE packet (orchestrator
writes the core types/traits: dom::ast, style::ComputedStyle skeleton,
Surface trait, fetch::Response, layout node interface — small, typed,
stubbed) → waves → integration milestones.
WAVE 1 (after freeze, fully parallel — 4-5 implementers):
  P1 dom parser + soup fixtures · P2 CSS parser/cascade + fixtures ·
  P3 fetch/http/cookies + fixture server · P4 image decoders behind a
  Decode trait + fixtures · P5 text/metrics (fontdue or atlas spike —
  both, race them, keep the winner).
WAVE 2 (needs Wave 1 interfaces landed; parallel where shown):
  P6 block flow + inline engine (the long pole — assign your strongest
  configuration; starts against fixture ASTs the moment P1's types
  land) · P7 tty backend + mem Surface goldens (parallel to P6 once
  minimal layout exists; coordinate via the Surface trait only) ·
  P8 table column solver (contract-tested pure function; parallel,
  joins P6 at integration).
WAVE 3: P9 fb backend + image pipeline wiring · P10 frames + forms ·
  P11 dialect completeness (details, media queries, entities, noscript)
  — parallel, distinct subtrees.
SERIAL CLOSE: M6 hardening/fuzz/size/acceptance — orchestrator-led,
single branch, no parallelism (fuzz findings and size passes touch
everything; parallel hands here cause churn).
Ceiling rule: if two packets keep colliding, the interface between them
is underspecified — stop, write a freeze packet, then resume. Merge
hell is a design smell, not a coordination problem.
