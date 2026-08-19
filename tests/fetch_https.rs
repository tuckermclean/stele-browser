//! Delegated-TLS integration tests. Everything talks to an in-process
//! `openssl s_server` with a generated test CA — NEVER the external network
//! (house law). These require a system `openssl` with s_client/s_server; if it
//! is absent, the tls_certs generation asserts and the suite fails loudly,
//! which is correct on CI (openssl IS present there).

mod support;
use support::{spawn_tls_responder, tls_certs_with};

use stele::fetch::{fetch, FetchError, Request, Url};

const OK_RESPONSE: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nhello over tls\n";

// The fetch path spawns an `openssl` child that snapshots the process
// environment at spawn time. `STELE_CA_FILE` is set per-test to point at
// that test's own generated CA, so tests that mutate it must not run
// concurrently — cargo runs tests in parallel threads by default, and two
// tests racing to set/clear this var would clobber each other. This lock is
// held across the WHOLE body below, not just the env mutation, because the
// fetch (and the openssl child it spawns) must observe the env var that
// belongs to ITS OWN call.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn get(url: &str, ca: &std::path::Path) -> Result<stele::fetch::Response, FetchError> {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    std::env::set_var("STELE_CA_FILE", ca);
    let r = fetch(&Request::get(Url::new(url)));
    std::env::remove_var("STELE_CA_FILE");
    r
}

#[test]
fn trusted_https_fetch_renders_the_body() {
    let certs = tls_certs_with("localhost", "IP:127.0.0.1", "trusted");
    let port = spawn_tls_responder(&certs.leaf_cert, &certs.leaf_key, OK_RESPONSE);
    let resp = get(&format!("https://127.0.0.1:{port}/"), &certs.ca_cert)
        .expect("trusted TLS fetch succeeds");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"hello over tls\n");
}

#[test]
fn untrusted_ca_is_a_legible_tls_error() {
    let certs = tls_certs_with("localhost", "IP:127.0.0.1", "untrusted");
    let port = spawn_tls_responder(&certs.leaf_cert, &certs.leaf_key, OK_RESPONSE);
    // Verify against the UNRELATED CA → openssl rejects.
    let err = get(&format!("https://127.0.0.1:{port}/"), &certs.other_ca)
        .expect_err("untrusted CA must fail closed");
    match err {
        FetchError::Tls(m) => assert!(m.contains("Nothing was fetched"), "message: {m}"),
        other => panic!("expected Tls, got {other:?}"),
    }
}

#[test]
fn hostname_mismatch_is_refused() {
    // Leaf is valid but for the wrong name (no 127.0.0.1 SAN) → -verify_hostname fails.
    let certs = tls_certs_with("wrong.example", "DNS:wrong.example", "mismatch");
    let port = spawn_tls_responder(&certs.leaf_cert, &certs.leaf_key, OK_RESPONSE);
    let err = get(&format!("https://127.0.0.1:{port}/"), &certs.ca_cert)
        .expect_err("hostname mismatch must be refused");
    assert!(matches!(err, FetchError::Tls(_)), "got {err:?}");
}

#[test]
fn body_line_starting_with_Q_survives_the_quiet_trap() {
    // Without -quiet, an s_client body line starting with 'Q' closes the
    // connection. -quiet is on, so the 'Q' line must arrive intact.
    const Q_BODY: &[u8] =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 9\r\n\r\nQuit\nyes\n";
    let certs = tls_certs_with("localhost", "IP:127.0.0.1", "qtrap");
    let port = spawn_tls_responder(&certs.leaf_cert, &certs.leaf_key, Q_BODY);
    let resp = get(&format!("https://127.0.0.1:{port}/"), &certs.ca_cert)
        .expect("Q-line body must survive");
    assert_eq!(resp.body, b"Quit\nyes\n");
}

#[test]
fn close_delimited_response_terminates_via_stdin_eof() {
    // No Content-Length, no chunked: body is delimited by connection close.
    // Our shutdown_write (stdin EOF, -no_ign_eof) + server close must let the
    // read complete rather than hang to the timeout.
    const CLOSE_DELIMITED: &[u8] =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nclosed-body";
    let certs = tls_certs_with("localhost", "IP:127.0.0.1", "closeeof");
    let port = spawn_tls_responder(&certs.leaf_cert, &certs.leaf_key, CLOSE_DELIMITED);
    let resp = get(&format!("https://127.0.0.1:{port}/"), &certs.ca_cert)
        .expect("close-delimited body terminates");
    assert_eq!(resp.body, b"closed-body");
}

#[test]
fn secure_cookie_set_over_https_is_not_sent_over_http() {
    use stele::fetch::cookies::CookieJar;
    let mut jar = CookieJar::new();
    jar.set_from_header(&Url::new("https://example.com/"), "sid=abc; Secure; Path=/");
    // Over https: sent.
    assert_eq!(jar.header_for(&Url::new("https://example.com/")).as_deref(), Some("sid=abc"));
    // Over http: withheld (Secure).
    assert_eq!(jar.header_for(&Url::new("http://example.com/")), None);
}
