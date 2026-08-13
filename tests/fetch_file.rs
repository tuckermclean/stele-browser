//! `file://` fetch (P3). No network — local filesystem only, using the
//! checked-in fixtures/ corpus plus a scratch temp file for extension cases.

use std::io::Write;

use stele::fetch::file::FileFetcher;
use stele::fetch::{Fetch, FetchError, Method, Request, Url};

fn fixtures_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

#[test]
fn reads_existing_html_fixture_with_html_content_type() {
    let path = fixtures_dir().join("basic.html");
    let expected = std::fs::read(&path).expect("fixture must exist on disk");
    let url = Url::new(format!("file://{}", path.display()));
    let mut fetcher = FileFetcher::new();
    let resp = fetcher
        .fetch(&Request::get(url.clone()))
        .expect("existing file must fetch OK");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, expected);
    assert_eq!(resp.header("Content-Type"), Some("text/html"));
    assert_eq!(resp.final_url, url);
}

#[test]
fn missing_file_is_a_fetch_error_not_a_panic() {
    let url = Url::new("file:///definitely/does/not/exist/on/this/machine.html");
    let mut fetcher = FileFetcher::new();
    let result = fetcher.fetch(&Request::get(url));
    assert!(result.is_err());
}

#[test]
fn content_type_by_extension_txt() {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("stele-p3-test-{}.txt", std::process::id()));
    {
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"hello from a text fixture\n").unwrap();
    }
    let url = Url::new(format!("file://{}", path.display()));
    let mut fetcher = FileFetcher::new();
    let resp = fetcher.fetch(&Request::get(url)).unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("Content-Type"), Some("text/plain"));
    assert_eq!(resp.body, b"hello from a text fixture\n");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn non_file_scheme_is_unsupported() {
    let url = Url::new("http://example.com/");
    let mut fetcher = FileFetcher::new();
    let result = fetcher.fetch(&Request {
        method: Method::Get,
        url,
        headers: Vec::new(),
        body: Vec::new(),
    });
    match result {
        Err(FetchError::UnsupportedScheme(_)) => {}
        other => panic!("expected UnsupportedScheme, got {:?}", other),
    }
}
