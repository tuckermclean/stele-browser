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
#   ./accept.sh            run all live checks; exit nonzero on first failure
#   ./accept.sh --bless    regenerate golden outputs from the current binary
#                          (never bless your own render blind — see brief §10)
#
# The binary is expected pre-built at:
#   target/i486-monolith-linux-musl/release/stele
# Build it with the canonical pipeline (inside the monolith-builder image):
#   cargo build --release \
#     --target targets/i486-monolith-linux-musl.json \
#     -Zbuild-std=std,panic_abort \
#     -Zjson-target-spec   # this nightly gates .json target specs behind it

set -uo pipefail

TARGET_STEM="i486-monolith-linux-musl"
BIN="target/${TARGET_STEM}/release/stele"
GOLDEN_HELLO="goldens/m0-hello.txt"
SIZE_BUDGET_BYTES=$(( 2 * 1000 * 1000 ))   # A2: 2.0 MB stripped

BLESS=0
[ "${1:-}" = "--bless" ] && BLESS=1

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

echo "== Stele acceptance (M0 skeleton) =="

# ---------------------------------------------------------------------------
# A1 — statically linked, i386-class ELF.
# ---------------------------------------------------------------------------
if [ ! -f "$BIN" ]; then
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
if [ ! -f "$BIN" ]; then
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
if [ -f "$BIN" ]; then
  bytes=$(wc -c < "$BIN")
  note "size: ${bytes} bytes (budget ${SIZE_BUDGET_BYTES})"
  if [ "$bytes" -le "$SIZE_BUDGET_BYTES" ]; then
    pass "A2: within size budget (informational until M6)"
  else
    pend "A2: OVER budget — informational until M6 size pass"
  fi
fi

# ---------------------------------------------------------------------------
# Not yet live — each flips on at the milestone that earns it.
# ---------------------------------------------------------------------------
pend "A3: fixture golden renders (tty dumps + mem-Surface PNGs) — M1..M5"
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
