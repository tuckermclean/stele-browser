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
# Build it with the canonical pipeline (inside the monolith-builder image):
#   cargo build --release \
#     --target targets/i486-monolith-linux-musl.json \
#     -Zbuild-std=std,panic_abort \
#     -Zjson-target-spec   # this nightly gates .json target specs behind it
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
# A2 — size budget (≤ 2.0 MB stripped). Informational in M0; a hard gate in M6.
# ---------------------------------------------------------------------------
if [ "$TTY_ONLY" = 1 ]; then
  :
elif [ -f "$BIN" ]; then
  bytes=$(wc -c < "$BIN")
  note "size: ${bytes} bytes (budget ${SIZE_BUDGET_BYTES})"
  if [ "$bytes" -le "$SIZE_BUDGET_BYTES" ]; then
    pass "A2: within size budget (informational until M6)"
  else
    pend "A2: OVER budget — informational until M6 size pass"
  fi
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
pend "A5: first-paint speed budget over kitchen-sink.html — M5/M6"
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
