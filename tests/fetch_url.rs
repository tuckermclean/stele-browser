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
