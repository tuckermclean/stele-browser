//! M6 hardening: a hand-rolled mutation-fuzz / totality-stress harness.
//!
//! `cargo-fuzz` isn't available in this image (no network access to fetch
//! its libFuzzer dependency, and this is a from-scratch i486 target anyway),
//! so this is a plain `#[test]` that drives the SAME real pipeline every
//! other golden test in this repo drives (`dom::parser::parse` ->
//! `style::collect_author_sheets_for_viewport` (which itself calls the
//! private `style::media::flatten_media` pre-pass — see that function's own
//! doc comment; there is no separate public entry point to call it through
//! directly, and none is needed: every real render path in `main.rs` goes
//! through `collect_author_sheets_for_viewport`, so driving THAT is driving
//! the real pipeline, `flatten_media` included) -> `style::cascade::cascade`
//! -> `layout::box_tree::build_box_tree` -> `layout::layout` ->
//! `backend::tty::render`, occasionally also `backend::raster::paint` onto a
//! small `MemSurface` + `backend::raster::encode_png`) over thousands of
//! deliberately hostile/mutated/random inputs, asserting only ONE thing per
//! iteration: **it returns**. No panic, no abort, no hang.
//!
//! `panic = "abort"` (`Cargo.toml`'s `[profile.release]`, charter C4 "the
//! rock does not unwind") is what makes a real render binary crash hard on
//! any panic; `cargo test` itself builds under the default (`unwind`) test
//! profile, so a panic here surfaces as an ordinary failed-test backtrace
//! rather than a hard process abort — same discovery value (a failing
//! assertion this harness never makes on purpose, i.e. a genuine bug), a
//! more debuggable failure mode locally. Every panic this harness would
//! catch is exactly the class of bug that DOES abort the real release
//! binary — this is what stands in for that hard guard in a test run.
//!
//! ## Determinism
//!
//! A hand-rolled `xorshift64*` PRNG (`Rng`, below) — NOT the `rand` crate
//! (no new deps per the packet brief) and no system entropy — seeded with a
//! fixed constant, so every CI run mutates/generates the exact same sequence
//! of inputs. A discovered panic is thus always reproducible by re-running
//! this same test: `cargo +nightly test --test fuzz_totality`.
//!
//! ## Budget
//!
//! Each of the four fuzz categories below runs a few thousand bounded
//! iterations (see each category's own `ITERATIONS` constant) — small
//! documents, a handful of pipeline stages, no I/O beyond the once-at-
//! compile-time `include_bytes!`/`include_str!` corpus — so the whole test
//! finishes in well under a second on ordinary hardware, comfortably inside
//! "well under a minute" for CI.
//!
//! ## Harness totality
//!
//! The harness itself must not be the thing that panics on its own
//! constructed input: mutated bytes are fed through `dom::parser::parse`
//! (which takes `&str`, not `&[u8]`) via `String::from_utf8_lossy` — never
//! `str::from_utf8(..).unwrap()` — so a mutation that chops a multi-byte
//! UTF-8 sequence in half degrades to U+FFFD replacement characters rather
//! than failing the conversion. Every index/slice operation in the mutators
//! below is bounds-checked against the CURRENT buffer length at the point
//! it's used (never a stale length), and every random range/index draw
//! saturates rather than divides by zero on an empty buffer.

use std::collections::HashMap;

use stele::backend::{raster, tty};
use stele::dom;
use stele::img;
use stele::layout::{self, box_tree::build_box_tree, Size};
use stele::style::{self, cascade, parser as css_parser};
use stele::surface::{Color, MemSurface};

// ---------------------------------------------------------------------------
// Deterministic PRNG (xorshift64* — see module docs: no `rand`, no system
// entropy, fixed seed so CI is exactly reproducible run to run).
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    /// `seed | 1`: xorshift64* never advances from an all-zero state (it
    /// would stay zero forever), so this guards against a caller passing
    /// `0` — every seed actually used below is a nonzero literal anyway,
    /// this is just defensive.
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A value in `[0, bound)`. `bound == 0` degrades to `0` rather than
    /// dividing by zero — the one totality seam every other helper below
    /// routes through.
    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next_u64() % bound as u64) as usize
        }
    }

    fn byte(&mut self) -> u8 {
        (self.next_u64() & 0xFF) as u8
    }
}

// ---------------------------------------------------------------------------
// Corpus: every real HTML fixture in the repo (kitchen-sink.html included —
// see the M6 packet brief), byte-mutated; plus fully-random blobs; plus
// small generated CSS strings; plus every real image fixture, byte-mutated,
// for `img::decode_bytes`.
// ---------------------------------------------------------------------------

const HTML_CORPUS: &[&[u8]] = &[
    include_bytes!("../fixtures/author-css.html"),
    include_bytes!("../fixtures/basic.html"),
    include_bytes!("../fixtures/details.html"),
    include_bytes!("../fixtures/entities.html"),
    include_bytes!("../fixtures/flex-polite.html"),
    include_bytes!("../fixtures/forms.html"),
    include_bytes!("../fixtures/frame_cycle_a.html"),
    include_bytes!("../fixtures/frame_cycle_b.html"),
    include_bytes!("../fixtures/frame_main.html"),
    include_bytes!("../fixtures/frame_nav.html"),
    include_bytes!("../fixtures/frames.html"),
    include_bytes!("../fixtures/images.html"),
    include_bytes!("../fixtures/kitchen-sink.html"),
    include_bytes!("../fixtures/media-query.html"),
    include_bytes!("../fixtures/noscript.html"),
    include_bytes!("../fixtures/soup.html"),
    include_bytes!("../fixtures/tables.html"),
];

const IMAGE_CORPUS: &[&[u8]] = &[
    include_bytes!("../fixtures/images-red.png"),
    include_bytes!("../fixtures/images-blue.gif"),
    include_bytes!("../fixtures/images-anim.gif"),
    include_bytes!("../fixtures/p4-baseline.jpg"),
    include_bytes!("../fixtures/p4-cmyk.jpg"),
    include_bytes!("../fixtures/p4-progressive.jpg"),
];

/// Bytes injected preferentially by the "insert a byte" mutation — the
/// syntax-load-bearing characters for both HTML and CSS (brief: "inject
/// `<`/`>`/`&`/quotes/braces").
const PROVOCATIVE_BYTES: &[u8] = b"<>&\"'{};:/=%#-@!";

/// Cap on any single mutated/generated buffer this harness ever builds — a
/// mutation loop that keeps inserting/duplicating chunks could otherwise
/// grow a buffer unboundedly across many mutation ops in one iteration; this
/// keeps every iteration's own cost bounded regardless of how the RNG rolls.
const MAX_BUF_LEN: usize = 8_000;

/// One mutation pass over `base`: a handful of small, local edits (flip,
/// insert, delete, duplicate-a-chunk, truncate — brief: "byte-mutations...
/// flip/insert/delete/truncate random bytes, duplicate chunks, inject
/// `<`/`>`/`&`/quotes/braces"). Total over any `base` (including empty):
/// every index draw goes through `Rng::below`, itself total on a zero bound.
fn mutate(rng: &mut Rng, base: &[u8]) -> Vec<u8> {
    let mut buf = base.to_vec();
    let ops = 1 + rng.below(6);
    for _ in 0..ops {
        if buf.is_empty() {
            buf.push(rng.byte());
            continue;
        }
        match rng.below(6) {
            0 => {
                // Flip a random bit in a random byte.
                let i = rng.below(buf.len());
                buf[i] ^= 1 << rng.below(8);
            }
            1 => {
                // Insert a random byte at a random position.
                let i = rng.below(buf.len() + 1);
                buf.insert(i, rng.byte());
            }
            2 => {
                // Delete a random byte.
                let i = rng.below(buf.len());
                buf.remove(i);
            }
            3 => {
                // Inject a syntax-provocative byte.
                let i = rng.below(buf.len() + 1);
                let b = PROVOCATIVE_BYTES[rng.below(PROVOCATIVE_BYTES.len())];
                buf.insert(i, b);
            }
            4 => {
                // Duplicate a small chunk elsewhere in the buffer.
                let start = rng.below(buf.len());
                let max_len = (buf.len() - start).min(32).max(1);
                let len = 1 + rng.below(max_len);
                let end = (start + len).min(buf.len());
                let chunk: Vec<u8> = buf[start..end].to_vec();
                let at = rng.below(buf.len() + 1);
                for (k, b) in chunk.into_iter().enumerate() {
                    let pos = (at + k).min(buf.len());
                    buf.insert(pos, b);
                }
            }
            _ => {
                // Truncate at a random point.
                let cut = rng.below(buf.len() + 1);
                buf.truncate(cut);
            }
        }
        if buf.len() > MAX_BUF_LEN {
            buf.truncate(MAX_BUF_LEN);
        }
    }
    buf
}

/// A fully-random byte blob, bounded length — no relation to any real
/// fixture at all (brief: "some fully-random byte blobs").
fn random_blob(rng: &mut Rng, max_len: usize) -> Vec<u8> {
    let len = rng.below(max_len + 1);
    (0..len).map(|_| rng.byte()).collect()
}

/// Small CSS-flavored fragments, mixed with occasional raw garbage bytes —
/// enough to hit real tokenizer/parser branches (selectors, declarations,
/// `@media`, braces) without being a literal copy of any real stylesheet.
const CSS_FRAGMENTS: &[&str] = &[
    "p", "div", "span", "a", ".cls", "#id", ":", ";", "{", "}", "(", ")", ",", " ", "@media", "@import", "min-width",
    "max-width", "px", "em", "%", "0", "1", "800", "auto", "!important", "flex", "display", "block", "none", "color",
    "red", "#fff", "rgb", "url", "\"", "'", "\n",
];

/// A small random CSS string (brief: "random small CSS strings fed to
/// `style::parse`"). `char::from(u8)` is total for every `u8` value (all
/// 256 map to valid Unicode scalar values in the Latin-1 range), so mixing
/// in raw garbage bytes as chars never risks an invalid-`char` panic.
fn random_css(rng: &mut Rng) -> String {
    let n = 1 + rng.below(40);
    let mut s = String::new();
    for _ in 0..n {
        if rng.below(8) == 0 {
            s.push(char::from(rng.byte()));
        } else {
            s.push_str(CSS_FRAGMENTS[rng.below(CSS_FRAGMENTS.len())]);
        }
    }
    s
}

// ---------------------------------------------------------------------------
// Pipeline drivers — the real wiring, matching `main.rs`'s `dump_text`/
// `dump_png` (see module docs).
// ---------------------------------------------------------------------------

/// Drive the full document pipeline over raw (possibly hostile/invalid-UTF8)
/// `bytes` at a given `cols`/viewport width, asserting nothing beyond "this
/// returns". Mirrors `main.rs::dump_text`'s wiring exactly (fetch excluded —
/// there's no network/file hop to fuzz here, only the parse-through-render
/// pipeline), occasionally also painting to a small pixel surface and PNG-
/// encoding it (brief: "occasionally paint::paint onto a small MemSurface +
/// encode_png").
fn drive_pipeline(bytes: &[u8], cols: usize, also_paint: bool) {
    let html = String::from_utf8_lossy(bytes);
    let dom_tree = dom::parser::parse(&html);

    let viewport_w = (cols as f32) * 8.0;
    let sheets = style::collect_author_sheets_for_viewport(&dom_tree, viewport_w);
    let styles = cascade::cascade(&dom_tree, &sheets);

    let Some(root) = build_box_tree(&dom_tree, &styles, &HashMap::new()) else {
        return;
    };
    let viewport = Size { w: viewport_w, h: 20_000.0 };
    let fragments = layout::layout(&root, viewport);
    let _ = tty::render(&fragments, cols).to_text();

    if also_paint {
        // A small, bounded pixel surface — this is a totality stress pass,
        // not a golden render, so an arbitrarily tall document is clamped
        // hard rather than sized to content (mirrors `main.rs::dump_png`'s
        // own `MAX_PNG_HEIGHT` clamp, just much smaller — speed matters more
        // than fidelity here).
        let mut content_bottom = 0.0f32;
        for f in &fragments {
            let (y, h) = (f.rect.origin.y, f.rect.size.h);
            if y.is_finite() && h.is_finite() {
                content_bottom = content_bottom.max(y + h);
            }
        }
        let height = if content_bottom.is_finite() && content_bottom > 0.0 {
            (content_bottom.ceil() as u32).clamp(1, 600)
        } else {
            1
        };
        let width = (cols as u32 * 8).clamp(1, 800);
        let mut surface = MemSurface::new(width, height, Color::WHITE);
        raster::paint(&mut surface, &fragments, &HashMap::new());
        let _ = raster::encode_png(&surface);
    }
}

/// A handful of representative-but-varied tty widths/viewports, cheap enough
/// to cycle through without materially slowing the fuzz loop — narrow widths
/// stress line-wrapping/float-exclusion edge cases wider ones rarely hit.
const COLS_CHOICES: &[usize] = &[1, 10, 20, 40, 80, 120];

fn pick_cols(rng: &mut Rng) -> usize {
    COLS_CHOICES[rng.below(COLS_CHOICES.len())]
}

// ---------------------------------------------------------------------------
// The four fuzz categories.
// ---------------------------------------------------------------------------

const HTML_MUTATION_ITERATIONS: usize = 2000;
const RANDOM_BLOB_ITERATIONS: usize = 800;
const CSS_FUZZ_ITERATIONS: usize = 800;
const IMAGE_FUZZ_ITERATIONS: usize = 800;

#[test]
fn fuzz_html_mutations_of_real_fixtures_never_panic() {
    let mut rng = Rng::new(0xC0FFEE_1996_u64);
    for i in 0..HTML_MUTATION_ITERATIONS {
        let base = HTML_CORPUS[rng.below(HTML_CORPUS.len())];
        let mutated = mutate(&mut rng, base);
        let cols = pick_cols(&mut rng);
        let also_paint = i % 10 == 0;
        drive_pipeline(&mutated, cols, also_paint);
    }
}

#[test]
fn fuzz_fully_random_blobs_never_panic() {
    let mut rng = Rng::new(0xDEAD_BEEF_2600_u64);
    for i in 0..RANDOM_BLOB_ITERATIONS {
        let blob = random_blob(&mut rng, 2000);
        let cols = pick_cols(&mut rng);
        let also_paint = i % 10 == 0;
        drive_pipeline(&blob, cols, also_paint);
    }
}

#[test]
fn fuzz_random_css_strings_never_panic() {
    let mut rng = Rng::new(0xFACADE_5150_u64);
    for _ in 0..CSS_FUZZ_ITERATIONS {
        let css = random_css(&mut rng);
        // `style::parser::parse` directly (brief: "random small CSS strings
        // fed to `style::parse`") ...
        let sheet: css_parser::Stylesheet = css_parser::parse(&css);
        // ... and cascaded against a tiny real document, so a pathological
        // sheet is also exercised through the same `cascade` path a real
        // `<style>` block would take, not just parsed and discarded.
        let dom_tree = dom::parser::parse("<html><body><p class=\"cls\" id=\"id\">x</p></body></html>");
        let _ = cascade::cascade(&dom_tree, std::slice::from_ref(&sheet));
    }
}

#[test]
fn fuzz_image_decode_never_panics() {
    let mut rng = Rng::new(0xBADA55_1994_u64);
    for i in 0..IMAGE_FUZZ_ITERATIONS {
        let bytes = if i % 4 == 0 {
            random_blob(&mut rng, 4000)
        } else {
            let base = IMAGE_CORPUS[rng.below(IMAGE_CORPUS.len())];
            mutate(&mut rng, base)
        };
        // Exercise both the hinted and sniffed decode paths (brief:
        // "img::decode_bytes with random/truncated bytes").
        let content_type = match rng.below(4) {
            0 => Some("image/png"),
            1 => Some("image/jpeg"),
            2 => Some("image/gif"),
            _ => None,
        };
        let _ = img::decode_bytes(&bytes, content_type);
    }
}

// ---------------------------------------------------------------------------
// Harness self-tests (not fuzzing — proving the harness's own helpers are
// total, per the packet brief's "guard against the harness itself being
// non-total" requirement).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod harness_self_totality {
    use super::*;

    #[test]
    fn mutate_on_empty_input_does_not_panic() {
        let mut rng = Rng::new(1);
        for _ in 0..50 {
            let _ = mutate(&mut rng, &[]);
        }
    }

    #[test]
    fn mutate_never_exceeds_the_length_cap() {
        let mut rng = Rng::new(2);
        let base = vec![b'x'; MAX_BUF_LEN];
        for _ in 0..20 {
            let out = mutate(&mut rng, &base);
            assert!(out.len() <= MAX_BUF_LEN);
        }
    }

    #[test]
    fn random_blob_respects_the_requested_bound() {
        let mut rng = Rng::new(3);
        for _ in 0..50 {
            let out = random_blob(&mut rng, 100);
            assert!(out.len() <= 100);
        }
    }

    #[test]
    fn random_blob_with_zero_max_len_is_always_empty() {
        let mut rng = Rng::new(4);
        assert_eq!(random_blob(&mut rng, 0), Vec::<u8>::new());
    }

    #[test]
    fn rng_below_zero_bound_is_total() {
        let mut rng = Rng::new(5);
        for _ in 0..1000 {
            assert_eq!(rng.below(0), 0);
        }
    }

    #[test]
    fn random_css_produces_valid_utf8_strings_of_bounded_length() {
        let mut rng = Rng::new(6);
        for _ in 0..200 {
            let s = random_css(&mut rng);
            assert!(s.len() < 2000, "random_css should stay small");
        }
    }

    /// Same fixed seeds the real fuzz tests use, run for real one more
    /// time here too — a cheap, explicit "this exact sequence is
    /// deterministic" check independent of the four `#[test]`s above (which
    /// only assert "returns", not reproducibility itself).
    #[test]
    fn same_seed_produces_the_same_mutation_sequence() {
        let mut a = Rng::new(0xC0FFEE_1996_u64);
        let mut b = Rng::new(0xC0FFEE_1996_u64);
        for _ in 0..500 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }
}
