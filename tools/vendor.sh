#!/usr/bin/env bash
#
# tools/vendor.sh - Copyright (C) 2026 Tucker McLean
#
# This program is free software: you can redistribute it and/or modify
# it under the terms of the GNU General Public License as published by
# the Free Software Foundation, either version 3 of the License, or
# (at your option) any later version.
#
# This program is distributed in the hope that it will be useful,
# but WITHOUT ANY WARRANTY; without even the implied warranty of
# MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
# GNU General Public License for more details.
#
# You should have received a copy of the GNU General Public License
# along with this program.  If not, see <https://www.gnu.org/licenses/>.
#
# vendor.sh — A6/C8 attestation ceremony, the half that needs cargo+network.
#
# This sandbox has neither (no cargo/rustc on PATH — confirmed), so this
# script is committed unrun. It is meant for whoever next has both: a CI
# job on the monolith-builder image, or an operator machine.
#
# What it does:
#   1. `cargo vendor vendor` — vendors the full dependency tree into
#      vendor/, and prints the `[source]` replacement stanza Cargo needs to
#      build from it (this script does NOT edit .cargo/config.toml itself —
#      that's a deliberate, reviewable step, not something to auto-splice).
#   2. Regenerates attestation/dependency-manifest.txt via
#      tools/gen-dependency-manifest.py --write, so the committed manifest
#      and the freshly vendored tree agree.
#   3. If `cargo-auditable` is on PATH, does an opportunistic smoke build
#      (`cargo auditable build --release`) to prove the toolchain round
#      trips — this is NOT the real i486 release build (accept.sh owns
#      that); it only proves cargo-auditable + this Cargo.lock are
#      compatible. Skipped (not failed) if the tool is absent, since that
#      is the documented, escalated gap (DECISIONS.md D70).
#
# Usage: ./tools/vendor.sh   (run from repo root)

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

if ! command -v cargo >/dev/null 2>&1; then
  echo "vendor.sh: cargo not found on PATH — nothing to do here (see DECISIONS.md D70)" >&2
  exit 1
fi

echo "==> cargo vendor vendor"
cargo vendor vendor

echo
echo "==> regenerating attestation/dependency-manifest.txt from the post-vendor Cargo.lock"
python3 tools/gen-dependency-manifest.py --write

if command -v cargo-auditable >/dev/null 2>&1; then
  echo
  echo "==> cargo-auditable present — opportunistic smoke build (cargo auditable build --release)"
  cargo auditable build --release
else
  echo
  echo "==> cargo-auditable not on PATH — skipping smoke build (see DECISIONS.md D70 / escalated image request)"
fi

echo
echo "vendor.sh done. Next: add the [source] replacement stanza cargo vendor printed above to"
echo ".cargo/config.toml (review it — this script does not do that automatically), commit vendor/"
echo "and the regenerated manifest, and re-run ./accept.sh."
