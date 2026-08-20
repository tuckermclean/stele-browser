#!/usr/bin/env python3
"""Generate `src/text/terminus_glyphs.rs` from the upstream Terminus Font BDF
sources (packet/terminus-font, design doc
`docs/superpowers/specs/2026-08-21-terminus-font-design.md`).

## Upstream source (pin before regenerating)

    Release tarball: https://sourceforge.net/projects/terminus-font/files/terminus-font-4.49/terminus-font-4.49.1.tar.gz/download
    Version:          4.49.1
    SHA-256:          d961c1b781627bf417f9b340693d64fc219e0113ad3a3af1a3424c7aa373ef79

Download that tarball, verify its SHA-256 matches the value above, and
extract it to a directory (e.g. `/tmp/terminus-font-4.49.1`). Do NOT commit
the BDF sources themselves to this repository (~2.45 MB of third-party text
that never ships in the binary — see the design doc §1) — only this tool
and its generated output are checked in.

## Usage

    python3 tools/gen-terminus-glyphs.py --bdf-dir <extracted-dir> \
        --out src/text/terminus_glyphs.rs

`--bdf-dir` must contain `ter-u{12,16,20,24,32}{n,b}.bdf` (10 files, the
normal/bold pair at each of the 5 embedded sizes).

## What this does

For each of the 5 sizes x 2 weights (10 BDF files), parses every glyph whose
`ENCODING` falls in `0x20..=0x7E` (printable ASCII) or `0xA0..=0xFF` (Latin-1
supplement) -- 191 glyphs per file, 1,910 total -- and emits a Rust `const`
table of that glyph's bitmap rows, normalized from BDF's MSB-first bit order
to this project's LSB-leftmost convention (bit 0 = leftmost pixel, matching
`src/text/bitmap.rs`'s documented convention for the font8x8 atlas this
packet replaces -- see that module's doc comment for the empirical citation).

### Bit-order normalization (the classic trap -- read before touching this)

A BDF `BITMAP` row is `ceil(glyph_width / 8)` bytes, hex-encoded, MSB-first:
bit 7 of the first byte is column 0 (leftmost), bit 0 of the LAST byte is
the rightmost column in the row's byte-aligned width. This project's raster
convention is the OPPOSITE: bit 0 (LSB) of the row value is the leftmost
pixel (`src/text/bitmap.rs:101`). This tool bit-reverses each row's full
`8 * nbytes`-bit value (not just the meaningful `glyph_width` bits) --
reversing the padding bits along with the real ones is harmless (they land
on the unused high-order end of the output either way) and keeps the
transform a single, uniform operation regardless of whether a row is 1 byte
(sizes 12/16, cell width <= 8) or 2 bytes (sizes 20/24/32, cell width 9-16).

Every glyph in the embedded coverage range has been confirmed (by directly
reading all 10 source files) to have `BBX` exactly equal to the font's
`FONTBOUNDINGBOX` (same width, height, x-offset, y-offset for every glyph --
Terminus's `SPACING "C"` fully-monospace guarantee) -- so no BBX-vs-FBB
offset reconciliation is needed; each glyph's `BITMAP` block is already
exactly `cell_h` rows of exactly `cell_w`-columns-worth of hex data, top row
first. `--strict` (default on) re-verifies this per-glyph while parsing and
aborts loudly if any future Terminus release ever violates it.

## Regeneration

Re-run this exact command after re-verifying the SHA-256 above (or updating
it, if regenerating against a newer Terminus release -- update the pinned
URL/hash in this doc comment AND in the generated file's own header in the
same commit):

    python3 tools/gen-terminus-glyphs.py --bdf-dir <extracted-dir> --out src/text/terminus_glyphs.rs

The generated file is committed to the repo (no `build.rs`, no BDF file
needed at `cargo build` time -- AGENTS.md rule 3, CI-driven, no exotic local
build step). Hand edits to `src/text/terminus_glyphs.rs` will be silently
overwritten the next time this tool runs -- change extraction logic HERE,
not there.
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path

SOURCE_URL = (
    "https://sourceforge.net/projects/terminus-font/files/"
    "terminus-font-4.49/terminus-font-4.49.1.tar.gz/download"
)
SOURCE_SHA256 = "d961c1b781627bf417f9b340693d64fc219e0113ad3a3af1a3424c7aa373ef79"
SOURCE_VERSION = "4.49.1"

# The 5 embedded pixel-size buckets, ascending -- the SINGLE ordering every
# generated table (and, downstream, `text::terminus::nearest_terminus_size`)
# agrees on.
SIZES = [12, 16, 20, 24, 32]
# BDF filename weight suffix -> Rust `FontWeight` variant, in TABLES[..][idx] order.
WEIGHTS = [("n", "Normal"), ("b", "Bold")]

ASCII_RANGE = range(0x20, 0x7F)  # printable ASCII, 95 code points
LATIN1_RANGE = range(0xA0, 0x100)  # Latin-1 supplement, 96 code points


@dataclass
class Glyph:
    encoding: int
    cell_w: int
    cell_h: int
    rows: list[int]  # one bit-reversed row value per row, LSB-leftmost


def reverse_bits(value: int, nbits: int) -> int:
    """Reverse the low `nbits` bits of `value` (MSB-first -> LSB-leftmost)."""
    out = 0
    for i in range(nbits):
        if value & (1 << (nbits - 1 - i)):
            out |= 1 << i
    return out


def parse_bdf(path: Path, strict: bool) -> tuple[int, int, dict[int, Glyph]]:
    """Parse one BDF file; return (cell_w, cell_h, {encoding: Glyph})
    restricted to the ASCII + Latin-1 coverage this packet embeds."""
    text = path.read_text(encoding="ascii")

    fbb_m = re.search(r"^FONTBOUNDINGBOX (\d+) (\d+) (-?\d+) (-?\d+)$", text, re.M)
    if not fbb_m:
        raise ValueError(f"{path}: no FONTBOUNDINGBOX")
    cell_w, cell_h, fbb_xoff, fbb_yoff = (int(g) for g in fbb_m.groups())

    glyphs: dict[int, Glyph] = {}
    blocks = text.split("STARTCHAR")[1:]
    for block in blocks:
        enc_m = re.search(r"^ENCODING (-?\d+)", block, re.M)
        if not enc_m:
            continue
        encoding = int(enc_m.group(1))
        if encoding not in ASCII_RANGE and encoding not in LATIN1_RANGE:
            continue

        bbx_m = re.search(r"^BBX (\d+) (\d+) (-?\d+) (-?\d+)$", block, re.M)
        dwidth_m = re.search(r"^DWIDTH (-?\d+) (-?\d+)$", block, re.M)
        bitmap_m = re.search(r"^BITMAP\n(.*?)\nENDCHAR", block, re.M | re.S)
        if not (bbx_m and dwidth_m and bitmap_m):
            raise ValueError(f"{path}: malformed glyph block for ENCODING {encoding}")

        bbx_w, bbx_h, bbx_xoff, bbx_yoff = (int(g) for g in bbx_m.groups())
        dwidth_x = int(dwidth_m.group(1))

        if strict and not (
            bbx_w == cell_w
            and bbx_h == cell_h
            and bbx_xoff == fbb_xoff
            and bbx_yoff == fbb_yoff
            and dwidth_x == cell_w
        ):
            raise ValueError(
                f"{path}: ENCODING {encoding} BBX/DWIDTH ({bbx_w},{bbx_h},{bbx_xoff},{bbx_yoff}"
                f" dwidth={dwidth_x}) does not match FONTBOUNDINGBOX ({cell_w},{cell_h},"
                f"{fbb_xoff},{fbb_yoff}) -- this tool assumes a fully-monospace font "
                "(SPACING \"C\") where every glyph's box equals the font bounding box; "
                "re-run with --no-strict to bypass (NOT recommended without auditing why)."
            )

        row_lines = [ln.strip() for ln in bitmap_m.group(1).splitlines() if ln.strip()]
        if len(row_lines) != bbx_h:
            raise ValueError(
                f"{path}: ENCODING {encoding} has {len(row_lines)} BITMAP rows, expected {bbx_h}"
            )

        rows: list[int] = []
        for line in row_lines:
            nbits = len(line) * 4  # 2 hex chars per byte, 8 bits per byte
            raw = int(line, 16)
            rows.append(reverse_bits(raw, nbits))

        glyphs[encoding] = Glyph(encoding=encoding, cell_w=bbx_w, cell_h=bbx_h, rows=rows)

    missing = [e for e in list(ASCII_RANGE) + list(LATIN1_RANGE) if e not in glyphs]
    if missing:
        raise ValueError(f"{path}: missing {len(missing)} required glyphs: {missing[:10]}...")

    return cell_w, cell_h, glyphs


def emit_glyphrows(rows: list[int], wide: bool) -> str:
    variant = "Wide" if wide else "Narrow"
    ty = "u16" if wide else "u8"
    items = ", ".join((f"0x{v:04X}" if wide else f"0x{v:02X}") for v in rows)
    return f"GlyphRows::{variant}(&[{items}])"


def build_table_source(size: int, weight_suffix: str, weight_name: str, bdf_dir: Path, strict: bool) -> tuple[str, int, int, str]:
    path = bdf_dir / f"ter-u{size}{weight_suffix}.bdf"
    if not path.exists():
        raise FileNotFoundError(f"missing source BDF: {path}")
    cell_w, cell_h, glyphs = parse_bdf(path, strict)
    wide = cell_w > 8

    ascii_entries = [emit_glyphrows(glyphs[e].rows, wide) for e in ASCII_RANGE]
    latin1_entries = [emit_glyphrows(glyphs[e].rows, wide) for e in LATIN1_RANGE]

    const_prefix = f"SIZE{size}_{weight_name.upper()}"
    lines = []
    lines.append(f"const {const_prefix}_ASCII: [GlyphRows; {len(ascii_entries)}] = [")
    for e in ascii_entries:
        lines.append(f"    {e},")
    lines.append("];")
    lines.append(f"const {const_prefix}_LATIN1: [GlyphRows; {len(latin1_entries)}] = [")
    for e in latin1_entries:
        lines.append(f"    {e},")
    lines.append("];")

    table_expr = (
        f"SizeWeightTable {{ cell_w: {cell_w}, cell_h: {cell_h}, "
        f"ascii: &{const_prefix}_ASCII, latin1: &{const_prefix}_LATIN1 }}"
    )
    return "\n".join(lines), cell_w, cell_h, table_expr  # type: ignore[return-value]


def fallback_rows(cell_w: int, cell_h: int) -> list[int]:
    """A hollow "tofu" box, 1px inset on every side -- same spirit as the
    font8x8 atlas's own `FALLBACK_GLYPH` (`src/text/glyphs.rs`), generalized
    to an arbitrary cell size. Row 0 and the last row are blank (top/bottom
    margin); the second and second-to-last rows are a full horizontal bar
    (columns 1..=cell_w-2); the rows between are just the two side columns
    (1 and cell_w-2) -- a hollow rectangle, never fully solid, easy to spot
    in a golden PNG and visually distinct from any real glyph."""
    if cell_h < 4 or cell_w < 3:
        # Degenerately tiny bucket (shouldn't happen for the 5 real buckets,
        # all >= 6x12) -- fall back to an all-blank glyph rather than
        # indexing out of range.
        return [0] * cell_h
    top_bottom_bar = 0
    for c in range(1, cell_w - 1):
        top_bottom_bar |= 1 << c
    sides = (1 << 1) | (1 << (cell_w - 2))
    rows = [0] * cell_h
    rows[1] = top_bottom_bar
    rows[cell_h - 2] = top_bottom_bar
    for r in range(2, cell_h - 2):
        rows[r] = sides
    return rows


def generate(bdf_dir: Path, strict: bool) -> str:
    size_blocks = []
    table_rows = []  # table_rows[size_idx] = [normal_expr, bold_expr]
    fallback_exprs = []
    cell_dims = []

    for size in SIZES:
        weight_exprs = []
        cw = ch = None
        for suffix, name in WEIGHTS:
            src, cell_w, cell_h, table_expr = build_table_source(size, suffix, name, bdf_dir, strict)
            size_blocks.append(f"// --- size {size}px, weight {name} (ter-u{size}{suffix}.bdf) ---\n{src}")
            weight_exprs.append(table_expr)
            cw, ch = cell_w, cell_h
        table_rows.append(weight_exprs)
        cell_dims.append((cw, ch))
        wide = cw > 8
        fb_rows = fallback_rows(cw, ch)
        fallback_exprs.append(emit_glyphrows(fb_rows, wide))

    out = []
    out.append("// GENERATED FILE -- DO NOT EDIT BY HAND.")
    out.append("//")
    out.append("// Produced by `tools/gen-terminus-glyphs.py` from the Terminus Font BDF")
    out.append(f"// sources (upstream version {SOURCE_VERSION}). Regenerate with:")
    out.append("//")
    out.append("//     python3 tools/gen-terminus-glyphs.py --bdf-dir <extracted-dir> \\")
    out.append("//         --out src/text/terminus_glyphs.rs")
    out.append("//")
    out.append(f"// Source tarball: {SOURCE_URL}")
    out.append(f"// SHA-256:        {SOURCE_SHA256}")
    out.append("// License: SIL Open Font License, Version 1.1 (Dimitar Toshkov Zhekov) --")
    out.append("// see `third_party/terminus-font/OFL.TXT` (verbatim) and `NOTICE.md` in the")
    out.append("// same directory for the full attribution note.")
    out.append("//")
    out.append("//! A 191-glyph subset of Terminus Font (ASCII `0x20-0x7E` + Latin-1")
    out.append("//! `0xA0-0xFF`), 5 pixel sizes (12/16/20/24/32) x 2 weights (normal/bold),")
    out.append("//! embedded as compiled-in `const` tables -- see `tools/gen-terminus-glyphs.py`'s")
    out.append("//! own doc comment for the source, license, and bit-order normalization this")
    out.append("//! file's data has already had applied (BDF's MSB-first rows, bit-reversed to")
    out.append("//! this project's bit 0 (LSB) = leftmost-pixel convention -- see")
    out.append("//! `src/text/bitmap.rs`'s doc comment for that convention's own citation).")
    out.append("//!")
    out.append("//! Storage is mixed-width by design (spec `docs/superpowers/specs/")
    out.append("//! 2026-08-21-terminus-font-design.md` §2/§6): sizes 12/16 (cell width <= 8)")
    out.append("//! store one `u8` per bitmap row ([`GlyphRows::Narrow`]); sizes 20/24/32 (cell")
    out.append("//! width 9-16) store one `u16` per row ([`GlyphRows::Wide`]) -- byte-tight at")
    out.append("//! rest, widened to a uniform `u16` only at the `text::terminus::TerminusFont`")
    out.append("//! lookup boundary (a free `as u16` cast), not in this file's own storage.")
    out.append("//!")
    out.append("//! [`lookup`] is total over all of `char`: any scalar outside the 191-glyph")
    out.append("//! subset returns a small hollow-box fallback glyph at the resolved bucket's")
    out.append("//! own cell size, rather than panicking or vanishing -- the same \"never")
    out.append("//! vanish\" contract `text::glyphs::lookup` (font8x8, this file's predecessor)")
    out.append("//! already guaranteed.")
    out.append("")
    out.append("use crate::style::computed::FontWeight;")
    out.append("")
    out.append("/// The 5 embedded pixel-size buckets, ascending -- index `i` here lines up")
    out.append("/// with `TABLES[i]` and `FALLBACKS[i]`. The single source of truth for which")
    out.append("/// discrete sizes Terminus is embedded at; `text::terminus::nearest_terminus_size`")
    out.append("/// is the canonical snap-to-nearest-bucket function built on top of this list.")
    out.append(f"pub const SIZES: [f32; {len(SIZES)}] = [{', '.join(f'{s}.0' for s in SIZES)}];")
    out.append("")
    out.append("/// One glyph's bitmap rows, top row first, bit 0 (LSB) = leftmost pixel.")
    out.append("/// `Narrow` (one `u8`/row) is used for the 12/16px buckets (cell width <= 8);")
    out.append("/// `Wide` (one `u16`/row) for 20/24/32px (cell width 9-16) -- see this module's")
    out.append("/// doc comment for why storage is mixed-width.")
    out.append("#[derive(Debug, Clone, Copy, PartialEq, Eq)]")
    out.append("pub enum GlyphRows {")
    out.append("    Narrow(&'static [u8]),")
    out.append("    Wide(&'static [u16]),")
    out.append("}")
    out.append("")
    out.append("/// One (size, weight) bucket's full 191-glyph table.")
    out.append("#[derive(Debug, Clone, Copy)]")
    out.append("pub struct SizeWeightTable {")
    out.append("    pub cell_w: u8,")
    out.append("    pub cell_h: u8,")
    out.append("    /// Printable ASCII, `0x20..=0x7E`, indexed by `code - 0x20` (95 entries).")
    out.append("    pub ascii: &'static [GlyphRows],")
    out.append("    /// Latin-1 supplement, `0xA0..=0xFF`, indexed by `code - 0xA0` (96 entries).")
    out.append("    pub latin1: &'static [GlyphRows],")
    out.append("}")
    out.append("")
    out.append("/// One resolved glyph lookup: the bucket's cell size plus that glyph's own")
    out.append("/// (possibly-fallback) rows.")
    out.append("#[derive(Debug, Clone, Copy)]")
    out.append("pub struct Glyph {")
    out.append("    pub cell_w: u8,")
    out.append("    pub cell_h: u8,")
    out.append("    pub rows: GlyphRows,")
    out.append("}")
    out.append("")
    out.append("\n\n".join(size_blocks))
    out.append("")
    out.append(f"/// Fallback (hollow-box) rows, one per size bucket -- see")
    out.append("/// `tools/gen-terminus-glyphs.py`'s `fallback_rows` for the shape's derivation.")
    out.append(f"const FALLBACKS: [GlyphRows; {len(SIZES)}] = [")
    for e in fallback_exprs:
        out.append(f"    {e},")
    out.append("];")
    out.append("")
    out.append(f"/// `TABLES[size_bucket_index][weight_index]` -- `weight_index` is 0 for")
    out.append("/// [`FontWeight::Normal`], 1 for [`FontWeight::Bold`] (see [`weight_index`]).")
    out.append(f"pub const TABLES: [[SizeWeightTable; 2]; {len(SIZES)}] = [")
    for (normal_expr, bold_expr) in table_rows:
        out.append(f"    [{normal_expr}, {bold_expr}],")
    out.append("];")
    out.append("")
    out.append("fn weight_index(weight: FontWeight) -> usize {")
    out.append("    match weight {")
    out.append("        FontWeight::Normal => 0,")
    out.append("        FontWeight::Bold => 1,")
    out.append("    }")
    out.append("}")
    out.append("")
    out.append("/// Nearest-bucket resolution used ONLY internally by [`lookup`] -- callers are")
    out.append("/// expected to already have snapped `size_px` via")
    out.append("/// `text::terminus::nearest_terminus_size` (the canonical, extensively-tested")
    out.append("/// tie-break/clamp rule) before reaching here, so this only ever needs to find")
    out.append("/// the EXACT matching bucket; the simple nearest-by-absolute-difference search")
    out.append("/// below is a defensive fallback for totality (never panics on an unexpected")
    out.append("/// `size_px`), not a second copy of the canonical snap contract.")
    out.append("fn bucket_index(size_px: f32) -> usize {")
    out.append("    if !size_px.is_finite() {")
    out.append("        return 1; // defined finite default: the 16px bucket")
    out.append("    }")
    out.append("    let mut best = 0usize;")
    out.append("    let mut best_diff = f32::INFINITY;")
    out.append("    for (i, &s) in SIZES.iter().enumerate() {")
    out.append("        let diff = (size_px - s).abs();")
    out.append("        if diff < best_diff {")
    out.append("            best_diff = diff;")
    out.append("            best = i;")
    out.append("        }")
    out.append("    }")
    out.append("    best")
    out.append("}")
    out.append("")
    out.append("/// Look up one glyph: total over all of `char` (never panics) -- a scalar")
    out.append("/// outside the 191-glyph subset (`0x20..=0x7E` or `0xA0..=0xFF`) resolves to")
    out.append("/// [`FALLBACKS`]'s hollow box at the same bucket's cell size, rather than")
    out.append("/// vanishing. `size_px` is resolved to a bucket via [`bucket_index`] --see")
    out.append("/// that function's own doc comment for why it does its own (defensive,")
    out.append("/// non-canonical) nearest-match rather than re-implementing")
    out.append("/// `text::terminus::nearest_terminus_size`'s tie-break rule.")
    out.append("pub fn lookup(size_px: f32, weight: FontWeight, ch: char) -> Glyph {")
    out.append("    let idx = bucket_index(size_px);")
    out.append("    let table = &TABLES[idx][weight_index(weight)];")
    out.append("    let code = ch as u32;")
    out.append("    let rows = if (0x20..=0x7E).contains(&code) {")
    out.append("        table.ascii[(code - 0x20) as usize]")
    out.append("    } else if (0xA0..=0xFF).contains(&code) {")
    out.append("        table.latin1[(code - 0xA0) as usize]")
    out.append("    } else {")
    out.append("        FALLBACKS[idx]")
    out.append("    };")
    out.append("    Glyph { cell_w: table.cell_w, cell_h: table.cell_h, rows }")
    out.append("}")
    out.append("")
    return "\n".join(out) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--bdf-dir", required=True, type=Path, help="directory containing ter-uNN{n,b}.bdf")
    parser.add_argument("--out", required=True, type=Path, help="output path for the generated Rust file")
    parser.add_argument("--no-strict", dest="strict", action="store_false", help="skip the BBX==FONTBOUNDINGBOX sanity check (not recommended)")
    args = parser.parse_args()

    if not args.bdf_dir.is_dir():
        print(f"error: --bdf-dir {args.bdf_dir} is not a directory", file=sys.stderr)
        return 1

    source = generate(args.bdf_dir, args.strict)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(source, encoding="utf-8")
    print(f"wrote {args.out} ({len(source)} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
