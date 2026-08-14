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
# mem-Surface PNG half stays PENDING until P9's fb backend lands (M4).
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
fi
pend "A3: mem-Surface PNG goldens — P9/M4"

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
