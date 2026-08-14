//! Stele — a document-web browser for the 486.
//!
//! With no arguments, `main` prints the M0 hello (acceptance A4's golden;
//! `goldens/m0-hello.txt`) — untouched by this packet. `--headless
//! --dump-text <path-or-url> [--cols N]` (P7, M2) drives the full
//! fetch->parse->cascade->box-tree->layout->tty pipeline and prints the
//! resulting text grid: fetch (`file://`/bare local path or `http://` via
//! the P3 fetch layer) -> `dom::parser::parse` -> `style::cascade::cascade`
//! (no author sheets yet — that's a later packet) -> `layout::box_tree::
//! build_box_tree` -> `layout::layout` -> `backend::tty::render`.
//!
//! There is, and by construction will be, no engine anywhere in this
//! program that runs code shipped by the wire (charter C3).

use stele::backend::raster;
use stele::backend::tty;
use stele::dom;
use stele::fetch::file::FileFetcher;
use stele::fetch::http1::Http1Client;
use stele::fetch::{Fetch, Request, Url};
use stele::frames;
use stele::layout::box_tree::build_box_tree;
use stele::layout::{self, Size};
use stele::style::cascade;
use stele::surface::{Color, MemSurface};

/// Default terminal width in character cells for `--dump-text` when
/// `--cols` isn't given.
const DEFAULT_COLS: usize = 80;

/// A tall-but-bounded viewport height for headless layout: real height is
/// always content-derived (`layout::block`'s root box stretches to content,
/// never clamped to a fixed viewport height in M2), so this value is never
/// actually load-bearing — see `layout::block::layout_tree`'s doc comments.
const HEADLESS_VIEWPORT_HEIGHT: f32 = 100_000.0;

/// Fixed viewport width (CSS px) `--dump-png` lays out at, absent any CLI
/// flag to override it (M4 pixel foundation scope: no `--width` yet — a
/// later packet can add one). 800px is a common-enough "screenshot" width
/// for a document-web page and, not coincidentally, exactly 100 columns at
/// the 8px-per-column `text::BitmapFont::vga_8x16` cell width `--dump-text`
/// already keys its own layout off of.
const DEFAULT_PNG_WIDTH: u32 = 800;

/// Hard cap on the PNG surface's content-driven height, independent of the
/// document. Mirrors `backend::tty::MAX_GRID_ROWS`'s rationale: a hostile or
/// merely huge document must not drive an unbounded `MemSurface`
/// allocation (`width * height * 4` bytes). 20,000px is a page taller than
/// any real fixture is ever going to produce (`fixtures/basic.html` is a
/// few hundred px) while keeping the worst case (`800 * 20_000 * 4` == 64MB)
/// bounded.
const MAX_PNG_HEIGHT: u32 = 20_000;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Args {
    headless: bool,
    dump_text: Option<String>,
    dump_png: Option<(String, String)>,
    cols: usize,
}

impl Default for Args {
    fn default() -> Self {
        Args { headless: false, dump_text: None, dump_png: None, cols: DEFAULT_COLS }
    }
}

/// Parse `argv` (already stripped of `argv[0]`) into [`Args`]. Total: total
/// over any input, no std dependency beyond `String`/`str` (brief: "don't
/// pull clap"). Unrecognized flags are ignored rather than erroring — a
/// headless text browser for hostile/1996-era fixtures should degrade, not
/// hard-fail, on an unexpected argument.
fn parse_args(argv: &[String]) -> Args {
    let mut out = Args::default();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--headless" => out.headless = true,
            "--dump-text" => {
                i += 1;
                if let Some(v) = argv.get(i) {
                    out.dump_text = Some(v.clone());
                }
            }
            "--dump-png" => {
                // Two positional args follow: <src> <out.png>. A missing
                // trailing value (either or both absent) leaves `dump_png`
                // at `None` rather than partially populating it — matching
                // `--dump-text`'s own "trailing flag, no value" totality.
                let src = argv.get(i + 1).cloned();
                let out_path = argv.get(i + 2).cloned();
                if let (Some(src), Some(out_path)) = (src, out_path) {
                    i += 2;
                    out.dump_png = Some((src, out_path));
                }
            }
            "--cols" => {
                i += 1;
                if let Some(v) = argv.get(i).and_then(|s| s.parse::<usize>().ok()) {
                    out.cols = v;
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Resolve a CLI-supplied source into a fetchable [`Url`]: `http://`/
/// `file://` pass through unchanged; anything else (no recognized scheme,
/// e.g. `fixtures/basic.html` or `/abs/path.html`) is treated as a local
/// filesystem path and turned into an absolute `file://` URL — relative
/// paths are resolved against the current working directory first, since
/// `fetch::file::file_path` expects `file:///abs/path` shaped input (a bare
/// `file://relative/path` would misparse the first path segment as a host).
fn resolve_url(raw: &str) -> Url {
    let scheme = Url::new(raw).scheme();
    if scheme == "http" || scheme == "file" {
        return Url::new(raw);
    }
    let path = std::path::Path::new(raw);
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map(|cwd| cwd.join(path)).unwrap_or_else(|_| path.to_path_buf())
    };
    Url::new(format!("file://{}", abs.display()))
}

/// Fetch `url`'s body over whichever of the two live schemes it names.
/// Every other scheme (including `https`, which this build never serves —
/// no TLS, ever, per the charter) is a clean `Err`, never a panic.
fn fetch_body(url: &Url) -> Result<Vec<u8>, String> {
    match url.scheme().as_str() {
        "file" => FileFetcher::new()
            .fetch(&Request::get(url.clone()))
            .map(|r| r.body)
            .map_err(|e| format!("{e:?}")),
        "http" => Http1Client::new()
            .fetch(&Request::get(url.clone()))
            .map(|r| r.body)
            .map_err(|e| format!("{e:?}")),
        other => Err(format!("unsupported scheme: {other}")),
    }
}

/// Drive the full headless pipeline for `--dump-text`. Total: a fetch
/// error, non-UTF-8 body (lossily recovered), empty document, or
/// `display: none` root all resolve to a clean empty string rather than a
/// panic — the caller prints whatever comes back verbatim.
///
/// Frames (packet `frames`): if the fetched document's `<html>` contains a
/// `<frameset>` anywhere (`stele::frames::find_frameset`), this routes to
/// the frames renderer (`stele::frames::render`) INSTEAD of the ordinary
/// cascade->box-tree->layout->tty chain below — a frameset document has no
/// `<body>` to run that chain over; each `<frame src>` gets its own
/// independent instance of it, recursively, driven from `frames.rs`. See
/// that module's docs for the full design (track sizing, compositing,
/// totality bounds).
fn dump_text(source: &str, cols: usize) -> String {
    let url = resolve_url(source);
    let body = match fetch_body(&url) {
        Ok(b) => b,
        Err(_) => return String::new(),
    };
    let html = String::from_utf8_lossy(&body);
    let dom_tree = dom::parser::parse(&html);

    if let Some(frameset_id) = frames::find_frameset(&dom_tree) {
        return frames::render(&url, &dom_tree, frameset_id, cols).to_text();
    }

    let styles = cascade::cascade(&dom_tree, &[]);
    let Some(root) = build_box_tree(&dom_tree, &styles) else {
        return String::new();
    };
    let viewport = Size { w: cols as f32 * 8.0, h: HEADLESS_VIEWPORT_HEIGHT };
    let fragments = layout::layout(&root, viewport);
    tty::render(&fragments, cols).to_text()
}

/// A minimal valid single white pixel, PNG-encoded — the clean fallback
/// `dump_png` returns for a fetch error, an empty/`display:none` document,
/// or a frameset (pixel rendering of `<frameset>` documents is out of scope
/// for this packet; frames get real pixels in a later one). Mirrors
/// `dump_text`'s own "clean empty string, never a panic" totality contract,
/// just in PNG-shaped terms (there's no equivalent of an empty string for a
/// raster image — a 1x1 blank canvas is the smallest well-formed PNG this
/// module ever needs to produce, and `raster::encode_png` itself keys off
/// exactly this same "1x1 white" fallback for a zero-dimension surface).
fn blank_png() -> Vec<u8> {
    todo!("M4 Part 4: encode a blank 1x1 PNG")
}

/// Drive the full headless pixel pipeline for `--dump-png`: fetch, parse,
/// cascade, box-tree, layout at a fixed-width/content-height viewport, paint
/// fragments onto a `MemSurface`, and PNG-encode it. Total, mirroring
/// `dump_text`'s own contract: a fetch error, empty document, or frameset
/// document all resolve to [`blank_png`] rather than a panic. Frames: same
/// scope call as `blank_png`'s doc comment — a `<frameset>` document has no
/// single `layout::layout` call to drive (see `dump_text`'s own frames
/// carve-out), and wiring the frames compositor to pixels is a follow-up,
/// not this packet's job.
fn dump_png(source: &str) -> Vec<u8> {
    let _ = source;
    todo!("M4 Part 4: drive the headless pixel pipeline")
}

/// `--dump-png <src> <out.png>`'s CLI-facing wrapper: render `source` and
/// write the PNG bytes to `out_path`. The render half ([`dump_png`]) is
/// total (never fails); the only failure mode here is the filesystem write,
/// reported as a clean `Err` rather than a panic (e.g. an unwritable
/// directory, a hostile/invalid `out_path`).
fn write_dump_png(source: &str, out_path: &str) -> Result<(), String> {
    let _ = (source, out_path);
    todo!("M4 Part 4: write the rendered PNG to disk")
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() {
        println!("{}", stele::HELLO_LINE);
        return;
    }

    let args = parse_args(&argv);
    if args.headless {
        if let Some(source) = args.dump_text {
            println!("{}", dump_text(&source, args.cols));
            return;
        }
        if let Some((source, out_path)) = args.dump_png {
            if let Err(e) = write_dump_png(&source, &out_path) {
                eprintln!("stele: --dump-png failed: {e}");
            }
            return;
        }
        eprintln!("stele: --headless requires --dump-text <path-or-url> or --dump-png <path-or-url> <out.png>");
        return;
    }

    // No recognized mode: fall back to the M0 hello rather than erroring —
    // keeps `stele --nonsense` from ever panicking (totality applies to the
    // CLI surface too).
    println!("{}", stele::HELLO_LINE);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_args_defaults_when_empty() {
        let a = parse_args(&[]);
        assert_eq!(a, Args::default());
    }

    #[test]
    fn parse_args_reads_headless_dump_text_and_cols() {
        let a = parse_args(&args(&["--headless", "--dump-text", "fixtures/basic.html", "--cols", "40"]));
        assert!(a.headless);
        assert_eq!(a.dump_text.as_deref(), Some("fixtures/basic.html"));
        assert_eq!(a.cols, 40);
    }

    #[test]
    fn parse_args_defaults_cols_to_80_when_not_given() {
        let a = parse_args(&args(&["--headless", "--dump-text", "x.html"]));
        assert_eq!(a.cols, DEFAULT_COLS);
    }

    #[test]
    fn parse_args_ignores_unrecognized_flags_rather_than_failing() {
        let a = parse_args(&args(&["--wat", "--headless"]));
        assert!(a.headless);
    }

    #[test]
    fn parse_args_trailing_flag_with_missing_value_does_not_panic() {
        let a = parse_args(&args(&["--dump-text"]));
        assert_eq!(a.dump_text, None);
        let a2 = parse_args(&args(&["--cols"]));
        assert_eq!(a2.cols, DEFAULT_COLS);
    }

    #[test]
    fn parse_args_non_numeric_cols_falls_back_to_default() {
        let a = parse_args(&args(&["--cols", "not-a-number"]));
        assert_eq!(a.cols, DEFAULT_COLS);
    }

    #[test]
    fn resolve_url_passes_through_http_and_file_schemes() {
        assert_eq!(resolve_url("http://example.com/x").as_str(), "http://example.com/x");
        assert_eq!(resolve_url("file:///abs/path.html").as_str(), "file:///abs/path.html");
    }

    #[test]
    fn resolve_url_turns_a_bare_relative_path_into_an_absolute_file_url() {
        let url = resolve_url("fixtures/basic.html");
        assert_eq!(url.scheme(), "file");
        assert!(url.as_str().ends_with("fixtures/basic.html"));
        assert!(url.path().starts_with('/'), "resolved to an absolute path: {}", url.path());
    }

    #[test]
    fn resolve_url_turns_a_bare_absolute_path_into_a_file_url() {
        let url = resolve_url("/abs/path.html");
        assert_eq!(url.scheme(), "file");
        assert_eq!(url.path(), "/abs/path.html");
    }

    #[test]
    fn dump_text_over_file_fetch_matches_the_tty_golden() {
        let golden = include_str!("../goldens/basic.tty.txt");
        let text = dump_text("fixtures/basic.html", 80);
        assert_eq!(text, golden.trim_end_matches('\n'));
    }

    #[test]
    fn dump_text_on_a_missing_file_is_a_clean_empty_string_not_a_panic() {
        assert_eq!(dump_text("fixtures/does-not-exist-nope.html", 80), "");
    }

    #[test]
    fn dump_text_on_an_unsupported_scheme_is_a_clean_empty_string() {
        assert_eq!(dump_text("ftp://example.com/x", 80), "");
    }

    #[test]
    fn narrower_cols_clip_wide_lines_more_than_default() {
        let narrow = dump_text("fixtures/basic.html", 10);
        let wide = dump_text("fixtures/basic.html", 80);
        assert_ne!(narrow, wide);
        for line in narrow.lines() {
            assert!(line.chars().count() <= 10, "line exceeds requested cols: {line:?}");
        }
    }

    // ------------------------------------------------------------- --dump-png

    #[test]
    fn parse_args_reads_dump_png_source_and_out_path() {
        let a = parse_args(&args(&["--headless", "--dump-png", "fixtures/basic.html", "/tmp/out.png"]));
        assert!(a.headless);
        assert_eq!(a.dump_png, Some(("fixtures/basic.html".to_string(), "/tmp/out.png".to_string())));
    }

    #[test]
    fn parse_args_dump_png_missing_out_path_does_not_panic_or_partially_set() {
        let a = parse_args(&args(&["--dump-png", "fixtures/basic.html"]));
        assert_eq!(a.dump_png, None);
    }

    fn decode_png_dims(bytes: &[u8]) -> (u32, u32) {
        let decoder = png::Decoder::new(bytes);
        let reader = decoder.read_info().expect("dump_png must always produce a valid PNG");
        (reader.info().width, reader.info().height)
    }

    #[test]
    fn dump_png_over_file_fetch_produces_a_valid_png_at_the_default_width() {
        let bytes = dump_png("fixtures/basic.html");
        assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']));
        let (w, h) = decode_png_dims(&bytes);
        assert_eq!(w, DEFAULT_PNG_WIDTH);
        assert!(h > 0, "content-driven height should be nonzero for a real document");
    }

    #[test]
    fn dump_png_on_a_missing_file_is_a_clean_blank_png_not_a_panic() {
        let bytes = dump_png("fixtures/does-not-exist-nope.html");
        assert_eq!(bytes, blank_png());
    }

    #[test]
    fn dump_png_on_an_unsupported_scheme_is_a_clean_blank_png() {
        let bytes = dump_png("ftp://example.com/x");
        assert_eq!(bytes, blank_png());
    }

    #[test]
    fn dump_png_on_a_frameset_document_is_a_clean_blank_png() {
        // Pixel rendering of framesets is out-of-scope for this packet (see
        // dump_png's doc comment) -- must degrade cleanly, not panic.
        let bytes = dump_png("fixtures/frames.html");
        assert_eq!(bytes, blank_png());
    }

    #[test]
    fn write_dump_png_writes_valid_png_bytes_to_disk() {
        let out = std::env::temp_dir().join(format!("stele-test-{}.png", std::process::id()));
        let out_str = out.to_string_lossy().to_string();
        write_dump_png("fixtures/basic.html", &out_str).expect("write should succeed");
        let on_disk = std::fs::read(&out).expect("file should exist");
        assert_eq!(on_disk, dump_png("fixtures/basic.html"));
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn write_dump_png_to_an_unwritable_path_is_a_clean_err_not_a_panic() {
        let result = write_dump_png("fixtures/basic.html", "/nonexistent-dir-xyz/out.png");
        assert!(result.is_err());
    }

    #[test]
    fn blank_png_is_a_valid_1x1_png() {
        let bytes = blank_png();
        let (w, h) = decode_png_dims(&bytes);
        assert_eq!((w, h), (1, 1));
    }
}
