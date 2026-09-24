#!/usr/bin/env bash
#
# ensure-audit-tools.sh — make the A6/C8 audit tooling available, without
# needing a rebuilt substrate image.
#
# cargo-auditable and cargo-audit are absent from the pinned monolith-builder
# image (D6, and build-substrate's own tooling-check step reports both missing
# on every run). D73's answer is to bake them into a layer on top of the pinned
# digest — see tools/monolith-builder-audit.Dockerfile — and that remains the
# fast path. This script is the FLOOR beneath it: when the tools are not in the
# image, install them at accept time so A6 never depends on an image rebuild we
# may not be able to publish.
#
# Deliberately touches nothing under .github/: this file is an ordinary repo
# edit, so a plain `repo`-scoped token can land it (the same reasoning as D73).
#
# Exit codes — accept.sh depends on these:
#   0  both tools are on PATH (already present, or installed just now)
#   3  tools are absent AND cannot be installed here (no cargo, or crates.io
#      unreachable). Expected offline; the caller degrades A6 to PENDING.
#   1  something unexpected went wrong
set -euo pipefail

# Version floors. KEEP IN SYNC with the ARGs in
# tools/monolith-builder-audit.Dockerfile — these are the two copies.
CARGO_AUDITABLE_VERSION="0.7.6"
CARGO_AUDIT_VERSION="0.22.2"

have() { command -v "$1" >/dev/null 2>&1; }

missing=()
for t in cargo-auditable cargo-audit; do
  if have "$t"; then
    echo "$t: present ($("$t" --version 2>/dev/null | head -1))"
  else
    missing+=("$t")
  fi
done

if [ "${#missing[@]}" -eq 0 ]; then
  echo "audit tooling: complete, nothing to install"
  exit 0
fi

echo "audit tooling: missing ${missing[*]}"

if ! have cargo; then
  echo "audit tooling: no cargo on PATH — cannot bootstrap here" >&2
  exit 3
fi

# `cargo install` is the only network-dependent step. A failure here is almost
# always "crates.io unreachable", which is not an error worth failing accept.sh
# over — so a failed install degrades to 3, not 1.
install_one() {
  local crate="$1" version="$2"
  echo "installing $crate $version"
  if cargo install "$crate" --version "$version" --locked; then
    return 0
  fi
  # cargo-audit's default features need a system OpenSSL the image may not
  # carry; the vendored build is the Dockerfile's fallback too.
  if [ "$crate" = "cargo-audit" ]; then
    echo "retrying $crate with vendored openssl"
    if cargo install "$crate" --version "$version" --locked \
         --no-default-features --features fix,vendored-openssl; then
      return 0
    fi
  fi
  return 1
}

for t in "${missing[@]}"; do
  case "$t" in
    cargo-auditable) version="$CARGO_AUDITABLE_VERSION" ;;
    cargo-audit)     version="$CARGO_AUDIT_VERSION" ;;
    *) echo "unknown crate $t" >&2; exit 1 ;;
  esac
  if ! install_one "$t" "$version"; then
    echo "audit tooling: could not install $t (offline, or build failed)" >&2
    exit 3
  fi
done

# Freshly-installed binaries land in CARGO_HOME/bin, which is not necessarily
# on this shell's PATH even though it will be on the caller's next lookup.
cargo_bin="${CARGO_HOME:-$HOME/.cargo}/bin"
case ":$PATH:" in
  *":$cargo_bin:"*) ;;
  *) PATH="$cargo_bin:$PATH"; export PATH ;;
esac

for t in cargo-auditable cargo-audit; do
  have "$t" || { echo "audit tooling: $t still not on PATH after install" >&2; exit 3; }
done

echo "audit tooling: complete (installed ${missing[*]}; ensure $cargo_bin is on PATH)"
exit 0
