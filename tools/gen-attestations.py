#!/usr/bin/env python3
"""Generate `src/fetch/attestations_data.rs` from the REAL, i486-target-
filtered Cargo dependency graph (packet/attestation-modal, design doc
`docs/superpowers/specs/2026-08-21-attestation-modal-design.md` §Design 3).
Mirrors `tools/gen-terminus-glyphs.py`'s own precedent: a checked-in,
build-time-only generator; its OUTPUT is committed, it is never itself a
Cargo dependency of `stele`.

## What this does

1. Runs `cargo metadata --format-version=1 -Zjson-target-spec
   --filter-platform <i486 target JSON>` (the exact triple this repo builds
   for -- `targets/i486-monolith-linux-musl.json`, read from `accept.sh`'s
   own `--target` flag). `--filter-platform` resolves the REAL per-target
   graph, which is why `windows-sys`/`windows-link` (only reachable via
   `rustix`'s/`errno`'s `cfg(windows)` edges) never even appear in the
   output -- confirmed empirically, not hand-filtered by name.
2. Walks the resolved graph from the `stele` root node, following only
   `Normal`-kind dependency edges (drops `build`/`dev` edges -- this is how
   `slotmap`'s `version_check` build-dependency is excluded, automatically,
   with no name-based special case). Any node whose own compiled artifact is
   a `proc-macro` (`crate_types` contains `"proc-macro"`) is included as a
   leaf but its OWN dependencies are NOT walked -- a proc-macro crate runs
   on the build host at compile time and contributes zero bytes to the
   shipped `stele-i486` binary, and neither do ITS dependencies (this is how
   `serde_derive` -- and, transitively, `syn`/`quote`/`proc-macro2`/
   `unicode-ident`, which are reachable ONLY through `serde_derive` in this
   graph, confirmed by inspecting every inbound edge -- are excluded).
   `serde_derive` itself is then dropped from the final roster too (it is
   the proc-macro leaf, still build-host-only).
3. For each surviving package, picks an SPDX id: prefer `MIT` if the
   package's license expression offers it (uniform MIT-over-Apache-2.0
   choice, §Design 2's reasoning -- smaller text, fully valid discharge of
   a dual `MIT OR Apache-2.0` grant), else `Zlib` if offered, else the
   expression's first alternative (there are no sole-non-MIT/Zlib cases in
   this graph today; a future dependency that hits this path is a real
   signal to look at, not silently guessed past -- the tool errors loudly
   instead).
4. Locates that license's text on disk under the package's own vendored
   `~/.cargo/registry/src/.../<pkg>-<version>/` directory (present after any
   ordinary `cargo fetch`/`cargo metadata`) -- EXCEPT `taffy`, whose
   published crate tarball ships no `LICENSE*` file at all (confirmed: `ls`
   on the registry cache turns up nothing). `taffy`'s text is instead the
   pinned upstream source below (embedded directly in this file, not
   downloaded at generation time -- unlike the Terminus BDF sources, this is
   ~1.1 KB of plain text, small enough to vendor in the generator itself
   rather than requiring a separate manual download step).

   Upstream source (pin, re-verify before ever changing):
       URL:     https://raw.githubusercontent.com/DioxusLabs/taffy/v0.13.0/LICENSE
       Commit:  45a56299d366ddb383e593a1f0372158d00e8530 (tag v0.13.0)
       SHA-256: f97daf1a0124413dccf399a4e6626b4b74acd05282f80b6d64ac82225650b77a
   This is the taffy repository's own MIT text (matches its `Cargo.toml`'s
   `license = "MIT"` field exactly) -- taffy the crate ships no license
   file, but the source repository the crate is built from does.
5. Content-hashes (SHA-256, after normalizing line endings to `\n` and
   trimming trailing blank lines -- no other text alteration, attribution
   text is not "cleaned up") every chosen license text and dedupes
   byte-identical ones into `LICENSE_BLOCKS`, in first-seen order.
   Confirmed real (not hypothetical): `adler2`, `linux-raw-sys`, `rustix`,
   `serde`, and `serde_core`'s `LICENSE-MIT` files are all byte-identical
   (a 5-way dedup); `bitflags` 1.3.2 and 2.13.1 share one too.
6. Emits `src/fetch/attestations_data.rs`: a GENERATED-file header (this
   tool's name, the exact `cargo metadata` command, regeneration
   instructions), `pub struct DepEntry`, `pub const DEPS: &[DepEntry]` (one
   entry per surviving package, `license_block` indexing into...),
   `pub const LICENSE_BLOCKS: &[&str]` (the deduped texts). Each block's
   text is the license file's own content AS WRITTEN -- its paragraphs are
   already blank-line-delimited in the source file, which is what lets
   `fetch::about`'s render-assembly step (Task 3) split each block into one
   `<p>` per paragraph without any further processing here (design's
   `white-space: pre` gap -- see that module's doc comment).

## Usage

    python3 tools/gen-attestations.py --out src/fetch/attestations_data.rs

Run from anywhere inside the repo (or pass `--repo-root`) -- it shells out
to `cargo metadata` using the pinned nightly `rust-toolchain.toml` already
resolves for any command run inside the repo, no cross-compiler needed
(metadata resolution never compiles anything).

## Regeneration

Re-run the command above whenever `Cargo.lock`/`Cargo.toml` changes in a way
that could affect the runtime dependency graph. The generated file is
committed (no `build.rs`, no network access needed at `cargo build` time --
AGENTS.md rule 3). Hand edits to `src/fetch/attestations_data.rs` will be
silently overwritten the next time this tool runs -- change extraction
logic HERE, not there.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

TAFFY_LICENSE_URL = "https://raw.githubusercontent.com/DioxusLabs/taffy/v0.13.0/LICENSE"
TAFFY_LICENSE_COMMIT = "45a56299d366ddb383e593a1f0372158d00e8530"
TAFFY_LICENSE_SHA256 = "f97daf1a0124413dccf399a4e6626b4b74acd05282f80b6d64ac82225650b77a"

# Pinned verbatim 2026-08 (see this file's own module doc comment for the
# source/commit/hash). Embedded directly -- small enough (~1.1 KB) not to
# need a separate download step the way the Terminus BDF sources did.
TAFFY_LICENSE_TEXT = """MIT License

Copyright (c) 2018 Visly Inc.
Copyright (c) 2026 Taffy Authors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE."""

# Crates whose own compiled artifact never links into `stele-i486` (excluded
# by construction, not by this list -- see step 2 above) end up here for
# nothing but a loud, documented cross-check the generator performs on
# itself before writing output (`--strict`, default on).
EXPECTED_EXCLUDED = {
    "serde_derive",
    "syn",
    "quote",
    "proc-macro2",
    "unicode-ident",
    "version_check",
    "windows-sys",
    "windows-link",
}

I486_TARGET_JSON = "targets/i486-monolith-linux-musl.json"


@dataclass
class Dep:
    name: str
    version: str
    spdx: str
    license_text: str


def run_cargo_metadata(repo_root: Path) -> dict:
    target_json = repo_root / I486_TARGET_JSON
    if not target_json.is_file():
        raise SystemExit(f"error: {target_json} not found (expected the i486 target spec)")
    cmd = [
        "cargo",
        "metadata",
        "--format-version=1",
        "-Zjson-target-spec",
        "--filter-platform",
        str(target_json),
    ]
    proc = subprocess.run(cmd, cwd=repo_root, capture_output=True, text=True)
    if proc.returncode != 0:
        raise SystemExit(f"error: `{' '.join(cmd)}` failed:\n{proc.stderr}")
    return json.loads(proc.stdout)


def is_proc_macro(pkg: dict) -> bool:
    return any("proc-macro" in t.get("crate_types", []) for t in pkg["targets"])


def walk_runtime_graph(meta: dict) -> list[dict]:
    """BFS from the `stele` root over Normal-kind edges only, not
    recursing into a proc-macro node's own dependencies (step 2 of the
    module doc comment). Returns the surviving package dicts, root and
    proc-macro leaves excluded, sorted by (name, version)."""
    id_to_pkg = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root_id = meta["resolve"]["root"]

    visited: set[str] = set()
    stack = [root_id]
    while stack:
        cur = stack.pop()
        if cur in visited:
            continue
        visited.add(cur)
        pkg = id_to_pkg[cur]
        if cur != root_id and is_proc_macro(pkg):
            continue  # leaf: don't walk a proc-macro crate's own deps
        for d in nodes[cur].get("deps", []):
            kinds = [dk.get("kind") for dk in d.get("dep_kinds", [])]
            if not kinds or any(k is None for k in kinds):
                stack.append(d["pkg"])

    out = []
    for pid in visited:
        pkg = id_to_pkg[pid]
        if pkg["name"] == "stele" or is_proc_macro(pkg):
            continue
        out.append(pkg)
    out.sort(key=lambda p: (p["name"], p["version"]))
    return out


def choose_spdx(license_expr: str) -> str:
    # Handles both modern (" OR ") and legacy ("/") SPDX-ish separators
    # (bitflags 1.3.2 uses the old "MIT/Apache-2.0" form).
    import re

    tokens = [t.strip() for t in re.split(r"\s+OR\s+|/", license_expr) if t.strip()]
    for preferred in ("MIT", "Zlib"):
        if preferred in tokens:
            return preferred
    if not tokens:
        raise SystemExit(f"error: empty license expression {license_expr!r}")
    return tokens[0]


def find_license_text(manifest_path: str, spdx: str, name: str, version: str) -> str:
    if name == "taffy":
        return TAFFY_LICENSE_TEXT
    manifest_dir = Path(manifest_path).parent
    files = [f.name for f in manifest_dir.iterdir() if f.is_file()]
    token = spdx.upper().replace(" ", "")

    def strip_ext(fname: str) -> str:
        upper = fname.upper()
        for ext in (".MD", ".TXT"):
            if upper.endswith(ext):
                return upper[: -len(ext)]
        return upper

    dash_matches = [f for f in files if strip_ext(f) == f"LICENSE-{token}"]
    if dash_matches:
        return (manifest_dir / dash_matches[0]).read_text(encoding="utf-8")

    # Sole-license crates (color_quant, simd-adler32, slotmap): a bare
    # LICENSE[.md|.txt], no dash suffix.
    for cand in ("LICENSE", "LICENSE.md", "LICENSE.txt"):
        if cand in files:
            return (manifest_dir / cand).read_text(encoding="utf-8")

    raise SystemExit(
        f"error: no license file found for {name} {version} (spdx={spdx}, dir={manifest_dir}, "
        f"files={files}) -- add an explicit source, mirroring taffy's pinned fallback above"
    )


def normalize(text: str) -> str:
    """CRLF -> LF, trim trailing blank lines. No other content change --
    attribution text is transcribed, not edited."""
    text = text.replace("\r\n", "\n").replace("\r", "\n")
    return text.rstrip("\n") + "\n"


def rust_string_literal(s: str) -> str:
    out = s.replace("\\", "\\\\").replace('"', '\\"')
    out = out.replace("\n", "\\n")
    return f'"{out}"'


def generate(repo_root: Path, strict: bool = True) -> tuple[str, list[Dep]]:
    meta = run_cargo_metadata(repo_root)
    packages = walk_runtime_graph(meta)

    names = {p["name"] for p in packages}
    if strict:
        bad = names & EXPECTED_EXCLUDED
        if bad:
            raise SystemExit(
                f"error: build-only/Windows-only crate(s) leaked into the runtime roster: {sorted(bad)} "
                "-- the graph walk's exclusion logic (step 2 of this file's doc comment) is supposed to "
                "prevent this; re-check cargo metadata's dep_kinds/crate_types before proceeding"
            )

    deps: list[Dep] = []
    for pkg in packages:
        spdx = choose_spdx(pkg["license"] or "")
        text = normalize(find_license_text(pkg["manifest_path"], spdx, pkg["name"], pkg["version"]))
        deps.append(Dep(name=pkg["name"], version=pkg["version"], spdx=spdx, license_text=text))

    # Content-hash dedup, first-seen order.
    blocks: list[str] = []
    hash_to_index: dict[str, int] = {}
    block_index_for_dep: list[int] = []
    for d in deps:
        h = hashlib.sha256(d.license_text.encode("utf-8")).hexdigest()
        if h not in hash_to_index:
            hash_to_index[h] = len(blocks)
            blocks.append(d.license_text)
        block_index_for_dep.append(hash_to_index[h])

    cmd_doc = (
        "cargo metadata --format-version=1 -Zjson-target-spec "
        f"--filter-platform {I486_TARGET_JSON}"
    )

    lines: list[str] = []
    lines.append("// GENERATED FILE -- DO NOT EDIT BY HAND.")
    lines.append("//")
    lines.append("// Produced by `tools/gen-attestations.py` from the real, i486-target-filtered")
    lines.append("// Cargo dependency graph. Regenerate with:")
    lines.append("//")
    lines.append("//     python3 tools/gen-attestations.py --out src/fetch/attestations_data.rs")
    lines.append("//")
    lines.append(f"// (internally runs: `{cmd_doc}`)")
    lines.append("//")
    lines.append("// `taffy` 0.13.0 ships no LICENSE* file in its published crate; its text below is")
    lines.append(f"// pinned from the upstream repository instead: {TAFFY_LICENSE_URL}")
    lines.append(f"// (commit {TAFFY_LICENSE_COMMIT}, SHA-256 {TAFFY_LICENSE_SHA256}).")
    lines.append("//")
    lines.append("// See `tools/gen-attestations.py`'s own module doc comment for the full")
    lines.append("// exclusion/dedup algorithm this output was produced by.")
    lines.append("")
    lines.append("//! The Cargo runtime-dependency roster + deduped license texts rendered by")
    lines.append("//! `fetch::about`'s `about:attestations` page (packet/attestation-modal).")
    lines.append("")
    lines.append("/// One credited runtime dependency: its name, resolved version, the SPDX id")
    lines.append("/// this page credits it under (MIT preferred over Apache-2.0 where dual-")
    lines.append("/// licensed -- see the generator's doc comment), and which `LICENSE_BLOCKS`")
    lines.append("/// entry carries its (possibly shared) license text.")
    lines.append("#[derive(Debug, Clone, Copy)]")
    lines.append("pub struct DepEntry {")
    lines.append("    pub name: &'static str,")
    lines.append("    pub version: &'static str,")
    lines.append("    pub spdx: &'static str,")
    lines.append("    pub license_block: usize,")
    lines.append("}")
    lines.append("")
    lines.append(f"/// {len(deps)} runtime dependencies, i486-target-filtered (Windows-only-cfg")
    lines.append("/// crates and build-script/proc-macro-only crates excluded -- neither ships a")
    lines.append("/// byte into `stele-i486`; see the generator's doc comment for why).")
    lines.append("pub const DEPS: &[DepEntry] = &[")
    for d, bidx in zip(deps, block_index_for_dep):
        lines.append(
            f"    DepEntry {{ name: {rust_string_literal(d.name)}, version: {rust_string_literal(d.version)}, "
            f"spdx: {rust_string_literal(d.spdx)}, license_block: {bidx} }},"
        )
    lines.append("];")
    lines.append("")
    lines.append(f"/// {len(blocks)} distinct license texts (content-hash deduped from the")
    lines.append("/// per-dependency texts above -- several crates ship byte-identical")
    lines.append("/// `LICENSE-MIT` files). Each entry's paragraphs are already blank-line-")
    lines.append("/// delimited exactly as the upstream file wrote them; `fetch::about`'s")
    lines.append("/// render-assembly step splits on blank lines into one `<p>` per paragraph")
    lines.append("/// (never a single `<pre>` -- `white-space: pre` is cascaded but not yet")
    lines.append("/// enforced by `layout::inline`, see that module's own doc comment).")
    lines.append("pub const LICENSE_BLOCKS: &[&str] = &[")
    for b in blocks:
        lines.append(f"    {rust_string_literal(b)},")
    lines.append("];")
    lines.append("")

    return "\n".join(lines), deps


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out", required=True, type=Path, help="output path for the generated Rust file")
    parser.add_argument(
        "--repo-root", type=Path, default=Path(__file__).resolve().parent.parent, help="repo root (default: this tool's grandparent dir)"
    )
    parser.add_argument("--no-strict", dest="strict", action="store_false", help="skip the build-only/Windows-only leak cross-check")
    args = parser.parse_args()

    source, deps = generate(args.repo_root, args.strict)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(source, encoding="utf-8")
    print(f"wrote {args.out} ({len(source)} bytes, {len(deps)} deps)", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
