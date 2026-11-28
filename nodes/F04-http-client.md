# F04 — http-client

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | F01, F03 | none | M |

**Owns:** `src/fetch/http1.rs`, `src/fetch/https.rs`

## Purpose

Imported from the code. The bespoke, blocking HTTP/1.1 client (std only, DECISIONS.md) and its delegated
TLS: `https:` is served by running the user's own `openssl s_client` as a child process, so the binary
embeds no cryptography (charter C2, DECISIONS.md D53). The two files are one unit: `http1` calls
`https::connect` for an `https:` URL, and both drive the `ByteStream` seam of F01.

`src/fetch/http1.rs`:

- `Http1Client` owns a `CookieJar` (F03); `new()` starts with an empty one, `with_cookie_jar(jar)` with a
  given one. Its `Fetch::fetch` accepts `http` and `https` only (`UnsupportedScheme` otherwise).
- Each hop sends the jar's `Cookie:` value for that hop's URL and stores every `Set-Cookie` against that
  hop's URL. A 301, 302, 303, 307 or 308 is followed, to at most `MAX_REDIRECTS` (5) redirects, after which
  the answer is `TooManyRedirects`; a redirect without `Location` is a `Protocol` error. `Location` is
  resolved against the hop's URL. A 303, and a 301 or 302 answering a POST, turn the request into a GET
  without a body; a 307 or 308 keeps the method and body. When the body is gone, a caller's `Content-Type`
  is dropped. An `https` to `http` redirect is followed with a warning on stderr. The `Response` carries the
  last hop's status, URL, headers and body.
- The request is `<METHOD> <path>[?query] HTTP/1.1` (an empty path is `/`), with `Host` (the port only when
  it is not the scheme's default), `User-Agent: Stele/0.1`, `Accept: */*`, `Accept-Encoding: identity`
  (no gzip yet), `Connection: close`, for a POST a default `Content-Type:
  application/x-www-form-urlencoded` unless the caller gave one plus `Content-Length`, and the cookie
  header. Caller headers that would duplicate these are skipped, and a caller header whose name or value
  holds a CR or LF is dropped whole.
- Plain HTTP opens a `TcpStream` with 15-second read and write timeouts. The response parse is total:
  the status line must start with `HTTP/` and carry a numeric code; header lines without `:` or with an
  empty name are skipped; folded continuation lines join the previous value. The body is chunked (with
  trailers), `Content-Length` (a short body is returned as far as it got), or read to EOF. 64 MiB bounds
  a response: a longer `Content-Length` body is cut at 64 MiB, and larger headers, EOF bodies or chunked
  bodies are `Protocol` errors.

`src/fetch/https.rs`:

- `resolve_ca_file`: `$STELE_CA_FILE` when set and not empty (an error naming the path when it does not
  exist), else the first existing of four distribution CA-bundle paths, else an error listing them.
- `probe`: once per process (cached), runs `openssl s_client -help` with stdin from `/dev/null`, gives up
  after 10 seconds, and requires each of `-connect`, `-servername`, `-verify_return_error`,
  `-verify_hostname`, `-verify_ip`, `-CAfile` and `-quiet` in its output. Any gap means https is not
  available (fail closed).
- `connect(host, port)` probes, resolves the CA file, and spawns `openssl s_client -quiet -connect
  host:port -servername host -verify_return_error -CAfile <ca>` plus `-verify_ip host` for an IP literal or
  `-verify_hostname host` otherwise. `OpensslStream` is the child as a `ByteStream`: writes go to its stdin,
  each read of its stdout waits at most 15 seconds (`poll`), and `shutdown_write` closes stdin. The HTTPS
  path closes stdin after the request; `-quiet` implies `-ign_eof`, so the connection stays open for the
  response. Dropping the stream kills and reaps the child.
- On an exchange error, `tls_error_or` reads up to 64 KiB of the child's stderr before waiting on it. When
  the child exited non-zero, the error is `Tls("TLS connection to <host> failed (<openssl's reason>).
  Nothing was fetched.")`; otherwise, and for a read timeout, the original error stands.
- Every TLS failure message is one sentence that ends `Nothing was fetched.`

## Tests

`tests/fetch_http1.rs` (17 tests against the in-process fixture server of `tests/support/mod.rs` and its
`spawn_raw` responder: bodies, chunking, POST, redirects, the method rules, https dispatch, totality, and a
cookie round trip). `tests/fetch_https.rs` (5 of its 6 tests, against a local `openssl s_server` with a
generated test CA; they need a system `openssl`). The `tests` modules in `src/fetch/http1.rs` (3 tests:
CR/LF header injection) and `src/fetch/https.rs` (4 tests: `$STELE_CA_FILE`, the flag probe with a stub
`openssl`, a dead port). No test reaches the external network.

## Changes

none: imported

## Gate

- **F04-AC-1** — A `Content-Length` response is returned whole: `/content-length` gives its body and `Content-Type`, and `/fixtures/basic.html` the file's bytes. *Verify:* `cargo test --locked --jobs 2 --test fetch_http1 -- --test-threads=2 get_with_content_length_body get_fixture_file_through_the_server` exits 0.
- **F04-AC-2** — A chunked response is decoded to `Hello, chunked world!`. *Verify:* `cargo test --locked --jobs 2 --test fetch_http1 -- --test-threads=2 chunked_response_is_decoded` exits 0.
- **F04-AC-3** — A POST sends its body with the default `Content-Type: application/x-www-form-urlencoded`, and a caller's own `Content-Type` replaces that default. *Verify:* `cargo test --locked --jobs 2 --test fetch_http1 -- --test-threads=2 post_` exits 0.
- **F04-AC-4** — A chain of 3 redirects is followed to `/redirect/0`, which is the `final_url`; a chain of 5 still succeeds and a chain of 6 is `TooManyRedirects`. *Verify:* `cargo test --locked --jobs 2 --test fetch_http1 -- --test-threads=2 redirect_` exits 0.
- **F04-AC-5** — A 303 turns a POST into a GET without a body, a 301 turns a POST into a GET, and a 307 keeps the POST and its body. *Verify:* `cargo test --locked --jobs 2 --test fetch_http1 -- --test-threads=2 status_` exits 0.
- **F04-AC-6** — `Http1Client` sends an `https:` URL to the TLS transport instead of refusing it. *Verify:* `cargo test --locked --jobs 2 --test fetch_http1 -- --test-threads=2 https_scheme_is_dispatched_not_unsupported` exits 0.
- **F04-AC-7** — A response that is not HTTP, headers cut off before the blank line, and a bad chunk size are errors, and a `Content-Length` longer than the body sent does not panic. *Verify:* `cargo test --locked --jobs 2 --test fetch_http1 -- --test-threads=2 never_panics` exits 0.
- **F04-AC-8** — A caller header with CR or LF in its name or value never reaches the request, not even in part, and a plain custom header is sent. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::http1::tests::` exits 0.
- **F04-AC-9** — A cookie set by one response is sent with the next request of the same client. *Verify:* `cargo test --locked --jobs 2 --test fetch_http1 -- --test-threads=2 cookies_round_trip_through_a_real_hop` exits 0.
- **F04-AC-10** — An `https:` fetch of `127.0.0.1` against a server whose certificate the given CA signs for `IP:127.0.0.1` returns the body (an IP literal is verified as an IP). *Verify:* `cargo test --locked --jobs 2 --test fetch_https -- --test-threads=2 trusted_https_fetch_renders_the_body` exits 0.
- **F04-AC-11** — A certificate from another CA is refused with a `Tls` error that says `Nothing was fetched`, and a certificate for another name is refused with a `Tls` error. *Verify:* `cargo test --locked --jobs 2 --test fetch_https -- --test-threads=2 untrusted_ca_is_a_legible_tls_error hostname_mismatch_is_refused` exits 0.
- **F04-AC-12** — A TLS body line starting with `Q` arrives intact. *Verify:* `cargo test --locked --jobs 2 --test fetch_https -- --test-threads=2 body_line_starting_with_Q_survives_the_quiet_trap` exits 0.
- **F04-AC-13** — `$STELE_CA_FILE` naming an existing file is the CA file, and one naming a missing file is a `Tls` error that names the path and says `Nothing was fetched`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::https::tests::stele_ca_file_` exits 0.
- **F04-AC-14** — An `openssl` whose `s_client -help` lacks `-verify_ip` makes https unavailable, with a message naming the flag and openssl. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::https::tests::probe_fails_closed_when_a_required_flag_is_missing` exits 0.
- **F04-AC-15** — Connecting to a port where nothing listens, and reading from it, does not panic. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::https::tests::connect_to_a_dead_port_is_a_legible_tls_error_not_a_panic` exits 0.
- **F04-AC-16** — A TLS response with neither `Content-Length` nor chunking is read until the server closes the connection. *Verify:* `cargo test --locked --jobs 2 --test fetch_https -- --test-threads=2 close_delimited_response_terminates_via_stdin_eof` exits 0.
- **F04-AC-17** — The request line and fixed headers (`Host` without a default port, `User-Agent`, `Accept-Encoding: identity`, `Connection: close`); cookies sent and stored at every redirect hop against that hop's URL; the dropped `Content-Type` after a redirect empties the body; the `https` to `http` warning; closing the openssl child's stdin after the request (the close-delimited test passes without it); the 15-second timeouts; the 64 MiB bounds; folded header lines; the default CA paths and their order; the probe's 10-second limit and its cache; killing and reaping the child on drop. *Verify:* unpinned.

## Risks

- HTTPS depends on the user's `openssl` binary at run time: a missing, old or LibreSSL `openssl` makes every
  `https:` fetch fail (closed, with a message). `tests/fetch_https.rs` needs `openssl` on the machine that
  runs the gate.
- Every request opens a new connection (`Connection: close`) and, for HTTPS, a new child process; there is no
  keep-alive and no connection reuse.
- No gzip: `Accept-Encoding: identity` is sent, and a server that compresses anyway hands back compressed
  bytes. `Response`'s doc comment in F01 says gzip is already inflated; the code does not inflate.
- A `Content-Length` body is silently cut at 64 MiB and a short body is returned as if complete.
- A redirect from `https` to `http` is followed (only a stderr warning), and the cookie jar sends non-`Secure`
  cookies on the `http` hop.
- `https.rs`'s tests change process environment variables (`STELE_CA_FILE`, `PATH`) under a lock of their own;
  another test in the same binary that reads them without that lock can race.
- The read timeout covers each read, not the whole exchange; a peer that trickles bytes can hold a fetch open.
