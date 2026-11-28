# F05 — attestations-page

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | F01, B01, D01, D02, T01 | none | M |

**Owns:** `src/fetch/about.rs`, `src/fetch/attestations_data.rs`, `src/fetch/attestations_tests.rs`, `assets/`, `tools/gen-attestations.py`

## Purpose

Imported from the code. The `about:` scheme and its one page, `about:attestations`: Stele's own license
notice, the roster of every third-party crate compiled into the i486 binary with its license, the deduped
license texts, and Terminus Font's OFL-1.1 text, rendered by the engine itself (the A5x golden).

- `about::fetch(request)` never returns an error. The URL's path, compared exactly and with its case,
  picks the page: `attestations` gives the attestation page; any other path, the empty one of `about:`
  included, gives a small "Unknown about: page" naming the path (HTML-escaped) and linking to
  `about:attestations`. Both answer 200 with `content-type: text/html`.
- The page is `assets/attestations.html`, embedded with `include_str!`, with four placeholders filled in:
  `{{STELE_NOTICE}}` (the GPL-3.0-or-later short notice, a constant in `about.rs`), `{{ROSTER}}` (one
  `<li>` per crate, `<name> <version> -- <SPDX>`), `{{LICENSE_TEXTS}}` (per license block, an `<h3>` naming
  the crates that share it, then its text) and `{{OFL_TEXT}}` (`third_party/terminus-font/OFL.TXT`, T01,
  embedded with `include_str!`). Every license or notice text is split on blank lines into `<p>`s, never a
  `<pre>`, because inline layout always collapses whitespace. All interpolated text is HTML-escaped. The
  page is rebuilt on every fetch.
- `src/fetch/attestations_data.rs` is generated (header "GENERATED FILE -- DO NOT EDIT BY HAND"):
  `DepEntry { name, version, spdx, license_block }`, `DEPS` (24 crates) and `LICENSE_BLOCKS` (19 texts).
- `tools/gen-attestations.py --out src/fetch/attestations_data.rs` writes it. It runs `cargo metadata
  --format-version=1 -Zjson-target-spec --filter-platform targets/i486-monolith-linux-musl.json`, walks the
  resolved graph from `stele` over normal (not build or dev) edges, does not walk into a proc-macro crate
  and leaves proc-macro crates out of the roster. It credits each crate as `MIT` when offered, else `Zlib`,
  else the first alternative; reads the license file from the crate's registry directory (`taffy`'s text
  is pinned inside the generator, with its upstream URL, commit and SHA-256, because the published crate
  ships none); normalises line endings; and dedupes texts by SHA-256 in first-seen order. With `--strict`
  (the default) it stops if a known build-only or Windows-only crate reaches the roster.
- `src/fetch/attestations_tests.rs` holds the hand-checked tests of the generated data, kept outside it so a
  regeneration does not discard them; `src/fetch/mod.rs` (F01) includes it under `#[cfg(test)]`.

## Tests

The `tests` module in `src/fetch/about.rs` (8 tests: the page's status and type, the OFL text, a roster
entry, its DOM shape through `dom::parser` (D01, D02), unknown and case-different paths, and totality).
`src/fetch/attestations_tests.rs` (7 tests on `DEPS` and `LICENSE_BLOCKS`). `accept.sh`'s A5x renders
`about:attestations` against `goldens/attestations.png` in the host run (`--tty-only`) and in CI's `accept`
job. `fetch::dispatch_tests` (F01) reaches `about::fetch` through `fetch::fetch`. Nothing runs the
generator.

## Changes

none: imported

## Gate

- **F05-AC-1** — `about:attestations` answers 200 with `content-type: text/html` and a non-empty body. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::about::tests::fetch_attestations_returns_ok_200_html_nonempty` exits 0.
- **F05-AC-2** — The page holds the Terminus OFL-1.1 text (its phrase `Reserved Font Name`). *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::about::tests::attestations_page_embeds_ofl_verbatim` exits 0.
- **F05-AC-3** — The page names `taffy <version>` from `DEPS`, in the roster or in the heading of its license text. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::about::tests::attestations_page_embeds_a_real_generated_roster_entry` exits 0.
- **F05-AC-4** — Parsed, the page has at least one `<h1>`, a `<ul>` with more than five `<li>` children, and no `<pre>`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::about::tests::attestations_page_dom_has_headings_a_big_roster_and_zero_pre_elements` exits 0.
- **F05-AC-5** — Parsed, the page has more than 20 DOM nodes. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::about::tests::attestations_page_body_parses_into_a_non_degenerate_dom` exits 0.
- **F05-AC-6** — `about:` and `about:Attestations` give the unknown page, with status 200. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::about::tests::fetch_bare_about_scheme_is_the_unknown_page fetch::about::tests::fetch_path_case_is_significant_about_attestations_capitalized_is_unknown` exits 0.
- **F05-AC-7** — Every one of `about:`, `about:blank`, `about:xyz`, `about:Attestations`, a 10,000-character path, a non-ASCII path, `about:attestations/../../etc/passwd` and `about:attestations?x=1#frag` answers 200 with a non-empty body. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::about::tests::fetch_is_total_over_hostile_about_inputs` exits 0.
- **F05-AC-8** — `DEPS` credits `taffy` as `MIT` and `slotmap` as `Zlib`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::attestations_tests::taffy_is_credited_as_mit fetch::attestations_tests::slotmap_is_credited_as_zlib_its_sole_license` exits 0.
- **F05-AC-9** — `DEPS` has 24 entries, none of them `serde_derive`, `syn`, `quote`, `proc-macro2`, `unicode-ident`, `version_check`, `windows-sys` or `windows-link`, and every `license_block` indexes `LICENSE_BLOCKS`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::attestations_tests::deps_len_matches_the_real_i486_filtered_runtime_dependency_count fetch::attestations_tests::build_only_and_windows_only_crates_are_excluded fetch::attestations_tests::every_dep_license_block_index_is_in_range` exits 0.
- **F05-AC-10** — `rustix`, `linux-raw-sys` and `serde` share one license block, and every license block over 500 bytes has more than one blank-line paragraph. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::attestations_tests::rustix_linux_raw_sys_and_serde_share_one_deduped_license_block fetch::attestations_tests::every_license_block_over_500_bytes_has_more_than_one_paragraph` exits 0.
- **F05-AC-11** — `about:attestations` renders to `goldens/attestations.png`. *Verify:* `CARGO_BUILD_JOBS=2 ./accept.sh --tty-only` prints `PASS A5x`.
- **F05-AC-12** — `tools/gen-attestations.py` reproduces `src/fetch/attestations_data.rs` from the lockfile's i486 graph, and with `--strict` stops when a build-only or Windows-only crate reaches the roster. *Verify:* unpinned (no test runs the generator).
- **F05-AC-13** — The unknown page HTML-escapes the path it names; the notice and license texts are split into one `<p>` per blank-line paragraph. *Verify:* unpinned.

## Risks

- `src/fetch/attestations_data.rs` is generated: a hand edit is lost at the next run of the generator, and a
  change of logic goes in the generator. Nothing checks that the committed file is the generator's output;
  on 2026-11-28 a run of the generator reproduced it byte for byte.
- The data follows `Cargo.lock` (B01): a node that adds, removes or upgrades a dependency has to rerun the
  generator, so it changes this node's files and must depend on F05. `attestations_tests.rs` pins today's
  graph (24 crates, `taffy`'s and `slotmap`'s licenses, the shared block), and A5x pins the rendered page,
  so such a change also updates those tests and re-blesses `goldens/attestations.png` (golden discipline).
- The generator needs the crates in the local cargo registry and the pinned nightly; it shells out to
  `cargo metadata`. `taffy`'s pinned text has to be checked again when `taffy` is upgraded.
- The page's size is counted against the floppy: the license texts are compiled into the binary.
- `OFL.TXT` (T01) must stay verbatim; this page shows it.
- SPEC.md's planned A6 attestation ceremony (`cargo vendor`, `cargo-auditable`, `cargo-audit`) may change
  where the roster comes from.
