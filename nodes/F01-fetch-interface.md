# F01 — fetch-interface

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | none | F02, F03, F04, F05 | M |

**Owns:** `src/fetch/mod.rs`, `src/fetch/url.rs`, `src/fetch/transport.rs`

## Purpose

Imported from the code. `stele::fetch` is how every part of the browser gets bytes. Its module root holds
the shape every caller holds, the one scheme table, and the error text. `fetch::url` parses and resolves
URLs. `fetch::transport` is the byte-stream seam under the HTTP client.

In `src/fetch/mod.rs`:

- `Url`, a newtype over the raw string (`Url::new`, `as_str`). `Method` is `Get` or `Post` only (the
  dialect). `Request` holds a method, a URL, extra headers and a body (empty for GET); `Request::get`
  builds a GET with no headers. `Response` holds the status, the `final_url` after redirects, the headers
  and the body; `Response::header(name)` returns the first header whose name matches, ignoring ASCII case.
- `FetchError`: `Io`, `Protocol`, `TooManyRedirects`, `UnsupportedScheme(scheme)` and `Tls(sentence)`,
  where a `Tls` string is a whole user-facing sentence.
- The `Fetch` trait (`fetch(&mut self, &Request)`), implemented by `file::FileFetcher` (F02) and
  `http1::Http1Client` (F04).
- `fetch(request)`, the single scheme table every driver-level caller goes through: `about` to
  `about::fetch` (F05), `data` to `data::fetch` (F02), `file` to a fresh `FileFetcher`, `http` and `https` to
  a fresh `Http1Client` (so a fresh cookie jar per call), and any other scheme to
  `UnsupportedScheme(<scheme>)`.
- `err_to_string(err)`: a `Tls` error is its sentence alone, `UnsupportedScheme(s)` reads
  `unsupported scheme: <s>`, and every other error is its `Debug` form.
- The `mod` lines of the area's other files, including the generated `attestations_data` and the
  `#[cfg(test)]` sibling `attestations_tests` (F05).

In `src/fetch/url.rs`, methods on `Url`, all total (malformed input degrades, never panics):

- `parts()` parses `scheme:[//[userinfo@]host[:port]]path[?query][#fragment]` into `UrlParts`: the scheme
  (letters first, then letters, digits, `+`, `-`, `.`) lowercased; the host as written, after any
  `userinfo@`; the port if it parses as a `u16` (an explicit `:80` is kept, not normalised away); the path,
  empty when the URL has none; the query without its `?`; the fragment dropped. A string with no valid
  scheme is all path. `scheme()`, `host()`, `port(default)`, `path()` and `query()` read the parts.
- `resolve(reference)` resolves a reference against the URL as base, RFC 3986 §5 simplified: the fragment is
  dropped; a reference with a scheme is returned as is; `//authority` takes the base's scheme; an empty
  reference keeps the base path and query; `?q` keeps the base path; `/path` replaces the path; any other
  reference is merged with the base path's directory. Dot segments are removed with a bounded loop. The
  result is rebuilt as `scheme://host[:port]path[?query]`, so a `file:///dir/doc.html` base resolves to
  `file:///dir/<ref>`.

In `src/fetch/transport.rs`: the `ByteStream` trait (`Read + Write + shutdown_write`), implemented for
`TcpStream` (`shutdown(Write)`) here and for the openssl child in F04. The plain-HTTP path does not call
`shutdown_write`.

## Tests

`tests/fetch_url.rs` (20 tests: parsing, the default and explicit port, case, the empty path, totality,
RFC 3986-style resolution, a redirect `Location`, `file://` resolution). The `dispatch_tests` module in
`src/fetch/mod.rs` (6 tests: routing of `file:`, `about:` and an unknown scheme, and `err_to_string`).
`tests/fetch_https.rs`'s `trusted_https_fetch_renders_the_body` goes through `fetch::fetch` with an
`https:` URL. Nothing tests `transport.rs` on its own; the HTTP and HTTPS tests of F04 drive it.

## Changes

none: imported

## Gate

- **F01-AC-1** — `http://example.com:8080/a/b?x=1` parses to scheme `http`, host `example.com`, port 8080, path `/a/b` and query `x=1`; with no port, `port(80)` is 80 and a URL with no `?` has no query. *Verify:* `cargo test --locked --jobs 2 --test fetch_url -- --test-threads=2 parses_scheme_host_port_path_query default_port_is_used_when_absent` exits 0.
- **F01-AC-2** — The scheme is lowercased (`HTTP://` reads `http`) and the host is kept as written (`Host` stays `Host`). *Verify:* `cargo test --locked --jobs 2 --test fetch_url -- --test-threads=2 scheme_is_lowercased scheme_case_normalization_does_not_touch_host` exits 0.
- **F01-AC-3** — An explicit `:80` or `:443` is kept as `Some(port)` in `parts()`, while an absent port is `None`; `port(default)` gives the same value for both. *Verify:* `cargo test --locked --jobs 2 --test fetch_url -- --test-threads=2 explicit_default_port_is_kept_distinct_from_absent_port` exits 0.
- **F01-AC-4** — `http://host` has the empty path, not `/`. *Verify:* `cargo test --locked --jobs 2 --test fetch_url -- --test-threads=2 empty_path_is_the_empty_string_not_a_slash` exits 0.
- **F01-AC-5** — Parsing `""`, `not a url at all`, `http://`, `:::` and `http:///no-host-path` does not panic. *Verify:* `cargo test --locked --jobs 2 --test fetch_url -- --test-threads=2 malformed_url_never_panics_and_degrades_gracefully` exits 0.
- **F01-AC-6** — Against the base `http://a/b/c/d;p?q`, a relative path merges with the base's directory and dot segments are removed: `g` and `./g` give `http://a/b/c/g`, `g/` gives `http://a/b/c/g/`, `../g` gives `http://a/b/g` and `../../g` gives `http://a/g`. *Verify:* `cargo test --locked --jobs 2 --test fetch_url -- --test-threads=2 resolve_relative_path_against_directory resolve_dot_dot_segments_walk_up_the_path` exits 0.
- **F01-AC-7** — Against the same base, `/g` gives `http://a/g`, `//g` gives `http://g`, `?y` gives `http://a/b/c/d;p?y`, and `http://other/x` is returned as is; against `http://127.0.0.1:9000/redirect/3`, `/redirect/2` keeps the port. *Verify:* `cargo test --locked --jobs 2 --test fetch_url -- --test-threads=2 resolve_absolute_path_reference resolve_network_path_reference resolve_query_only_reference_keeps_base_path resolve_absolute_reference_ignores_base resolve_typical_redirect_location_headers` exits 0.
- **F01-AC-8** — A reference's fragment is dropped: `g#s` gives `http://a/b/c/g`. *Verify:* `cargo test --locked --jobs 2 --test fetch_url -- --test-threads=2 resolve_fragment_is_dropped` exits 0.
- **F01-AC-9** — Against a `file:///tmp/dir/doc.html` base, `pic.png` resolves to `file:///tmp/dir/pic.png` (and likewise for a deeper directory and for `images/pic.png`), and an absolute `file:///other/pic.png` reference is returned unchanged whatever the base. *Verify:* `cargo test --locked --jobs 2 --test fetch_url -- --test-threads=2 resolve_relative_file_reference resolve_an_absolute_file_reference_passes_through_unchanged_regardless_of_base` exits 0.
- **F01-AC-10** — `fetch::fetch` sends a `file:` URL to the file fetcher (a missing file is an `Io` error), an `about:` URL to the about fetcher (status 200), and `gopher://` to `UnsupportedScheme("gopher")`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::dispatch_tests::fetch_` exits 0.
- **F01-AC-11** — `fetch::fetch` sends an `https:` URL to the HTTP client, which fetches it over TLS. *Verify:* `cargo test --locked --jobs 2 --test fetch_https -- --test-threads=2 trusted_https_fetch_renders_the_body` exits 0.
- **F01-AC-12** — `err_to_string` renders `Tls(s)` as `s` alone, `UnsupportedScheme("gopher")` as `unsupported scheme: gopher`, and `Protocol("boom")` as `Protocol("boom")`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::dispatch_tests::err_to_string` exits 0.
- **F01-AC-13** — `Response::header` matches the header name ignoring ASCII case. *Verify:* unpinned (every test asks for a name in the case the response stored it).
- **F01-AC-14** — `TcpStream`'s `shutdown_write` closes the write half, and the plain-HTTP path never calls it. *Verify:* unpinned.
- **F01-AC-15** — `userinfo@` is dropped from the host, a port that is not a `u16` is `None`, and dot-segment removal stops after 10,000 steps. *Verify:* unpinned.

## Risks

- `src/fetch/mod.rs` both defines the types every fetcher uses and names every fetcher in its scheme table, so
  a node that adds a scheme or changes a fetcher's entry point edits this file. The scheme nodes (F02 to F05)
  depend on F01 and may change it; F01's graph entry does not depend on them.
- `fetch::fetch` makes a fresh `Http1Client`, so its cookie jar lives for one call (redirect hops included)
  and nothing is kept between calls. Cookie persistence (charter C6, SPEC.md's planned work) has to change
  that.
- `Url` is a string newtype and `parts()` reparses on every accessor call. The parser is not RFC 3986: no
  percent-decoding, no IPv6 brackets (a bare `[::1]` host, with no port, splits at its last colon), no IDNA, and the host's
  case is kept, which matters to the cookie jar's matching (F03 lowercases what it compares).
- `resolve` always rebuilds `scheme://host...`, so it is meant for hierarchical schemes; resolving a relative
  reference against a `data:` or `about:` base gives a URL such as `about://g`.
- Callers outside the area (`src/images.rs`, `src/bg_images.rs`, `src/stylesheets.rs`, `src/form.rs`,
  `src/frames.rs`, `src/browser.rs`, `src/main.rs`, `src/backend/address_edit.rs`) use these types and
  `fetch::fetch`; their nodes were mapped before this one and do not list it in their deps.
