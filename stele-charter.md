# STELE — founding charter
# The document-web browser. A standalone project The Monolith ships first.
#
# NAME (naming due diligence per house law): a stele is a stone slab that
# exists to carry inscriptions — a rock whose entire purpose is bearing
# documents. Monolith-family without being Monolith-branded. Pronounced
# STEE-lee. Collision check before canon: crates.io, GitHub topline, npm.
# Fallbacks in the same register: basalt, menhir. (Obsidian is taken and
# famous; do not fight that war.)

---

## THE KILL CONDITION

Stele kills Netscape when, on a 486DX2/66 with 16 MB, browsing the real
web through monolith-surf:

  K1. First paint of a text document beats Navigator 4 on identical
      hardware. Target: < 1s from Enter to readable text. (CI measures
      under emulation as a regression budget; the RECORD is set on real
      iron and published, per the world-proves-the-hardware doctrine.)
  K2. Everything Navigator 4 did for DOCUMENTS renders correctly:
      HTML 4.01 Transitional tag soup, tables (incl. nested, the 1996
      layout medium), frames + framesets, forms (GET/POST, all input
      types), GIF (incl. animated — non-negotiable, it is 1996) /
      JPEG (progressive AND baseline) / PNG, client-side image maps,
      cookies (Netscape jar format, as a text file the user owns).
  K3. Everything Navigator did to become an operating system does not
      exist and CANNOT exist: no JavaScript, no Java, no plugins, no
      updater, no telemetry, no mail client, no push channels. Not
      declined — unrepresentable (see Architecture).
  K4. A user coming from Navigator loses no document; a page that needs
      an application runtime renders its content, inert, with a status
      line noting what was refused. Refusal is always legible.

The strategic insight Stele ships as code: Netscape died of its own
rewrite — bloat was the cause of death on the historical record. Stele
is finished software. Its dialect is frozen; its version numbers should
converge, not grow.

## THE COVENANT (constitutional, CI-enforced where enforceable)

  C1. One static binary. PHASED: v0.x links static musl (one binary,
      zero runtime deps — disc-doctrine compliant); C1-FULL (no libc
      at all — raw syscalls via rustix/origin, nothing beneath Stele
      but the kernel ABI) is a named later milestone, not a v0 gate.
 C11. BUILD PROVENANCE: Stele builds inside the monolith-builder
      image, pinned by digest — the same attested environment that
      builds the disc. cargo-auditable embeds the full dependency
      manifest in the binary; releases are attested like the ISO.
      Stele's provenance story is the Monolith's, inherited.
  C2. The dialect is CURATED, not frozen — defined by principle, not
      by year. Principles: (a) declarative only — documents describe,
      never compute; (b) the IGNORE-UNKNOWN TREATY: full HTML/CSS
      syntax is parsed, a curated semantic set is implemented, and the
      remainder is silently skipped per the standards' own forward-
      compat rules — Stele is never wrong about a polite site, only
      selectively fluent, with no quirks matrix and no compat chase;
      (c) amendments are deliberate, individually priced against the
      486 speed budget and binary size, biased toward what the engine
      already speaks; (d) nothing reactive-coupled — no re-layout
      storms, no app semantics in costume (animations/transitions
      render as final state).
      Initial dialect: HTML 4.01 Transitional + HTML5 semantic/
      sectioning elements; CSS1 + FLEXBOX (in-dialect day one — the
      engine speaks it natively and polite modern documents use it) +
      one-shot media queries.
      ADOPTED AMENDMENTS (deliberate, priced, per clause (c) — recorded
      here so the charter never trails the code): CSS custom properties
      + var() (packet T1a); block floats via the flex substrate
      (PR #65, +16 KB); CSS GRID, handed to taffy's own algorithm, not
      reimplemented (PR #69); box-model completion — box-sizing,
      per-side borders, margin collapsing. Still candidates: <details>/
      <summary> interactive toggle (rendered as blocks today), MathML,
      static SVG. The dialect ships as a versioned one-page document:
      "What Stele Speaks."
      HTTP/1.1 + gzip. The RUDE web is the proxy's problem; the
      polite web is Stele's native constituency.
  C3. Hostile content is unrepresentable: the document AST is a closed
      sum type with no variant for script, no variant for executable
      anything. Parsing cannot produce what layout cannot hold.
      TRUE NAME: the heresy is not computation — the disc teems with
      it — but UNINVITED computation. A page auto-executing a
      stranger's code is the browser-shaped open port. Nothing in
      Stele runs until invited; the chair (C5) is invited by
      definition; the wire never is.
      THE JS TREATY (mirror of C2's CSS treaty, opposite direction):
      the polite web's own ethic is progressive enhancement — polite
      documents work script-free by their authors' stated code.
      Refusing wire-code does not degrade polite sites; it takes them
      at their word. <noscript> content renders FIRST-CLASS: the
      document web's fallback channel finally has a browser that
      honors it. Document-serving JS decomposes without remainder
      into: missing DIALECT (form-validation attributes, MathML,
      static script-free SVG — all C2 amendment candidates) and
      missing CHAIR (sortable tables, page/site search, highlighting
      — browser capabilities serving every document ever written,
      plus user Lua scripts in the Greasemonkey lineage, incl. a
      curated stele-scripts collection on disc). The residue that
      does not decompose is the application web done well — which is
      still the application web, and has a home that is not the rock.
  C4. panic=abort. The rock does not unwind.
  C5. The user is the only scripting engine: Lua chair (keybinds,
      filters, per-site rewrite rules, automation; headless mode for
      cron/Perl composition). Pages are inert material; intelligence
      sits in the chair.
      THE CHAIR IS RUNTIME-PLUGGABLE: beyond built-in Lua, a chair
      PROTOCOL — Stele pipes the document (AST-as-JSON or HTML) to
      any user program on stdin and reads transformations back on
      stdout. Shebang-dispatched, language-agnostic, Unix-shaped:
      Perl in 1996, JavaScript via the user's own Node/Bun whenever
      the user installs one, anything with stdin in between. Any
      language earns a seat in the chair the day its runtime is
      invited onto the machine. The wire can never issue that
      invitation (C3); and wire code is never delegated to external
      runtimes in particular — Node/Bun are ambient-authority server
      runtimes, and a stranger's code in one is the open port with a
      JIT. No plumbing from wire to any engine exists to misuse: the
      AST has no script variant to hand over.
  C6. Plain-file state: bookmarks a text file, history a log, cache a
      dir of hash-named files, cookie jar Netscape-format text. grep
      is the extension API.
  C7. Teaching keys, first-class: view-source; the Transcript (the
      actual HTTP conversation, DORA-lecture readable); Provenance
      (URL, bytes, hash, what the proxy translated). One keystroke
      each.
  C8. Fully attested: vendored crates + lockfile + cargo-auditable —
      the dependency manifest embedded IN the binary. Stele should be
      the most precisely accounted-for object on any disc it ships on.
  C9. Toolchain vow: ONE pinned rustc, vendored into the attested
      build container like any other source; custom target JSON for
      i486 (cpu=486, max-atomic-width=32 — no CMPXCHG8B exists there,
      and a single-threaded browser wants no 64-bit atomics). The
      mrustc bootstrap path is the documented archival answer to
      "rustc builds rustc."
 C10. Speed budget in CI forever: first-paint regression gate on the
      emulated 486 profile. A commit that slows first paint fails.

## ARCHITECTURE

One binary, three backends, runtime-selected:
  tty  — cells + escape codes, zero deps, works on anything with a
         console (the guaranteed floor).
  fb   — Linux framebuffer: embedded-graphics for drawing, fontdue for
         rasterization, inline images on a DX2 with no X server.
  x11  — wire-protocol X client over a socket (no Xlib), crisp beside
         Window Maker on the Pentium.
Same keybindings across all three; the skill follows the user across
hardware the way the disc does.

Substrate (exists today, no_std or no_std-capable): httparse (HTTP),
miniz_oxide (gzip), zune/jpeg-decoder/png/gif family (images; budget
maturity-vetting time), fontdue (glyphs), embedded-graphics (fb 2D),
bumpalo (DOM arena), rustix (syscalls), heapless where it helps.
TLS: none in v1 — monolith-surf owns modernity; direct-TLS is a later
optional feature (rustls/embedded-tls have no_std stories) that must
never become a default dependency. TLS is DELEGATED, never embedded —
zero bytes of cryptography in the binary; on Linux the provider is the
user's own openssl, exec'd as a child (s_client), verified and
fail-closed.

Bespoke (the soul, written in-house, ~the third you'd insist on owning
anyway):
  1. The parser: frozen-dialect HTML with 1996-grade tag-soup recovery.
     A few thousand lines. Owning it IS the covenant — html5ever is
     std-bound and living-standard-shaped, i.e. the thing we refuse.
  2. The layout engine: factored as SOLVERS OVER A FLEX SUBSTRATE.
     Adopt taffy (verify no_std/alloc feature status) as the box-math
     primitive: block flow = degenerate column flex; frames = nested
     row/column viewports; tables = a bespoke HTML 4.01 auto-layout
     pre-pass (two-pass min/max-content column solver + row heights +
     span placement — the genuinely hard part, written in-house) whose
     output feeds flex as fixed bases. Inline layout — text runs, line
     breaking, 1996 floats (img align=left) — is bespoke regardless,
     hanging off measure-function leaves; it is the soul of the
     program and no crate provides it.
     BONUS, EARNED: the engine's native tongue is flexbox, so Stele's
     own chrome (Transcript, Provenance, status, Lua-chair UI) is laid
     out in modern flex on a 486 — the one thing the app web got
     right, adopted as math, without its ecosystem.
     BOUNDARY (covenant-adjacent): engine capability enters the
     dialect only by C2's amendment process — but flexbox itself is
     in-dialect from day one (the engine speaks it; polite documents
     use it; refusing it would be ideology at the user's expense).
     What never enters: anything that computes. C3 does not move.
  3. The three backends' compositor glue + the Transcript/Provenance
     panes + the Lua chair bindings.

## MILESTONES

  M0  Toolchain: pinned rustc + i486 target JSON + no-libc hello on
      tty/fb under QEMU -cpu 486. (Proves the vow before the program.)
  M1  tty text browsing through monolith-surf: fetch, parse, flow
      layout, links, forms. The kid can read the web.
  M2  Tables + frames. The 1996 web becomes correct, not just legible.
  M3  fb backend + image pipeline (GIF anim, progressive JPEG, PNG).
      Pictures on the DX2. Screenshot goes in the README and nowhere
      is it explained, because it explains itself.
  M4  x11 backend. Window Maker desktop shot; second README image.
  M5  Lua chair + headless mode. First extension shipped as a doc
      example: a five-line history-grep.
  M6  Transcript + Provenance keys. The browser that teaches HTTP.
  M7  THE KILL TEST: side-by-side vs Navigator 4.08 (from the Browser
      Drawer, on persist) on real 486 hardware, same pages, stopwatch,
      published with photos. K1-K4 checked off in public.

## RELATIONSHIP TO THE MONOLITH

Standalone project, own repo, own covenant (this file). The Monolith is
first distributor: pins a version, attests it in the SBOM, ships it as
the default browser alongside links2/lynx during Stele's adolescence,
then alone. monolith-surf is a sibling, not a dependency — Stele speaks
frozen HTTP to anything; the proxy is how the frozen dialect reads the
living web.

Myth clause, for the eventual README: the kid receives a browser whose
implementation language will not be invented for fourteen years,
compiled for a chip its toolchain officially abandoned, that wins the
browser war by declining to enter it. The caveman did not build this
one either. But the one who did was once the caveman.
