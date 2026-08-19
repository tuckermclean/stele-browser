#!/usr/bin/env bash
#
# accept.sh — Stele's definition of DONE (build brief §0). Exit 0 == acceptance.
#
# Checks A1..A7 come online milestone by milestone. This is the M0 skeleton:
# A1 (statically-linked i386 binary) and A4 (the i486 binary runs under
# qemu-i386 -cpu 486 and prints its golden text) are LIVE; the rest announce
# themselves as PENDING against the milestone that lands them, and do not yet
# gate. As each milestone completes, its check flips from PENDING to live here.
#
# Usage:
#   ./accept.sh              run all live checks; exit nonzero on first failure
#   ./accept.sh --bless      regenerate golden outputs from the current binary
#                            (never bless your own render blind — see brief §10)
#   ./accept.sh --tty-only   run ONLY the A3 host-native tty-golden check (no
#                            i486/qemu involvement at all). For CI: the plain
#                            `accept` job has no Rust toolchain (qemu-user +
#                            file only), so a cargo-dependent check has no
#                            business running there — A3's host build+dump+
#                            diff instead runs from the `build` job (which
#                            already carries the pinned toolchain) via this
#                            flag. `--bless` composes with it.
#
# The i486 binary (A1/A4) is expected pre-built at:
#   target/i486-monolith-linux-musl/release/stele
# Build it with the canonical pipeline (inside the monolith-builder image;
# the unwind shim in RUSTFLAGS below is the CI/local linker workaround —
# see the "Build i486 release binary" CI step for its full derivation):
#   export RUSTFLAGS="-L /tmp/unwindshim -Zunstable-options -Cpanic=immediate-abort"
#   cargo build --release \
#     --target targets/i486-monolith-linux-musl.json \
#     -Zbuild-std=std,panic_abort \
#     -Zjson-target-spec   # this nightly gates .json target specs behind it
# -Cpanic=immediate-abort (packet size-squeeze-floppy) drops std's formatted-
# panic-message machinery from the release binary — panic=abort never
# unwinds to read that message anyway, so this is pure fat-trim, not a
# behavior change. It's a RUSTC flag (`-C`), not a cargo flag — `cargo build
# -C...` itself errors, so it travels via RUSTFLAGS, which also makes the
# build-std core/std compile pick it up (required: immediate-abort must
# apply to core, not just this crate). (NOTE: this used to be requested via
# -Zbuild-std-features=panic_immediate_abort; the pinned nightly promoted
# that to a real panic strategy selected via -Cpanic=immediate-abort
# instead — see the compiler's own "panic_immediate_abort is now a real
# panic strategy" error if this drifts again.) RUSTFLAGS here is scoped to
# this one build invocation, never Cargo.toml's [profile.release] — that
# profile is shared with the host unit-test build, which relies on normal
# panic/assert + #[should_panic] behavior. Release-only: never applies to
# A3's host build below (no build-std there at all — see that section's
# own note).
#
# A3's host binary is a plain `cargo build --release` (default host target,
# no `+nightly`/`+<toolchain>` override — that would bypass rust-toolchain.
# toml's pin and fetch a floating nightly, violating charter C9's "ONE
# pinned rustc"; plain `cargo` already resolves the pin via the file).

set -uo pipefail

TARGET_STEM="i486-monolith-linux-musl"
BIN="target/${TARGET_STEM}/release/stele"
GOLDEN_HELLO="goldens/m0-hello.txt"
SIZE_BUDGET_BYTES=$(( 2 * 1000 * 1000 ))   # A2: 2.0 MB stripped

BLESS=0
TTY_ONLY=0
for arg in "$@"; do
  case "$arg" in
    --bless) BLESS=1 ;;
    --tty-only) TTY_ONLY=1 ;;
  esac
done

fail=0
note()  { printf '  %s\n' "$*"; }
pass()  { printf '\033[32mPASS\033[0m %s\n' "$*"; }
bad()   { printf '\033[31mFAIL\033[0m %s\n' "$*"; fail=1; }
pend()  { printf '\033[33m····\033[0m %s\n' "$*"; }

# Resolve a user-mode qemu for i386.
find_qemu() {
  for q in qemu-i386 qemu-i386-static; do
    if command -v "$q" >/dev/null 2>&1; then echo "$q"; return 0; fi
  done
  return 1
}

if [ "$TTY_ONLY" = 1 ]; then
  echo "== Stele acceptance — A3 tty-golden only (host, pinned toolchain) =="
else
  echo "== Stele acceptance (M0 skeleton) =="
fi

# ---------------------------------------------------------------------------
# A1 — statically linked, i386-class ELF.
# ---------------------------------------------------------------------------
if [ "$TTY_ONLY" = 1 ]; then
  :
elif [ ! -f "$BIN" ]; then
  bad "A1: binary not found at $BIN (build it first — see header)"
else
  desc="$(file -b "$BIN")"
  note "file: $desc"
  if echo "$desc" | grep -q "ELF 32-bit" \
     && echo "$desc" | grep -q "Intel 80386" \
     && echo "$desc" | grep -qi "statically linked"; then
    pass "A1: statically linked, i386-class ELF"
  else
    bad "A1: expected 32-bit Intel 80386, statically linked"
  fi
fi

# ---------------------------------------------------------------------------
# A4 — the i486 binary runs under qemu-i386 -cpu 486 and prints golden text.
# This is the point of the whole exercise: catch any non-486 instruction.
# ---------------------------------------------------------------------------
if [ "$TTY_ONLY" = 1 ]; then
  :
elif [ ! -f "$BIN" ]; then
  bad "A4: binary not found at $BIN"
elif ! QEMU="$(find_qemu)"; then
  bad "A4: no qemu-i386 found (install qemu-user); cannot execute the i486 binary"
else
  note "qemu: $QEMU -cpu 486"
  if out="$("$QEMU" -cpu 486 "$BIN" 2>/tmp/stele_a4.err)"; then
    if [ "$BLESS" = 1 ]; then
      printf '%s\n' "$out" > "$GOLDEN_HELLO"
      pass "A4: blessed golden -> $GOLDEN_HELLO"
    elif diff -u "$GOLDEN_HELLO" <(printf '%s\n' "$out") >/tmp/stele_a4.diff 2>&1; then
      pass "A4: qemu-i386 -cpu 486 output matches golden"
    else
      bad "A4: output differs from $GOLDEN_HELLO"; sed 's/^/    /' /tmp/stele_a4.diff
    fi
  else
    bad "A4: binary crashed/illegal-instruction under qemu-i386 -cpu 486"
    sed 's/^/    /' /tmp/stele_a4.err
  fi
fi

# ---------------------------------------------------------------------------
# A2 — size budget (≤ 2.0 MB stripped). HARD GATE as of M6 (release
# hardening — this is the milestone the header comment/build brief always
# named as when A2 stops being informational and starts failing the build).
# The i486 binary has stayed comfortably under budget since M4 (~542KB at
# the time this gate flipped on, well under a third of the 2.0MB ceiling),
# so this is expected to pass with room to spare — flipping it to a hard
# `bad` (rather than `pend`) on overage is what actually makes A2 load-
# bearing: a future packet that quietly bloats the binary (an accidental
# heavy dependency, an unstripped debug build, ...) now fails acceptance
# instead of sailing through with an ignored warning.
# ---------------------------------------------------------------------------
if [ "$TTY_ONLY" = 1 ]; then
  :
elif [ -f "$BIN" ]; then
  bytes=$(wc -c < "$BIN")
  note "size: ${bytes} bytes (budget ${SIZE_BUDGET_BYTES})"
  if [ "$bytes" -le "$SIZE_BUDGET_BYTES" ]; then
    pass "A2: within size budget (${bytes} <= ${SIZE_BUDGET_BYTES} bytes)"
  else
    bad "A2: OVER size budget (${bytes} > ${SIZE_BUDGET_BYTES} bytes)"
  fi
fi

# ---------------------------------------------------------------------------
# A6a (covenant, sub-check of the TLS-delegation packet): TLS is DELEGATED —
# the binary must contain zero embedded cryptography. openssl runs as a
# CHILD; its NAME may appear in our error strings, but no TLS/crypto
# implementation symbols may be linked in. Distinct from the reserved A6
# (cargo-audit-clean vendored deps + the no-`script`-variant covenant grep
# over src/dom/ast.rs, per the build brief) — same letter, different concern,
# so this one is A6a (mirrors the A3a/A5a-style sub-label convention used
# elsewhere in this file).
# ---------------------------------------------------------------------------
check_a6a_covenant() {
  if [ ! -f "$BIN" ]; then
    bad "A6a: binary not found at $BIN"
    return
  fi
  # Symbols that would betray an embedded TLS/crypto stack (rustls, ring,
  # openssl-sys, boringssl, embedded-tls). The word "openssl" alone is allowed
  # (our error text); these are library-internal symbols that never appear
  # unless a crypto crate is linked. ring::'s alternative is scoped to its
  # actual crypto submodules (not a bare `ring::` substring) — a bare
  # substring false-positives on unrelated debug-info symbols like
  # `alloc::string::String` (which contains "ring::" as a substring).
  if strings -a "$BIN" | grep -Eiq 'rustls|ring::(aead|rand|digest|hmac|hkdf|signature|agreement|pbkdf2|error)|boringssl|libcrypto|SSL_CTX_new|EVP_|X509_verify|embedded_tls'; then
    bad "A6a: covenant broken — TLS/crypto implementation symbols found in $BIN"
    strings -a "$BIN" | grep -Ei 'rustls|ring::(aead|rand|digest|hmac|hkdf|signature|agreement|pbkdf2|error)|boringssl|libcrypto|SSL_CTX_new|EVP_|X509_verify|embedded_tls' | head | sed 's/^/    /'
  else
    pass "A6a: covenant intact — no embedded TLS/crypto symbols in the binary"
  fi
}

if [ "$TTY_ONLY" = 1 ]; then
  :
elif [ -f "$BIN" ]; then
  check_a6a_covenant
fi

# ---------------------------------------------------------------------------
# A3 — fixture golden renders. The tty-dump half is LIVE as of P7/M2: a
# host-native (no qemu — --dump-text has no 486-specific instructions; A4
# already exhaustively probes that) run of `stele --headless --dump-text`
# over fixtures/basic.html must match the checked-in golden exactly. The
# mem-Surface PNG half is now LIVE too, as of the M4 pixel-foundation
# packet (A3e, below): `stele --headless --dump-png` over fixtures/basic.html
# must match goldens/basic.png byte-for-byte (the PNG encoder is
# deterministic — see backend::raster's own doc comments). That PNG golden
# is PROPOSED (brief §10 blessing discipline), same as the tty goldens. The
# M4 images packet adds A3f: the same PNG-golden check over
# fixtures/images.html (goldens/images.png) — THE SCREENSHOT, real decoded
# image pixels blitted via the images fetch+decode pre-pass, not just
# layout/paint of boxes and text.
#
# Host binary: built via a plain `cargo build --release` (the default host
# target, NOT the i486 cross target `$BIN` used by A1/A4) since this check
# only exercises pure Rust logic (parse/cascade/layout/tty), not 486-legal
# codegen.
# ---------------------------------------------------------------------------
HOST_BIN="target/release/stele"
GOLDEN_TTY="goldens/basic.tty.txt"
FIXTURE_BASIC="fixtures/basic.html"

# This check is cargo-dependent by nature (it builds and runs a fresh host
# binary), and NOT every job that runs this script carries a Rust toolchain
# — the plain `accept` CI job only has qemu-user + file (see --tty-only's
# usage doc above). Rather than hard-failing there, treat "no cargo" as
# PENDING: informational, not a rejection. Where cargo IS available (locally,
# or the `build` job via `--tty-only`), the check is fully live.
if ! command -v cargo >/dev/null 2>&1; then
  pend "A3: no cargo in this environment — tty-golden check runs in the build job (pinned toolchain) instead"
else
  # Always (re)build: a stale `target/release/stele` from an earlier packet
  # must never let this check silently pass/fail against old code. Plain
  # `cargo` (no `+nightly`/`+<toolchain>` override) so rust-toolchain.toml's
  # pin governs — see the header comment.
  note "A3: building host binary (cargo build --release)"
  if ! cargo build --release >/tmp/stele_a3_build.log 2>&1; then
    bad "A3: host build failed"; sed 's/^/    /' /tmp/stele_a3_build.log
  fi

  if [ ! -f "$HOST_BIN" ]; then
    bad "A3: host binary still not found at $HOST_BIN"
  elif ! out="$("$HOST_BIN" --headless --dump-text "$FIXTURE_BASIC" 2>/tmp/stele_a3.err)"; then
    bad "A3: stele --headless --dump-text crashed on $FIXTURE_BASIC"
    sed 's/^/    /' /tmp/stele_a3.err
  elif [ "$BLESS" = 1 ]; then
    printf '%s\n' "$out" > "$GOLDEN_TTY"
    pass "A3: blessed tty golden -> $GOLDEN_TTY (never bless your own render blind — see brief §10)"
  elif diff -u "$GOLDEN_TTY" <(printf '%s\n' "$out") >/tmp/stele_a3.diff 2>&1; then
    pass "A3: tty dump of $FIXTURE_BASIC matches golden"
  else
    bad "A3: tty dump of $FIXTURE_BASIC differs from $GOLDEN_TTY"
    sed 's/^/    /' /tmp/stele_a3.diff
  fi

  # A3b — the table-layout packet's own tty golden (fixtures/tables.html):
  # same host binary (already built above), same blessing discipline. Kept
  # as its own block (not folded into the basic-golden if/elif chain above)
  # so a failure/bless of one fixture doesn't short-circuit the other.
  GOLDEN_TTY_TABLES="goldens/tables.tty.txt"
  FIXTURE_TABLES="fixtures/tables.html"
  if [ ! -f "$HOST_BIN" ]; then
    bad "A3b: host binary still not found at $HOST_BIN"
  elif ! out_tables="$("$HOST_BIN" --headless --dump-text "$FIXTURE_TABLES" 2>/tmp/stele_a3b.err)"; then
    bad "A3b: stele --headless --dump-text crashed on $FIXTURE_TABLES"
    sed 's/^/    /' /tmp/stele_a3b.err
  elif [ "$BLESS" = 1 ]; then
    printf '%s\n' "$out_tables" > "$GOLDEN_TTY_TABLES"
    pass "A3b: blessed tables tty golden -> $GOLDEN_TTY_TABLES (never bless your own render blind — see brief §10)"
  elif diff -u "$GOLDEN_TTY_TABLES" <(printf '%s\n' "$out_tables") >/tmp/stele_a3b.diff 2>&1; then
    pass "A3b: tty dump of $FIXTURE_TABLES matches golden"
  else
    bad "A3b: tty dump of $FIXTURE_TABLES differs from $GOLDEN_TTY_TABLES"
    sed 's/^/    /' /tmp/stele_a3b.diff
  fi

  # A3c — the form-rendering packet's own tty golden (fixtures/forms.html):
  # same host binary, same blessing discipline, same independent block so a
  # failure/bless of one fixture doesn't short-circuit the others.
  GOLDEN_TTY_FORMS="goldens/forms.tty.txt"
  FIXTURE_FORMS="fixtures/forms.html"
  if [ ! -f "$HOST_BIN" ]; then
    bad "A3c: host binary still not found at $HOST_BIN"
  elif ! out_forms="$("$HOST_BIN" --headless --dump-text "$FIXTURE_FORMS" 2>/tmp/stele_a3c.err)"; then
    bad "A3c: stele --headless --dump-text crashed on $FIXTURE_FORMS"
    sed 's/^/    /' /tmp/stele_a3c.err
  elif [ "$BLESS" = 1 ]; then
    printf '%s\n' "$out_forms" > "$GOLDEN_TTY_FORMS"
    pass "A3c: blessed forms tty golden -> $GOLDEN_TTY_FORMS (never bless your own render blind — see brief §10)"
  elif diff -u "$GOLDEN_TTY_FORMS" <(printf '%s\n' "$out_forms") >/tmp/stele_a3c.diff 2>&1; then
    pass "A3c: tty dump of $FIXTURE_FORMS matches golden"
  else
    bad "A3c: tty dump of $FIXTURE_FORMS differs from $GOLDEN_TTY_FORMS"
    sed 's/^/    /' /tmp/stele_a3c.diff
  fi

  # A3d — the frames packet's own tty golden (fixtures/frames.html): a
  # <frameset> document, so this exercises the frames.rs recursive
  # fetch->parse->cascade->layout->tty composite path (each <frame src>
  # is its own real file:// fetch of a sibling fixture), not just the
  # single-document pipeline the other A3* checks drive. Same blessing
  # discipline, same independent block.
  GOLDEN_TTY_FRAMES="goldens/frames.tty.txt"
  FIXTURE_FRAMES="fixtures/frames.html"
  if [ ! -f "$HOST_BIN" ]; then
    bad "A3d: host binary still not found at $HOST_BIN"
  elif ! out_frames="$("$HOST_BIN" --headless --dump-text "$FIXTURE_FRAMES" 2>/tmp/stele_a3d.err)"; then
    bad "A3d: stele --headless --dump-text crashed on $FIXTURE_FRAMES"
    sed 's/^/    /' /tmp/stele_a3d.err
  elif [ "$BLESS" = 1 ]; then
    printf '%s\n' "$out_frames" > "$GOLDEN_TTY_FRAMES"
    pass "A3d: blessed frames tty golden -> $GOLDEN_TTY_FRAMES (never bless your own render blind — see brief §10)"
  elif diff -u "$GOLDEN_TTY_FRAMES" <(printf '%s\n' "$out_frames") >/tmp/stele_a3d.diff 2>&1; then
    pass "A3d: tty dump of $FIXTURE_FRAMES matches golden"
  else
    bad "A3d: tty dump of $FIXTURE_FRAMES differs from $GOLDEN_TTY_FRAMES"
    sed 's/^/    /' /tmp/stele_a3d.diff
  fi

  # A3e — the pixel-foundation packet's own PNG golden (M4): same host
  # binary, same blessing discipline. Compared by raw byte equality rather
  # than re-decoding here (bash has no PNG decoder handy) — this is
  # equivalent to a pixel-array comparison for this specific golden because
  # `backend::raster::encode_png` is proven deterministic (no timestamp/text
  # chunks; see its own `encode_png_is_deterministic` unit test) AND
  # `tests/png_golden.rs`'s own Rust test already does the real
  # decode-and-compare-pixels check the brief asks for — this shell check is
  # an independent end-to-end confirmation (real `file://` fetch through the
  # compiled binary, not `include_str!`), not the only line of defense.
  GOLDEN_PNG="goldens/basic.png"
  if [ ! -f "$HOST_BIN" ]; then
    bad "A3e: host binary still not found at $HOST_BIN"
  elif ! "$HOST_BIN" --headless --dump-png "$FIXTURE_BASIC" /tmp/stele_a3e.png 2>/tmp/stele_a3e.err; then
    bad "A3e: stele --headless --dump-png crashed on $FIXTURE_BASIC"
    sed 's/^/    /' /tmp/stele_a3e.err
  elif [ "$BLESS" = 1 ]; then
    cp /tmp/stele_a3e.png "$GOLDEN_PNG"
    pass "A3e: blessed PNG golden -> $GOLDEN_PNG (never bless your own render blind — see brief §10)"
  elif [ ! -f "$GOLDEN_PNG" ]; then
    bad "A3e: no golden at $GOLDEN_PNG to compare against (run with --bless once accepted)"
  elif cmp -s "$GOLDEN_PNG" /tmp/stele_a3e.png; then
    pass "A3e: PNG dump of $FIXTURE_BASIC matches golden"
  else
    bad "A3e: PNG dump of $FIXTURE_BASIC differs from $GOLDEN_PNG"
    note "sizes: golden=$(wc -c < "$GOLDEN_PNG") actual=$(wc -c < /tmp/stele_a3e.png)"
  fi

  # A3f — the images packet's own PNG golden (M4): THE SCREENSHOT.
  # fixtures/images.html has real <img src> elements resolved against sibling
  # files on disk (a PNG, a JPEG, a GIF, an animated GIF, plus — as of the M4
  # floats + inline images packet — a floated `img align=left` with wrapping
  # text and a non-floated inline `<img>`), so — unlike A3e's
  # fixtures/basic.html, which has none — this exercises the real image
  # fetch+decode pre-pass (images::collect_images) end to end, not just
  # layout/paint. Same blessing discipline, same byte-equality rationale as
  # A3e (encode_png is deterministic; tests/images_golden.rs does the real
  # decode-and-compare-pixels check).
  GOLDEN_PNG_IMAGES="goldens/images.png"
  FIXTURE_IMAGES="fixtures/images.html"
  if [ ! -f "$HOST_BIN" ]; then
    bad "A3f: host binary still not found at $HOST_BIN"
  elif ! "$HOST_BIN" --headless --dump-png "$FIXTURE_IMAGES" /tmp/stele_a3f.png 2>/tmp/stele_a3f.err; then
    bad "A3f: stele --headless --dump-png crashed on $FIXTURE_IMAGES"
    sed 's/^/    /' /tmp/stele_a3f.err
  elif [ "$BLESS" = 1 ]; then
    cp /tmp/stele_a3f.png "$GOLDEN_PNG_IMAGES"
    pass "A3f: blessed PNG golden -> $GOLDEN_PNG_IMAGES (never bless your own render blind — see brief §10)"
  elif [ ! -f "$GOLDEN_PNG_IMAGES" ]; then
    bad "A3f: no golden at $GOLDEN_PNG_IMAGES to compare against (run with --bless once accepted)"
  elif cmp -s "$GOLDEN_PNG_IMAGES" /tmp/stele_a3f.png; then
    pass "A3f: PNG dump of $FIXTURE_IMAGES matches golden"
  else
    bad "A3f: PNG dump of $FIXTURE_IMAGES differs from $GOLDEN_PNG_IMAGES"
    note "sizes: golden=$(wc -c < "$GOLDEN_PNG_IMAGES") actual=$(wc -c < /tmp/stele_a3f.png)"
  fi

  # A3g -- the flex-polite packet's own PNG golden (M5): a modern no-JS blog
  # layout styled entirely via author CSS (`<style>` block: `display: flex`,
  # `justify-content`, `align-items`, `flex-grow`, `gap`, a fixed-width
  # sidebar), exercising author-CSS-driven flexbox end to end for the first
  # time (previous PNG goldens, A3e/A3f, have no flex in them at all). Same
  # blessing discipline, same byte-equality rationale as A3e/A3f
  # (encode_png is deterministic; tests/flex_polite_golden.rs does the real
  # decode-and-compare-pixels check, plus geometry assertions proving the
  # flex actually took effect -- nav right of title, article wider than the
  # fixed-width aside).
  GOLDEN_PNG_FLEX="goldens/flex-polite.png"
  FIXTURE_FLEX="fixtures/flex-polite.html"
  if [ ! -f "$HOST_BIN" ]; then
    bad "A3g: host binary still not found at $HOST_BIN"
  elif ! "$HOST_BIN" --headless --dump-png "$FIXTURE_FLEX" /tmp/stele_a3g.png 2>/tmp/stele_a3g.err; then
    bad "A3g: stele --headless --dump-png crashed on $FIXTURE_FLEX"
    sed 's/^/    /' /tmp/stele_a3g.err
  elif [ "$BLESS" = 1 ]; then
    cp /tmp/stele_a3g.png "$GOLDEN_PNG_FLEX"
    pass "A3g: blessed PNG golden -> $GOLDEN_PNG_FLEX (never bless your own render blind — see brief §10)"
  elif [ ! -f "$GOLDEN_PNG_FLEX" ]; then
    bad "A3g: no golden at $GOLDEN_PNG_FLEX to compare against (run with --bless once accepted)"
  elif cmp -s "$GOLDEN_PNG_FLEX" /tmp/stele_a3g.png; then
    pass "A3g: PNG dump of $FIXTURE_FLEX matches golden"
  else
    bad "A3g: PNG dump of $FIXTURE_FLEX differs from $GOLDEN_PNG_FLEX"
    note "sizes: golden=$(wc -c < "$GOLDEN_PNG_FLEX") actual=$(wc -c < /tmp/stele_a3g.png)"
  fi

  # A3h/A3i -- the @media packet's own tty goldens (M5): fixtures/media-
  # query.html rendered at TWO widths through the real --dump-text pipeline
  # (which now flattens @media against `cols * 8px` before cascade runs --
  # see style::media::flatten_media / style::collect_author_sheets_for_
  # viewport). A3h is the default-80-cols/640px dump (the `(max-width:
  # 500px)` query does NOT match: sidebar visible, narrow-notice absent);
  # A3i is the `--cols 40`/320px dump (the query DOES match: sidebar
  # hidden, narrow-notice visible) -- together they're the one live proof
  # in this shell-level acceptance script that @media actually responds to
  # the viewport, not just a hardcoded Rust unit test. Same blessing
  # discipline as every other tty golden here.
  GOLDEN_TTY_MEDIA_WIDE="goldens/media-query-wide.tty.txt"
  GOLDEN_TTY_MEDIA_NARROW="goldens/media-query-narrow.tty.txt"
  FIXTURE_MEDIA="fixtures/media-query.html"
  if [ ! -f "$HOST_BIN" ]; then
    bad "A3h: host binary still not found at $HOST_BIN"
  elif ! out_media_wide="$("$HOST_BIN" --headless --dump-text "$FIXTURE_MEDIA" 2>/tmp/stele_a3h.err)"; then
    bad "A3h: stele --headless --dump-text crashed on $FIXTURE_MEDIA"
    sed 's/^/    /' /tmp/stele_a3h.err
  elif [ "$BLESS" = 1 ]; then
    printf '%s\n' "$out_media_wide" > "$GOLDEN_TTY_MEDIA_WIDE"
    pass "A3h: blessed media-query WIDE tty golden -> $GOLDEN_TTY_MEDIA_WIDE (never bless your own render blind — see brief §10)"
  elif diff -u "$GOLDEN_TTY_MEDIA_WIDE" <(printf '%s\n' "$out_media_wide") >/tmp/stele_a3h.diff 2>&1; then
    pass "A3h: tty dump of $FIXTURE_MEDIA at 80 cols (640px, query does not match) matches golden"
  else
    bad "A3h: tty dump of $FIXTURE_MEDIA at 80 cols differs from $GOLDEN_TTY_MEDIA_WIDE"
    sed 's/^/    /' /tmp/stele_a3h.diff
  fi

  if [ ! -f "$HOST_BIN" ]; then
    bad "A3i: host binary still not found at $HOST_BIN"
  elif ! out_media_narrow="$("$HOST_BIN" --headless --dump-text "$FIXTURE_MEDIA" --cols 40 2>/tmp/stele_a3i.err)"; then
    bad "A3i: stele --headless --dump-text crashed on $FIXTURE_MEDIA --cols 40"
    sed 's/^/    /' /tmp/stele_a3i.err
  elif [ "$BLESS" = 1 ]; then
    printf '%s\n' "$out_media_narrow" > "$GOLDEN_TTY_MEDIA_NARROW"
    pass "A3i: blessed media-query NARROW tty golden -> $GOLDEN_TTY_MEDIA_NARROW (never bless your own render blind — see brief §10)"
  elif diff -u "$GOLDEN_TTY_MEDIA_NARROW" <(printf '%s\n' "$out_media_narrow") >/tmp/stele_a3i.diff 2>&1; then
    pass "A3i: tty dump of $FIXTURE_MEDIA at 40 cols (320px, query matches) matches golden"
  else
    bad "A3i: tty dump of $FIXTURE_MEDIA at 40 cols differs from $GOLDEN_TTY_MEDIA_NARROW"
    sed 's/^/    /' /tmp/stele_a3i.diff
  fi

  # A3j -- the dialect-completeness packet's own tty golden (M5):
  # fixtures/details.html, exercising <details>/<summary> disclosure
  # collapse (a collapsed <details> shows only its markered summary; a
  # <details open> shows the summary AND its body). Same blessing
  # discipline as every other tty golden here.
  GOLDEN_TTY_DETAILS="goldens/details.tty.txt"
  FIXTURE_DETAILS="fixtures/details.html"
  if [ ! -f "$HOST_BIN" ]; then
    bad "A3j: host binary still not found at $HOST_BIN"
  elif ! out_details="$("$HOST_BIN" --headless --dump-text "$FIXTURE_DETAILS" 2>/tmp/stele_a3j.err)"; then
    bad "A3j: stele --headless --dump-text crashed on $FIXTURE_DETAILS"
    sed 's/^/    /' /tmp/stele_a3j.err
  elif [ "$BLESS" = 1 ]; then
    printf '%s\n' "$out_details" > "$GOLDEN_TTY_DETAILS"
    pass "A3j: blessed details tty golden -> $GOLDEN_TTY_DETAILS (never bless your own render blind — see brief §10)"
  elif diff -u "$GOLDEN_TTY_DETAILS" <(printf '%s\n' "$out_details") >/tmp/stele_a3j.diff 2>&1; then
    pass "A3j: tty dump of $FIXTURE_DETAILS matches golden"
  else
    bad "A3j: tty dump of $FIXTURE_DETAILS differs from $GOLDEN_TTY_DETAILS"
    sed 's/^/    /' /tmp/stele_a3j.diff
  fi

  # A3k -- the dialect-completeness packet's own tty golden (M5):
  # fixtures/noscript.html, confirming <noscript> content actually renders
  # (Stele has no JavaScript by construction, so <noscript> is precisely
  # "what to show when scripting is unavailable" -- always, here). Same
  # blessing discipline.
  GOLDEN_TTY_NOSCRIPT="goldens/noscript.tty.txt"
  FIXTURE_NOSCRIPT="fixtures/noscript.html"
  if [ ! -f "$HOST_BIN" ]; then
    bad "A3k: host binary still not found at $HOST_BIN"
  elif ! out_noscript="$("$HOST_BIN" --headless --dump-text "$FIXTURE_NOSCRIPT" 2>/tmp/stele_a3k.err)"; then
    bad "A3k: stele --headless --dump-text crashed on $FIXTURE_NOSCRIPT"
    sed 's/^/    /' /tmp/stele_a3k.err
  elif [ "$BLESS" = 1 ]; then
    printf '%s\n' "$out_noscript" > "$GOLDEN_TTY_NOSCRIPT"
    pass "A3k: blessed noscript tty golden -> $GOLDEN_TTY_NOSCRIPT (never bless your own render blind — see brief §10)"
  elif diff -u "$GOLDEN_TTY_NOSCRIPT" <(printf '%s\n' "$out_noscript") >/tmp/stele_a3k.diff 2>&1; then
    pass "A3k: tty dump of $FIXTURE_NOSCRIPT matches golden"
  else
    bad "A3k: tty dump of $FIXTURE_NOSCRIPT differs from $GOLDEN_TTY_NOSCRIPT"
    sed 's/^/    /' /tmp/stele_a3k.diff
  fi

  # A3l -- the dialect-completeness packet's own tty golden (M5):
  # fixtures/entities.html, locking in HTML entity decoding coverage (named,
  # numeric, hex numeric, and an unrecognized entity passing through
  # literally) through the real render pipeline. Same blessing discipline.
  GOLDEN_TTY_ENTITIES="goldens/entities.tty.txt"
  FIXTURE_ENTITIES="fixtures/entities.html"
  if [ ! -f "$HOST_BIN" ]; then
    bad "A3l: host binary still not found at $HOST_BIN"
  elif ! out_entities="$("$HOST_BIN" --headless --dump-text "$FIXTURE_ENTITIES" 2>/tmp/stele_a3l.err)"; then
    bad "A3l: stele --headless --dump-text crashed on $FIXTURE_ENTITIES"
    sed 's/^/    /' /tmp/stele_a3l.err
  elif [ "$BLESS" = 1 ]; then
    printf '%s\n' "$out_entities" > "$GOLDEN_TTY_ENTITIES"
    pass "A3l: blessed entities tty golden -> $GOLDEN_TTY_ENTITIES (never bless your own render blind — see brief §10)"
  elif diff -u "$GOLDEN_TTY_ENTITIES" <(printf '%s\n' "$out_entities") >/tmp/stele_a3l.diff 2>&1; then
    pass "A3l: tty dump of $FIXTURE_ENTITIES matches golden"
  else
    bad "A3l: tty dump of $FIXTURE_ENTITIES differs from $GOLDEN_TTY_ENTITIES"
    sed 's/^/    /' /tmp/stele_a3l.diff
  fi

  # A3m -- the list-markers packet's own tty golden (M6):
  # fixtures/lists.html, exercising `<ul>`/`<ol>`/`<li>` marker synthesis
  # (bullet + decimal markers, per-list ordinal counting across a nested
  # list, `list-style-type: none`, and `<ol start>`). Same blessing
  # discipline as every other tty golden here.
  GOLDEN_TTY_LISTS="goldens/lists.tty.txt"
  FIXTURE_LISTS="fixtures/lists.html"
  if [ ! -f "$HOST_BIN" ]; then
    bad "A3m: host binary still not found at $HOST_BIN"
  elif ! out_lists="$("$HOST_BIN" --headless --dump-text "$FIXTURE_LISTS" 2>/tmp/stele_a3m.err)"; then
    bad "A3m: stele --headless --dump-text crashed on $FIXTURE_LISTS"
    sed 's/^/    /' /tmp/stele_a3m.err
  elif [ "$BLESS" = 1 ]; then
    printf '%s\n' "$out_lists" > "$GOLDEN_TTY_LISTS"
    pass "A3m: blessed lists tty golden -> $GOLDEN_TTY_LISTS (never bless your own render blind — see brief §10)"
  elif diff -u "$GOLDEN_TTY_LISTS" <(printf '%s\n' "$out_lists") >/tmp/stele_a3m.diff 2>&1; then
    pass "A3m: tty dump of $FIXTURE_LISTS matches golden"
  else
    bad "A3m: tty dump of $FIXTURE_LISTS differs from $GOLDEN_TTY_LISTS"
    sed 's/^/    /' /tmp/stele_a3m.diff
  fi

  # A3n -- the block-in-inline packet's own tty golden: fixtures/
  # block-in-inline.html, exercising CSS "block-in-inline" resolution -- a
  # block-level `<ol>`/`<li>` nested inside an inline-display `<font>`
  # wrapper must still render as separate stacked block boxes (each list
  # item on its own line, with its own marker), not get folded whole into
  # the inline formatting context and run together on one line. Confirmed
  # real-world breakage: http://68k.news/ wraps every news list in
  # `<font size="4"><ol><li>...`. Same blessing discipline as every other
  # tty golden here.
  GOLDEN_TTY_BLOCK_IN_INLINE="goldens/block-in-inline.txt"
  FIXTURE_BLOCK_IN_INLINE="fixtures/block-in-inline.html"
  if [ ! -f "$HOST_BIN" ]; then
    bad "A3n: host binary still not found at $HOST_BIN"
  elif ! out_block_in_inline="$("$HOST_BIN" --headless --dump-text "$FIXTURE_BLOCK_IN_INLINE" 2>/tmp/stele_a3n.err)"; then
    bad "A3n: stele --headless --dump-text crashed on $FIXTURE_BLOCK_IN_INLINE"
    sed 's/^/    /' /tmp/stele_a3n.err
  elif [ "$BLESS" = 1 ]; then
    printf '%s\n' "$out_block_in_inline" > "$GOLDEN_TTY_BLOCK_IN_INLINE"
    pass "A3n: blessed block-in-inline tty golden -> $GOLDEN_TTY_BLOCK_IN_INLINE (never bless your own render blind — see brief §10)"
  elif diff -u "$GOLDEN_TTY_BLOCK_IN_INLINE" <(printf '%s\n' "$out_block_in_inline") >/tmp/stele_a3n.diff 2>&1; then
    pass "A3n: tty dump of $FIXTURE_BLOCK_IN_INLINE matches golden"
  else
    bad "A3n: tty dump of $FIXTURE_BLOCK_IN_INLINE differs from $GOLDEN_TTY_BLOCK_IN_INLINE"
    sed 's/^/    /' /tmp/stele_a3n.diff
  fi

  # A3o/A3p -- the presentational-attrs packet's own goldens: fixtures/
  # presentational.html exercises vintage HTML presentational attributes
  # (<font size color>, bgcolor, align=, <body text>) mapped onto computed
  # style through the new presentational-hint cascade tier
  # (style::value::presentational_hints / style::cascade's UA < hint <
  # author tiers), NOT a post-cascade mutation. A3o is the tty dump (proves
  # the centering/right-alignment took effect); A3p is the PNG dump (proves
  # the purple/red text colors, the pale-yellow background, and the larger
  # heading font size -- none of which the tty dump can show). Same
  # blessing discipline as every other golden here: these are PROPOSED
  # goldens, not self-blessed -- the orchestrator/reviewer visually
  # inspects goldens/presentational.png and countersigns before either is
  # trusted.
  GOLDEN_TTY_PRESENTATIONAL="goldens/presentational.tty.txt"
  FIXTURE_PRESENTATIONAL="fixtures/presentational.html"
  if [ ! -f "$HOST_BIN" ]; then
    bad "A3o: host binary still not found at $HOST_BIN"
  elif ! out_presentational="$("$HOST_BIN" --headless --dump-text "$FIXTURE_PRESENTATIONAL" 2>/tmp/stele_a3o.err)"; then
    bad "A3o: stele --headless --dump-text crashed on $FIXTURE_PRESENTATIONAL"
    sed 's/^/    /' /tmp/stele_a3o.err
  elif [ "$BLESS" = 1 ]; then
    printf '%s\n' "$out_presentational" > "$GOLDEN_TTY_PRESENTATIONAL"
    pass "A3o: blessed presentational tty golden -> $GOLDEN_TTY_PRESENTATIONAL (never bless your own render blind — see brief §10)"
  elif diff -u "$GOLDEN_TTY_PRESENTATIONAL" <(printf '%s\n' "$out_presentational") >/tmp/stele_a3o.diff 2>&1; then
    pass "A3o: tty dump of $FIXTURE_PRESENTATIONAL matches golden"
  else
    bad "A3o: tty dump of $FIXTURE_PRESENTATIONAL differs from $GOLDEN_TTY_PRESENTATIONAL"
    sed 's/^/    /' /tmp/stele_a3o.diff
  fi

  GOLDEN_PNG_PRESENTATIONAL="goldens/presentational.png"
  if [ ! -f "$HOST_BIN" ]; then
    bad "A3p: host binary still not found at $HOST_BIN"
  elif ! "$HOST_BIN" --headless --dump-png "$FIXTURE_PRESENTATIONAL" /tmp/stele_a3p.png 2>/tmp/stele_a3p.err; then
    bad "A3p: stele --headless --dump-png crashed on $FIXTURE_PRESENTATIONAL"
    sed 's/^/    /' /tmp/stele_a3p.err
  elif [ "$BLESS" = 1 ]; then
    cp /tmp/stele_a3p.png "$GOLDEN_PNG_PRESENTATIONAL"
    pass "A3p: blessed presentational PNG golden -> $GOLDEN_PNG_PRESENTATIONAL (never bless your own render blind — see brief §10)"
  elif [ ! -f "$GOLDEN_PNG_PRESENTATIONAL" ]; then
    bad "A3p: no golden at $GOLDEN_PNG_PRESENTATIONAL to compare against (run with --bless once accepted)"
  elif cmp -s "$GOLDEN_PNG_PRESENTATIONAL" /tmp/stele_a3p.png; then
    pass "A3p: PNG dump of $FIXTURE_PRESENTATIONAL matches golden"
  else
    bad "A3p: PNG dump of $FIXTURE_PRESENTATIONAL differs from $GOLDEN_PNG_PRESENTATIONAL"
    note "sizes: golden=$(wc -c < "$GOLDEN_PNG_PRESENTATIONAL") actual=$(wc -c < /tmp/stele_a3p.png)"
  fi

  # A3q -- packet T1c's contrast covenant: `--audit-contrast <file>` is a
  # DEFENSE-IN-DEPTH GATE, not the repair itself -- backend::raster::paint
  # already repairs every text run's foreground color against its own
  # analytically-resolved effective background before it ever reaches this
  # check (see style::contrast's own doc comments). A correct implementation
  # reports ZERO violations on every fixture here: style::contrast::
  # repair_fg guarantees at least one of black/white always clears
  # CONTRAST_MIN against ANY background (see that function's own doc
  # comment) -- so a NONZERO exit below signals a REGRESSION in repair_fg/
  # effective_background, not an expected/normal outcome. `--bless` has
  # nothing to bless for an audit gate (no golden render output), so this
  # block is a deliberate no-op under --bless (composes harmlessly, per
  # this packet's own brief).
  #
  # NOTE (scope, future packet): this audits RAW, pre-quantization computed
  # colors -- it does NOT re-check contrast after `backend::fb::
  # convert_to_fb_bytes`'s palette quantization (the `--render-fb`
  # framebuffer path), which could in principle nudge a borderline-
  # compliant color across the threshold on very low-color-depth hardware.
  # Auditing post-quantization is a reasonable follow-up, not this packet's
  # job (brief scope).
  # t1d-httpforever extends this corpus with fixtures/httpforever.html --
  # the real-world dark-theme fidelity fixture (see fixtures/evidence/
  # README.md's own "httpforever" section) -- run in BOTH its default
  # (light) rendering AND, in a second independent loop below, under
  # `--color-scheme dark` (that page's dark theme is reachable ONLY via
  # that flag -- no `prefers-color-scheme` fallback in its own CSS). This is
  # the proof this packet's brief asks for: the T1a/T1b/T1c stack (var(),
  # --color-scheme, and the contrast-repair covenant) actually holds on a
  # dense, unmodified, real-world page in EITHER theme, not just the
  # synthetic fixtures the earlier packets shipped their own tests against.
  if [ "$BLESS" = 1 ]; then
    pend "A3q: --audit-contrast has no golden to bless (defense-in-depth gate, not a render) -- skipped under --bless"
  elif [ ! -f "$HOST_BIN" ]; then
    bad "A3q: host binary still not found at $HOST_BIN"
  else
    audit_fail=0
    for f in fixtures/basic.html fixtures/images.html fixtures/kitchen-sink.html fixtures/presentational.html fixtures/table-border.html fixtures/table-spacing.html fixtures/httpforever.html; do
      if ! out_audit="$("$HOST_BIN" --headless --audit-contrast "$f" 2>&1)"; then
        bad "A3q: --audit-contrast reported a violation on $f"
        sed 's/^/    /' <<< "$out_audit"
        audit_fail=1
      fi
    done
    if ! out_audit_dark="$("$HOST_BIN" --headless --color-scheme dark --audit-contrast fixtures/httpforever.html 2>&1)"; then
      bad "A3q: --audit-contrast --color-scheme dark reported a violation on fixtures/httpforever.html"
      sed 's/^/    /' <<< "$out_audit_dark"
      audit_fail=1
    fi
    if [ "$audit_fail" = 0 ]; then
      pass "A3q: --audit-contrast reports zero violations across the fixture corpus (including httpforever.html light AND dark)"
    fi
  fi

  # ---------------------------------------------------------------------
  # A3r/A3s -- packet t1d-httpforever's own PNG goldens: fixtures/
  # httpforever.html, Stele's canonical dark-theme fidelity fixture (see
  # fixtures/evidence/README.md's "httpforever" section for the full
  # provenance/theme-mechanism writeup). A3r is the LIGHT (default, no
  # `--color-scheme` flag) render; A3s is the SAME fixture under
  # `--color-scheme dark` -- the only way to reach this real-world page's
  # dark theme through Stele at all, since its own dark mode is gated
  # entirely on a JS-toggled `html[data-theme="dark"]` with no
  # `prefers-color-scheme` fallback (packet t1d-httpforever wired
  # `--dump-png` through to `--color-scheme` for exactly this fixture --
  # see `dump_png_opts`'s own doc comment in src/main.rs). Same blessing
  # discipline, same byte-equality rationale as every other PNG golden here
  # (A3e/A3f/A3g/A3p).
  GOLDEN_PNG_HTTPFOREVER_LIGHT="goldens/httpforever.light.png"
  GOLDEN_PNG_HTTPFOREVER_DARK="goldens/httpforever.dark.png"
  FIXTURE_HTTPFOREVER="fixtures/httpforever.html"
  if [ ! -f "$HOST_BIN" ]; then
    bad "A3r: host binary still not found at $HOST_BIN"
  elif ! "$HOST_BIN" --headless --dump-png "$FIXTURE_HTTPFOREVER" /tmp/stele_a3r.png 2>/tmp/stele_a3r.err; then
    bad "A3r: stele --headless --dump-png crashed on $FIXTURE_HTTPFOREVER"
    sed 's/^/    /' /tmp/stele_a3r.err
  elif [ "$BLESS" = 1 ]; then
    cp /tmp/stele_a3r.png "$GOLDEN_PNG_HTTPFOREVER_LIGHT"
    pass "A3r: blessed httpforever LIGHT PNG golden -> $GOLDEN_PNG_HTTPFOREVER_LIGHT (never bless your own render blind — see brief §10)"
  elif [ ! -f "$GOLDEN_PNG_HTTPFOREVER_LIGHT" ]; then
    bad "A3r: no golden at $GOLDEN_PNG_HTTPFOREVER_LIGHT to compare against (run with --bless once accepted)"
  elif cmp -s "$GOLDEN_PNG_HTTPFOREVER_LIGHT" /tmp/stele_a3r.png; then
    pass "A3r: PNG dump of $FIXTURE_HTTPFOREVER (light) matches golden"
  else
    bad "A3r: PNG dump of $FIXTURE_HTTPFOREVER (light) differs from $GOLDEN_PNG_HTTPFOREVER_LIGHT"
    note "sizes: golden=$(wc -c < "$GOLDEN_PNG_HTTPFOREVER_LIGHT") actual=$(wc -c < /tmp/stele_a3r.png)"
  fi

  if [ ! -f "$HOST_BIN" ]; then
    bad "A3s: host binary still not found at $HOST_BIN"
  elif ! "$HOST_BIN" --headless --color-scheme dark --dump-png "$FIXTURE_HTTPFOREVER" /tmp/stele_a3s.png 2>/tmp/stele_a3s.err; then
    bad "A3s: stele --headless --color-scheme dark --dump-png crashed on $FIXTURE_HTTPFOREVER"
    sed 's/^/    /' /tmp/stele_a3s.err
  elif [ "$BLESS" = 1 ]; then
    cp /tmp/stele_a3s.png "$GOLDEN_PNG_HTTPFOREVER_DARK"
    pass "A3s: blessed httpforever DARK PNG golden -> $GOLDEN_PNG_HTTPFOREVER_DARK (never bless your own render blind — see brief §10)"
  elif [ ! -f "$GOLDEN_PNG_HTTPFOREVER_DARK" ]; then
    bad "A3s: no golden at $GOLDEN_PNG_HTTPFOREVER_DARK to compare against (run with --bless once accepted)"
  elif cmp -s "$GOLDEN_PNG_HTTPFOREVER_DARK" /tmp/stele_a3s.png; then
    pass "A3s: PNG dump of $FIXTURE_HTTPFOREVER (dark) matches golden"
  else
    bad "A3s: PNG dump of $FIXTURE_HTTPFOREVER (dark) differs from $GOLDEN_PNG_HTTPFOREVER_DARK"
    note "sizes: golden=$(wc -c < "$GOLDEN_PNG_HTTPFOREVER_DARK") actual=$(wc -c < /tmp/stele_a3s.png)"
  fi

  # ---------------------------------------------------------------------
  # A3t/A3u -- packet t4-button-honesty's OWN fixture (fixtures/
  # controls.html, NOT httpforever.html -- that fixture's goldens are
  # deliberately left untouched here and re-blessed in a later
  # consolidation step once every T4-era label change has landed).
  # controls.html exercises every rung of `box_tree::resolve_bracket_label`'s
  # label-resolution order (own text, `value`, `aria-label`, `title`, the
  # submit-only "Submit" default, honest-empty `[ ]`) in one document,
  # including the exact D4 repro shape: an icon-only `<button type=button
  # aria-label="Theme">` with no text/value, which must render `[ Theme ]`,
  # never a fabricated `[ Submit ]`. A3t is the tty dump (same discipline as
  # A3/A3b/A3c/A3d); A3u is the PNG dump (same byte-equality rationale as
  # A3e/A3f/.../A3s).
  # ---------------------------------------------------------------------
  GOLDEN_TTY_CONTROLS="goldens/controls.tty.txt"
  FIXTURE_CONTROLS="fixtures/controls.html"
  if [ ! -f "$HOST_BIN" ]; then
    bad "A3t: host binary still not found at $HOST_BIN"
  elif ! out_controls="$("$HOST_BIN" --headless --dump-text "$FIXTURE_CONTROLS" 2>/tmp/stele_a3t.err)"; then
    bad "A3t: stele --headless --dump-text crashed on $FIXTURE_CONTROLS"
    sed 's/^/    /' /tmp/stele_a3t.err
  elif [ "$BLESS" = 1 ]; then
    printf '%s\n' "$out_controls" > "$GOLDEN_TTY_CONTROLS"
    pass "A3t: blessed controls tty golden -> $GOLDEN_TTY_CONTROLS (never bless your own render blind — see brief §10)"
  elif diff -u "$GOLDEN_TTY_CONTROLS" <(printf '%s\n' "$out_controls") >/tmp/stele_a3t.diff 2>&1; then
    pass "A3t: tty dump of $FIXTURE_CONTROLS matches golden"
  else
    bad "A3t: tty dump of $FIXTURE_CONTROLS differs from $GOLDEN_TTY_CONTROLS"
    sed 's/^/    /' /tmp/stele_a3t.diff
  fi

  GOLDEN_PNG_CONTROLS="goldens/controls.png"
  if [ ! -f "$HOST_BIN" ]; then
    bad "A3u: host binary still not found at $HOST_BIN"
  elif ! "$HOST_BIN" --headless --dump-png "$FIXTURE_CONTROLS" /tmp/stele_a3u.png 2>/tmp/stele_a3u.err; then
    bad "A3u: stele --headless --dump-png crashed on $FIXTURE_CONTROLS"
    sed 's/^/    /' /tmp/stele_a3u.err
  elif [ "$BLESS" = 1 ]; then
    cp /tmp/stele_a3u.png "$GOLDEN_PNG_CONTROLS"
    pass "A3u: blessed controls PNG golden -> $GOLDEN_PNG_CONTROLS (never bless your own render blind — see brief §10)"
  elif [ ! -f "$GOLDEN_PNG_CONTROLS" ]; then
    bad "A3u: no golden at $GOLDEN_PNG_CONTROLS to compare against (run with --bless once accepted)"
  elif cmp -s "$GOLDEN_PNG_CONTROLS" /tmp/stele_a3u.png; then
    pass "A3u: PNG dump of $FIXTURE_CONTROLS matches golden"
  else
    bad "A3u: PNG dump of $FIXTURE_CONTROLS differs from $GOLDEN_PNG_CONTROLS"
    note "sizes: golden=$(wc -c < "$GOLDEN_PNG_CONTROLS") actual=$(wc -c < /tmp/stele_a3u.png)"
  fi

  # ---------------------------------------------------------------------
  # A3v/A3w -- packet t2-glyph-fallback's own goldens: fixtures/
  # punctuation.html exercises em/en dash, curly quotes, ellipsis, bullet,
  # nbsp, the multiplication sign, a right arrow, and (to prove the
  # skip-and-count default actually skips rather than tofu-ing) an emoji and
  # a CJK pair -- see `text::translit`'s own module doc for the atlas +
  # transliteration resolution order this fixture is meant to exercise. A3v
  # is the tty dump (same discipline as A3/A3b/.../A3s's own tty blocks
  # above); A3w is the PNG dump (same byte-equality rationale as A3e/A3f/
  # .../A3s's own PNG blocks). Same host binary, same blessing discipline,
  # same independent block so a failure/bless of one doesn't short-circuit
  # the other. (Numbered A3v/A3w, not A3t/A3u, to avoid colliding with
  # packet t4-button-honesty's own controls.html checks right above, which
  # landed on main first and already claimed A3t/A3u.)
  GOLDEN_TTY_PUNCTUATION="goldens/punctuation.tty.txt"
  GOLDEN_PNG_PUNCTUATION="goldens/punctuation.png"
  FIXTURE_PUNCTUATION="fixtures/punctuation.html"
  if [ ! -f "$HOST_BIN" ]; then
    bad "A3v: host binary still not found at $HOST_BIN"
  elif ! out_punctuation="$("$HOST_BIN" --headless --dump-text "$FIXTURE_PUNCTUATION" 2>/tmp/stele_a3v.err)"; then
    bad "A3v: stele --headless --dump-text crashed on $FIXTURE_PUNCTUATION"
    sed 's/^/    /' /tmp/stele_a3v.err
  elif [ "$BLESS" = 1 ]; then
    printf '%s\n' "$out_punctuation" > "$GOLDEN_TTY_PUNCTUATION"
    pass "A3v: blessed punctuation tty golden -> $GOLDEN_TTY_PUNCTUATION (never bless your own render blind — see brief §10)"
  elif diff -u "$GOLDEN_TTY_PUNCTUATION" <(printf '%s\n' "$out_punctuation") >/tmp/stele_a3v.diff 2>&1; then
    pass "A3v: tty dump of $FIXTURE_PUNCTUATION matches golden"
  else
    bad "A3v: tty dump of $FIXTURE_PUNCTUATION differs from $GOLDEN_TTY_PUNCTUATION"
    sed 's/^/    /' /tmp/stele_a3v.diff
  fi

  if [ ! -f "$HOST_BIN" ]; then
    bad "A3w: host binary still not found at $HOST_BIN"
  elif ! "$HOST_BIN" --headless --dump-png "$FIXTURE_PUNCTUATION" /tmp/stele_a3w.png 2>/tmp/stele_a3w.err; then
    bad "A3w: stele --headless --dump-png crashed on $FIXTURE_PUNCTUATION"
    sed 's/^/    /' /tmp/stele_a3w.err
  elif [ "$BLESS" = 1 ]; then
    cp /tmp/stele_a3w.png "$GOLDEN_PNG_PUNCTUATION"
    pass "A3w: blessed punctuation PNG golden -> $GOLDEN_PNG_PUNCTUATION (never bless your own render blind — see brief §10)"
  elif [ ! -f "$GOLDEN_PNG_PUNCTUATION" ]; then
    bad "A3w: no golden at $GOLDEN_PNG_PUNCTUATION to compare against (run with --bless once accepted)"
  elif cmp -s "$GOLDEN_PNG_PUNCTUATION" /tmp/stele_a3w.png; then
    pass "A3w: PNG dump of $FIXTURE_PUNCTUATION matches golden"
  else
    bad "A3w: PNG dump of $FIXTURE_PUNCTUATION differs from $GOLDEN_PNG_PUNCTUATION"
    note "sizes: golden=$(wc -c < "$GOLDEN_PNG_PUNCTUATION") actual=$(wc -c < /tmp/stele_a3w.png)"
  fi

  # ---------------------------------------------------------------------
  # A3x/A3y -- packet t3-inline-spacing's own goldens (fixtures/
  # inline-spacing.html, the D3 fix), three flex rows plus two inline-run
  # shapes:
  #   1. a real, correctly two-value-parsed px `gap` (must NOT get any
  #      synthesized extra space -- the real 20px column-gap is already
  #      enough);
  #   2. `gap: .35rem 1.1rem`, the EXACT shape of fixtures/httpforever.
  #      html's `.footer__projects` -- resolved via `gap`'s own scoped
  #      `rem` ≈ `em` approximation (`value::token_to_gap_length`), so this
  #      is also a real, non-synthesized gap;
  #   3. no `gap` declared at all (CSS initial value 0) -- separated ONLY
  #      by the zero-advance synthesis rule, the independent safety net
  #      that still applies when a gap can't be resolved at all;
  #   4. an intra-word inline split (`<b>bo</b>ld`, must stay "bold");
  #   5. adjacent inline elements with no source whitespace
  #      (`a<span>b</span>c`, must stay "abc").
  # A3x is the tty dump (same discipline as A3/A3b/.../A3v); A3y is the PNG
  # dump (same byte-equality rationale as A3e/A3f/.../A3w).
  # ---------------------------------------------------------------------
  GOLDEN_TTY_INLINE_SPACING="goldens/inline-spacing.tty.txt"
  GOLDEN_PNG_INLINE_SPACING="goldens/inline-spacing.png"
  FIXTURE_INLINE_SPACING="fixtures/inline-spacing.html"
  if [ ! -f "$HOST_BIN" ]; then
    bad "A3x: host binary still not found at $HOST_BIN"
  elif ! out_inline_spacing="$("$HOST_BIN" --headless --dump-text "$FIXTURE_INLINE_SPACING" 2>/tmp/stele_a3x.err)"; then
    bad "A3x: stele --headless --dump-text crashed on $FIXTURE_INLINE_SPACING"
    sed 's/^/    /' /tmp/stele_a3x.err
  elif [ "$BLESS" = 1 ]; then
    printf '%s\n' "$out_inline_spacing" > "$GOLDEN_TTY_INLINE_SPACING"
    pass "A3x: blessed inline-spacing tty golden -> $GOLDEN_TTY_INLINE_SPACING (never bless your own render blind — see brief §10)"
  elif diff -u "$GOLDEN_TTY_INLINE_SPACING" <(printf '%s\n' "$out_inline_spacing") >/tmp/stele_a3x.diff 2>&1; then
    pass "A3x: tty dump of $FIXTURE_INLINE_SPACING matches golden"
  else
    bad "A3x: tty dump of $FIXTURE_INLINE_SPACING differs from $GOLDEN_TTY_INLINE_SPACING"
    sed 's/^/    /' /tmp/stele_a3x.diff
  fi

  if [ ! -f "$HOST_BIN" ]; then
    bad "A3y: host binary still not found at $HOST_BIN"
  elif ! "$HOST_BIN" --headless --dump-png "$FIXTURE_INLINE_SPACING" /tmp/stele_a3y.png 2>/tmp/stele_a3y.err; then
    bad "A3y: stele --headless --dump-png crashed on $FIXTURE_INLINE_SPACING"
    sed 's/^/    /' /tmp/stele_a3y.err
  elif [ "$BLESS" = 1 ]; then
    cp /tmp/stele_a3y.png "$GOLDEN_PNG_INLINE_SPACING"
    pass "A3y: blessed inline-spacing PNG golden -> $GOLDEN_PNG_INLINE_SPACING (never bless your own render blind — see brief §10)"
  elif [ ! -f "$GOLDEN_PNG_INLINE_SPACING" ]; then
    bad "A3y: no golden at $GOLDEN_PNG_INLINE_SPACING to compare against (run with --bless once accepted)"
  elif cmp -s "$GOLDEN_PNG_INLINE_SPACING" /tmp/stele_a3y.png; then
    pass "A3y: PNG dump of $FIXTURE_INLINE_SPACING matches golden"
  else
    bad "A3y: PNG dump of $FIXTURE_INLINE_SPACING differs from $GOLDEN_PNG_INLINE_SPACING"
    note "sizes: golden=$(wc -c < "$GOLDEN_PNG_INLINE_SPACING") actual=$(wc -c < /tmp/stele_a3y.png)"
  fi

  # ---------------------------------------------------------------------
  # A3z -- packet/block-floats' own PNG golden: fixtures/css1-float-5526c.
  # html, the W3C CSS1 §5.5.26 "display/box/float/clear test" (see
  # fixtures/evidence/css1-float-5526c.diagnosis.md for the full diagnosis
  # + `tests/css1_float_golden.rs`'s own doc comment for the structural-
  # fidelity claim this golden makes: a narrow red `dt` column on the left,
  # a white `dd` column on the right with a row of yellow `li` cards side
  # by side -- not stacked -- plus the floated blockquote/h1, matching
  # `fixtures/evidence/css1-float-5526c.reference.gif`'s Mondrian-grid
  # layout to a pixel-structural (not byte-identical -- font rasterization
  # and form-widget chrome are excused, same as every other CSS1-vintage
  # fixture) degree. Same blessing discipline, same byte-equality
  # rationale as every other PNG golden here (A3e/A3f/.../A3y) -- taffy's
  # `float_layout` placement + Stele's own PNG encoder are both
  # deterministic, so a correct render byte-matches its own golden exactly
  # even though the golden itself was never claimed to byte-match the W3C
  # reference GIF.
  # ---------------------------------------------------------------------
  GOLDEN_PNG_CSS1_FLOAT="goldens/css1-float-5526c.png"
  FIXTURE_CSS1_FLOAT="fixtures/css1-float-5526c.html"
  if [ ! -f "$HOST_BIN" ]; then
    bad "A3z: host binary still not found at $HOST_BIN"
  elif ! "$HOST_BIN" --headless --dump-png "$FIXTURE_CSS1_FLOAT" /tmp/stele_a3z.png 2>/tmp/stele_a3z.err; then
    bad "A3z: stele --headless --dump-png crashed on $FIXTURE_CSS1_FLOAT"
    sed 's/^/    /' /tmp/stele_a3z.err
  elif [ "$BLESS" = 1 ]; then
    cp /tmp/stele_a3z.png "$GOLDEN_PNG_CSS1_FLOAT"
    pass "A3z: blessed css1-float-5526c PNG golden -> $GOLDEN_PNG_CSS1_FLOAT (never bless your own render blind — see brief §10)"
  elif [ ! -f "$GOLDEN_PNG_CSS1_FLOAT" ]; then
    bad "A3z: no golden at $GOLDEN_PNG_CSS1_FLOAT to compare against (run with --bless once accepted)"
  elif cmp -s "$GOLDEN_PNG_CSS1_FLOAT" /tmp/stele_a3z.png; then
    pass "A3z: PNG dump of $FIXTURE_CSS1_FLOAT matches golden"
  else
    bad "A3z: PNG dump of $FIXTURE_CSS1_FLOAT differs from $GOLDEN_PNG_CSS1_FLOAT"
    note "sizes: golden=$(wc -c < "$GOLDEN_PNG_CSS1_FLOAT") actual=$(wc -c < /tmp/stele_a3z.png)"
  fi

  # ---------------------------------------------------------------------
  # A5 -- the M6 hardening packet's kitchen-sink coverage fixture
  # (fixtures/kitchen-sink.html, "the everything page": headings, inline
  # markup, lists, blockquote, pre, hr, br, a table with colspan/rowspan, a
  # form, plain + floated images, author-CSS flexbox, details/summary
  # open+closed, noscript, entities, and a small @media rule -- the WHOLE
  # curated dialect in one document, a coverage benchmark + regression net,
  # NOT an adversarial torture test -- soup.html/tests/fuzz_totality.rs are
  # that). Two independent checks, same blessing discipline as every other
  # golden here: A5a is the tty dump (real author-CSS pipeline, no image
  # fetch -- matches --dump-text's own contract), A5b is the PNG dump (real
  # author-CSS + real image fetch/decode pipeline -- matches --dump-png's).
  # `tests/kitchen_sink_golden.rs` drives the exact same two pipelines
  # in-process (Rust-level pixel/text assertions); this block is the
  # independent end-to-end confirmation through the real compiled binary,
  # same rationale as A3e/A3f/A3g's own such notes.
  #
  # NOTE for the record: the original build brief (`stele-build-brief.md`
  # §0) defines "A5" as a first-paint SPEED budget over kitchen-sink.html
  # (< 50M retired instructions under qemu-i386, or < 150ms host wall-clock)
  # -- a performance regression fence, not a golden-render check. This M6
  # packet's own brief explicitly directed "Wire accept.sh A5 (kitchen-sink
  # tty + png golden checks)" instead, which is what's implemented here;
  # flagged so the orchestrator can decide whether the speed-budget check
  # belongs alongside this (as A5c, say) in a follow-up, since the two are
  # not mutually exclusive and this packet did not implement the former.
  # ---------------------------------------------------------------------
  GOLDEN_TTY_KITCHEN_SINK="goldens/kitchen-sink.tty.txt"
  FIXTURE_KITCHEN_SINK="fixtures/kitchen-sink.html"
  if [ ! -f "$HOST_BIN" ]; then
    bad "A5a: host binary still not found at $HOST_BIN"
  elif ! out_kitchen_sink="$("$HOST_BIN" --headless --dump-text "$FIXTURE_KITCHEN_SINK" 2>/tmp/stele_a5a.err)"; then
    bad "A5a: stele --headless --dump-text crashed on $FIXTURE_KITCHEN_SINK"
    sed 's/^/    /' /tmp/stele_a5a.err
  elif [ "$BLESS" = 1 ]; then
    printf '%s\n' "$out_kitchen_sink" > "$GOLDEN_TTY_KITCHEN_SINK"
    pass "A5a: blessed kitchen-sink tty golden -> $GOLDEN_TTY_KITCHEN_SINK (never bless your own render blind — see brief §10)"
  elif diff -u "$GOLDEN_TTY_KITCHEN_SINK" <(printf '%s\n' "$out_kitchen_sink") >/tmp/stele_a5a.diff 2>&1; then
    pass "A5a: tty dump of $FIXTURE_KITCHEN_SINK matches golden"
  else
    bad "A5a: tty dump of $FIXTURE_KITCHEN_SINK differs from $GOLDEN_TTY_KITCHEN_SINK"
    sed 's/^/    /' /tmp/stele_a5a.diff
  fi

  GOLDEN_PNG_KITCHEN_SINK="goldens/kitchen-sink.png"
  if [ ! -f "$HOST_BIN" ]; then
    bad "A5b: host binary still not found at $HOST_BIN"
  elif ! "$HOST_BIN" --headless --dump-png "$FIXTURE_KITCHEN_SINK" /tmp/stele_a5b.png 2>/tmp/stele_a5b.err; then
    bad "A5b: stele --headless --dump-png crashed on $FIXTURE_KITCHEN_SINK"
    sed 's/^/    /' /tmp/stele_a5b.err
  elif [ "$BLESS" = 1 ]; then
    cp /tmp/stele_a5b.png "$GOLDEN_PNG_KITCHEN_SINK"
    pass "A5b: blessed kitchen-sink PNG golden -> $GOLDEN_PNG_KITCHEN_SINK (never bless your own render blind — see brief §10)"
  elif [ ! -f "$GOLDEN_PNG_KITCHEN_SINK" ]; then
    bad "A5b: no golden at $GOLDEN_PNG_KITCHEN_SINK to compare against (run with --bless once accepted)"
  elif cmp -s "$GOLDEN_PNG_KITCHEN_SINK" /tmp/stele_a5b.png; then
    pass "A5b: PNG dump of $FIXTURE_KITCHEN_SINK matches golden"
  else
    bad "A5b: PNG dump of $FIXTURE_KITCHEN_SINK differs from $GOLDEN_PNG_KITCHEN_SINK"
    note "sizes: golden=$(wc -c < "$GOLDEN_PNG_KITCHEN_SINK") actual=$(wc -c < /tmp/stele_a5b.png)"
  fi
fi

if [ "$TTY_ONLY" = 1 ]; then
  echo "===================================="
  if [ "$fail" = 0 ]; then
    echo "ACCEPT (--tty-only): A3 tty-golden check green"
    exit 0
  else
    echo "REJECT (--tty-only): A3 tty-golden check failed"
    exit 1
  fi
fi

# ---------------------------------------------------------------------------
# Not yet live — each flips on at the milestone that earns it.
# ---------------------------------------------------------------------------
if [ -f src/dom/ast.rs ]; then
  # A6 covenant grep, live once the AST exists: no script variant may appear.
  if grep -i "script" src/dom/ast.rs >/tmp/stele_covenant 2>/dev/null; then
    bad "A6 covenant: 'script' appears in src/dom/ast.rs — the AST must have no script variant"
    sed 's/^/    /' /tmp/stele_covenant
  else
    pass "A6 covenant: no script variant in src/dom/ast.rs"
  fi
else
  pend "A6: cargo-audit clean + cargo-auditable + covenant grep — M1 (ast.rs) / M6 (audit)"
fi
pend "A7: JOURNAL/DECISIONS/REPORT current for the operator — M6"

echo "===================================="
if [ "$fail" = 0 ]; then
  echo "ACCEPT: all live checks green"
  exit 0
else
  echo "REJECT: a live check failed"
  exit 1
fi
