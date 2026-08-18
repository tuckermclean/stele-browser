//! M5 (dialect-completeness) `--stats` CLI integration test: spawns the REAL
//! compiled `stele` binary (unlike every other test in this repo, which
//! calls the library's own functions directly — this is the one place the
//! stdout/stderr SEPARATION claim actually needs a real process boundary to
//! prove, since Rust's own test harness has no clean way to intercept a
//! sibling in-process `eprintln!`/`println!` pair independently).
//!
//! Proves: `--stats` output goes to STDERR only (never perturbs the
//! `--dump-text` stdout a golden diffs against), and the printed line
//! matches the aggregated counts for a document with known ignored-CSS
//! content (mirrors the packet brief's own worked example: 2 unknown
//! declarations + 1 `@import`).

use std::io::Write;
use std::process::Command;

/// A tiny fixture with a known, hand-countable set of refused CSS: two
/// unrecognized declarations (`flibbertigibbet`, `wobble`) and one
/// `@import` at-rule, alongside one ordinary recognized declaration
/// (`color: red`) so the sheet isn't trivially all-refused.
const STATS_FIXTURE_HTML: &str = r#"<!doctype html>
<html><head><style>
  p { flibbertigibbet: 1; color: red; wobble: 2; }
  @import url(unused.css);
</style></head>
<body><p>hi</p></body></html>
"#;

fn write_fixture() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("stele-stats-cli-{}.html", std::process::id()));
    let mut f = std::fs::File::create(&path).expect("create scratch fixture");
    f.write_all(STATS_FIXTURE_HTML.as_bytes()).expect("write scratch fixture");
    path
}

#[test]
fn stats_flag_prints_the_aggregated_line_to_stderr_and_leaves_stdout_unchanged() {
    let fixture = write_fixture();

    let without_stats = Command::new(env!("CARGO_BIN_EXE_stele"))
        .args(["--headless", "--dump-text"])
        .arg(&fixture)
        .output()
        .expect("run stele --dump-text");
    assert!(without_stats.status.success());

    let with_stats = Command::new(env!("CARGO_BIN_EXE_stele"))
        .args(["--headless", "--dump-text"])
        .arg(&fixture)
        .arg("--stats")
        .output()
        .expect("run stele --dump-text --stats");
    assert!(with_stats.status.success());

    // stdout: byte-for-byte identical whether or not --stats was passed --
    // the render output (what a golden diffs against) must never change.
    assert_eq!(
        without_stats.stdout, with_stats.stdout,
        "--stats must not perturb stdout (the golden-compared render output)"
    );

    // --stats prints NOTHING on stderr when absent...
    assert!(without_stats.stderr.is_empty(), "no --stats flag -> no stats line on stderr");

    // ...and the exact aggregated line when present, on stderr only.
    let stderr = String::from_utf8_lossy(&with_stats.stderr);
    assert_eq!(
        stderr.trim_end(),
        "stele --stats: 2 ignored declarations, 1 ignored at-rule, 0 media blocks, 0 missing glyphs",
        "unexpected --stats stderr line: {stderr:?}"
    );
    assert!(
        !String::from_utf8_lossy(&with_stats.stdout).contains("--stats"),
        "the stats line must never leak into stdout"
    );

    let _ = std::fs::remove_file(&fixture);
}

#[test]
fn stats_flag_with_no_author_css_prints_all_zeros() {
    let path = std::env::temp_dir().join(format!("stele-stats-cli-zeros-{}.html", std::process::id()));
    std::fs::write(&path, "<p>no author css here</p>").expect("write scratch fixture");

    let out = Command::new(env!("CARGO_BIN_EXE_stele"))
        .args(["--headless", "--dump-text"])
        .arg(&path)
        .arg("--stats")
        .output()
        .expect("run stele --dump-text --stats");
    assert!(out.status.success());

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(stderr.trim_end(), "stele --stats: 0 ignored declarations, 0 ignored at-rules, 0 media blocks, 0 missing glyphs");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn stats_flag_counts_missing_glyphs_for_unmappable_characters() {
    // packet t2-glyph-fallback: a document with genuinely-unmappable
    // characters (an emoji and a CJK pair -- neither atlas-covered nor
    // transliterable, per `text::translit`'s documented resolution order)
    // must report a nonzero missing-glyph count, while its ASCII-only
    // sibling above reports zero -- proves the counter is wired through the
    // real CLI, not just the pure library-level helpers `main.rs`'s own unit
    // tests already cover.
    let path = std::env::temp_dir().join(format!("stele-stats-cli-missing-glyphs-{}.html", std::process::id()));
    std::fs::write(&path, "<p>emoji: \u{1F600} cjk: \u{65E5}\u{672C}</p>").expect("write scratch fixture");

    let out = Command::new(env!("CARGO_BIN_EXE_stele"))
        .args(["--headless", "--dump-text"])
        .arg(&path)
        .arg("--stats")
        .output()
        .expect("run stele --dump-text --stats");
    assert!(out.status.success());

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        stderr.trim_end(),
        "stele --stats: 0 ignored declarations, 0 ignored at-rules, 0 media blocks, 3 missing glyphs",
        "unexpected --stats stderr line: {stderr:?}"
    );

    let _ = std::fs::remove_file(&path);
}
