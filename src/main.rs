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

use stele::backend::tty;
use stele::dom;
use stele::fetch::file::FileFetcher;
use stele::fetch::http1::Http1Client;
use stele::fetch::{Fetch, Request, Url};
use stele::layout::box_tree::build_box_tree;
use stele::layout::{self, Size};
use stele::style::cascade;

/// Default terminal width in character cells for `--dump-text` when
/// `--cols` isn't given.
const DEFAULT_COLS: usize = 80;

/// A tall-but-bounded viewport height for headless layout: real height is
/// always content-derived (`layout::block`'s root box stretches to content,
/// never clamped to a fixed viewport height in M2), so this value is never
/// actually load-bearing — see `layout::block::layout_tree`'s doc comments.
const HEADLESS_VIEWPORT_HEIGHT: f32 = 100_000.0;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Args {
    headless: bool,
    dump_text: Option<String>,
    cols: usize,
}

impl Default for Args {
    fn default() -> Self {
        Args { headless: false, dump_text: None, cols: DEFAULT_COLS }
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
fn resolve_url(_raw: &str) -> Url {
    todo!("P7 RED: resolve_url")
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
fn dump_text(_source: &str, _cols: usize) -> String {
    todo!("P7 RED: dump_text")
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() {
        println!("{}", stele::HELLO_LINE);
        return;
    }

    let args = parse_args(&argv);
    if args.headless {
        match args.dump_text {
            Some(source) => println!("{}", dump_text(&source, args.cols)),
            None => eprintln!("stele: --headless requires --dump-text <path-or-url>"),
        }
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
}
