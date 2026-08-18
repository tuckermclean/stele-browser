//! URL parsing + relative-reference resolution (P3, brief §10: contract-
//! tested, strict test-first). No network — pure string-to-parts logic.

use stele::fetch::Url;

#[test]
fn parses_scheme_host_port_path_query() {
    let u = Url::new("http://example.com:8080/a/b?x=1");
    assert_eq!(u.scheme(), "http");
    assert_eq!(u.host(), "example.com");
    assert_eq!(u.port(80), 8080);
    assert_eq!(u.path(), "/a/b");
    assert_eq!(u.query().as_deref(), Some("x=1"));
}

#[test]
fn default_port_is_used_when_absent() {
    let u = Url::new("http://example.com/a");
    assert_eq!(u.port(80), 80);
    assert_eq!(u.query(), None);
}

#[test]
fn scheme_is_lowercased() {
    let u = Url::new("HTTP://Example.com/");
    assert_eq!(u.scheme(), "http");
}

#[test]
fn path_only_no_query() {
    let u = Url::new("http://example.com/a/b/c");
    assert_eq!(u.path(), "/a/b/c");
    assert_eq!(u.query(), None);
}

#[test]
fn malformed_url_never_panics_and_degrades_gracefully() {
    for raw in ["", "not a url at all", "http://", ":::", "http:///no-host-path"] {
        let u = Url::new(raw);
        // Must not panic; components best-effort (possibly empty).
        let _ = (u.scheme(), u.host(), u.port(0), u.path(), u.query());
    }
}

// -- relative-reference resolution (RFC 3986 §5.4-style cases) -------------

fn base() -> Url {
    // Mirrors RFC 3986 §5.1's example base, adapted to a resolvable authority.
    Url::new("http://a/b/c/d;p?q")
}

#[test]
fn resolve_relative_path_against_directory() {
    assert_eq!(base().resolve("g").as_str(), "http://a/b/c/g");
    assert_eq!(base().resolve("./g").as_str(), "http://a/b/c/g");
    assert_eq!(base().resolve("g/").as_str(), "http://a/b/c/g/");
}

#[test]
fn resolve_absolute_path_reference() {
    assert_eq!(base().resolve("/g").as_str(), "http://a/g");
}

#[test]
fn resolve_network_path_reference() {
    assert_eq!(base().resolve("//g").as_str(), "http://g");
    assert_eq!(base().resolve("//g/x").as_str(), "http://g/x");
}

#[test]
fn resolve_query_only_reference_keeps_base_path() {
    assert_eq!(base().resolve("?y").as_str(), "http://a/b/c/d;p?y");
}

#[test]
fn resolve_absolute_reference_ignores_base() {
    assert_eq!(
        base().resolve("http://other/x").as_str(),
        "http://other/x"
    );
}

#[test]
fn resolve_dot_dot_segments_walk_up_the_path() {
    assert_eq!(base().resolve("../../g").as_str(), "http://a/g");
    assert_eq!(base().resolve("../g").as_str(), "http://a/b/g");
}

#[test]
fn resolve_fragment_is_dropped() {
    assert_eq!(base().resolve("g#s").as_str(), "http://a/b/c/g");
}

#[test]
fn resolve_typical_redirect_location_headers() {
    let b = Url::new("http://127.0.0.1:9000/redirect/3");
    assert_eq!(
        b.resolve("/redirect/2").as_str(),
        "http://127.0.0.1:9000/redirect/2"
    );
}

// -- `file://` resolution (packet/fix-local-img-loading) -------------------
//
// Investigating a "local <img> renders blank" report, these pin down that
// `Url::resolve` and `fetch::file::file_path` (see `tests/fetch_file.rs`)
// already do the right thing for a `file://` base — a RELATIVE `<img src>`
// resolves against the document's own directory (not the repo tree, not the
// process's cwd — directory-INDEPENDENT), and an ABSOLUTE `file://` `<img
// src>` passes through unchanged, exactly the two shapes the report called
// out. The actual root cause turned out to live elsewhere (an `<img>` with
// no `width`/`height` attribute got a 0x0 intrinsic size even after a
// successful decode — see `src/layout/box_tree.rs`'s `replaced_intrinsic`
// and its doc comment) — these cases are a real, independently-useful
// contract regardless, and a regression guard against this exact hypothesis
// ever becoming true.

#[test]
fn resolve_relative_file_reference_against_an_arbitrary_tmp_directory() {
    // Directory-independence is the point: nothing here is under the repo
    // tree, and the base is a real multi-segment `/tmp/...` path, not `/`.
    let base = Url::new("file:///tmp/dir/doc.html");
    assert_eq!(base.resolve("pic.png").as_str(), "file:///tmp/dir/pic.png");
}

#[test]
fn resolve_relative_file_reference_against_a_deeper_arbitrary_directory() {
    let base = Url::new("file:///tmp/a/b/c/doc.html");
    assert_eq!(base.resolve("pic.png").as_str(), "file:///tmp/a/b/c/pic.png");
}

#[test]
fn resolve_an_absolute_file_reference_passes_through_unchanged_regardless_of_base() {
    let base = Url::new("file:///tmp/dir/doc.html");
    assert_eq!(
        base.resolve("file:///other/pic.png").as_str(),
        "file:///other/pic.png"
    );
    // Same result no matter where the document itself lives — this is what
    // "regardless of the document's directory" means for an absolute ref.
    let repo_base = Url::new("file:///home/someone/repo/fixtures/doc.html");
    assert_eq!(
        repo_base.resolve("file:///other/pic.png").as_str(),
        "file:///other/pic.png"
    );
}

#[test]
fn resolve_relative_file_reference_into_a_nested_sibling_directory() {
    let base = Url::new("file:///tmp/dir/doc.html");
    assert_eq!(base.resolve("images/pic.png").as_str(), "file:///tmp/dir/images/pic.png");
}
