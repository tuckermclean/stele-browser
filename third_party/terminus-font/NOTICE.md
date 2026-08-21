# Terminus Font — attribution

Stele embeds a **191-glyph subset** (ASCII `0x20-0x7E` + Latin-1 supplement
`0xA0-0xFF`, 5 pixel sizes × 2 weights) of **Terminus Font**, by
**Dimitar Toshkov Zhekov**, under the **SIL Open Font License, Version 1.1**
(`OFL.TXT` in this directory, copied verbatim from the upstream release).

- **Upstream project**: <https://terminus-font.sourceforge.net/>
- **Version embedded**: `4.49.1`
- **Extraction tool**: `tools/gen-terminus-glyphs.py` (this repo) — reads the
  upstream `.bdf` sources and emits the generated Rust table,
  `src/text/terminus_glyphs.rs`. See that tool's own doc comment for the
  exact release URL and the SHA-256 of the tarball it was verified against.

This is a **Modified Version** under OFL's own definition (a size/glyph
subset, re-encoded into a different data format) — it is **not** "Terminus
Font" itself, is never presented to a user as a chosen typeface (Stele has
no font picker; this subset is the only rendering path), and does not use
the Reserved Font Name "Terminus Font" to describe itself as such. This note
and the accompanying doc comments describe it accurately as *"a 191-glyph
subset of Terminus Font by Dimitar Toshkov Zhekov, embedded under OFL-1.1."*

OFL-1.1's Condition 1 (no standalone sale of the font software) is not a
concern: the subset is embedded as an inseparable rendering dependency, not
a product. Condition 2 (the license text must accompany any copy) is
satisfied at the source/repository level — `OFL.TXT` ships alongside every
checkout — **and**, as of packet/attestation-modal, at the compiled-binary
level too: `about:attestations` (`src/fetch/about.rs`) embeds `OFL.TXT`
verbatim via `include_str!` and renders it in-binary (reachable from the
`--x11` chrome's attestations affordance), so the license text now travels
with the binary itself, not only the source checkout — the "forthcoming
attestation modal" this note previously pointed at has now landed.
