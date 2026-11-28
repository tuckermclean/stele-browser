# F03 — cookie-jar

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | F01 | F04 | S |

**Owns:** `src/fetch/cookies.rs`

## Purpose

Imported from the code. The cookie jar of charter C6: Set-Cookie parsing, domain and path matching
(RFC 6265, adapted), and the Netscape text format the user owns. The HTTP client (F04) holds one jar and
consults and updates it at every hop.

- `Cookie` holds the domain, path, name, value and the `secure` flag. There is no expiry field:
  `Expires` and `Max-Age` are parsed and ignored, so every cookie is a session cookie (DECISIONS.md). A
  domain with a leading `.` came from an explicit `Domain=` and matches subdomains; a domain without one is
  host-only and matches that host exactly.
- `CookieJar::set_from_header(url, value)` parses one `Set-Cookie` value against the response's URL. The
  first `name=value` pair is the cookie (trimmed); no `=` or an empty name drops the whole header. `Domain=`
  (leading dots stripped, lowercased) is ignored when empty, rejects the whole cookie when it has no dot (a
  single-label supercookie guard, not a public-suffix list, DECISIONS.md D12), and rejects the whole cookie
  when the responding host does not domain-match it. `Path=` counts only when it starts with `/`; otherwise
  the default path is the request path's directory. `Secure` sets the flag; `HttpOnly`, `SameSite` and any
  other attribute are ignored. A cookie with the same domain (ignoring case), path and name replaces the
  stored one.
- `header_for(url)` builds the `Cookie:` value: the cookies whose domain matches the URL's host, whose path
  path-matches the URL's path at a `/` boundary, and which are not `secure` unless the URL is `https`,
  longest path first, joined by `; `; `None` when nothing matches. When the cookie's domain or the host is
  an IP literal (it contains `:`, or is a dotted quad), domain matching is exact.
- `to_netscape()` writes `# Netscape HTTP Cookie File` and one line of seven tab-separated fields per
  cookie: domain, the include-subdomains flag (`TRUE` for a leading `.`), path, secure, expiry (always 0),
  name and value. `from_netscape(text)` reads that format back, skipping blank and `#` lines, lines of fewer
  than seven fields and lines with an empty name.

## Tests

The `tests` module in `src/fetch/cookies.rs` (17 tests: parsing, domain and path matching, the `Domain=`
guards, IP literals, joining, and the Netscape format). `tests/fetch_https.rs`'s
`secure_cookie_set_over_https_is_not_sent_over_http`. F04's `cookies_round_trip_through_a_real_hop` uses
the jar through the HTTP client.

## Changes

none: imported

## Gate

- **F03-AC-1** — `sid=abc123` set from `http://example.com/a/b/c` is sent for `http://example.com/a/b/x` (the default path is `/a/b`) and not for `http://other.com/a/b/c` (host-only). *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::cookies::tests::set_from_header_basic_name_value_is_host_only_default_path` exits 0.
- **F03-AC-2** — A cookie with `Domain=example.com` set from `www.example.com` is sent to `example.com` and `api.example.com`, and not to `notexample.com` or `evilexample.com`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::cookies::tests::set_from_header_domain_attribute_enables_subdomain_matching` exits 0.
- **F03-AC-3** — `Path=/admin` is sent for `/admin` and `/admin/x` and not for `/other`; `Path=/foo` is sent for `/foo` and `/foo/bar` and not for `/foobar`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::cookies::tests::set_from_header_path_attribute_is_respected fetch::cookies::tests::header_for_path_boundary_is_a_segment_not_a_prefix` exits 0.
- **F03-AC-4** — A `Secure` cookie is sent to the `https:` URL and withheld from the `http:` URL of the same host. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::cookies::tests::set_from_header_secure_flag_is_parsed` exits 0, and `cargo test --locked --jobs 2 --test fetch_https -- --test-threads=2 secure_cookie_set_over_https_is_not_sent_over_http` exits 0.
- **F03-AC-5** — `HttpOnly`, `Expires`, `Max-Age` and `SameSite` do not stop the cookie from being stored. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::cookies::tests::set_from_header_ignores_unknown_attributes_without_breaking_parse` exits 0.
- **F03-AC-6** — The headers ``, `notanamevalue`, `=novalue` and `;;;` store nothing and do not panic. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::cookies::tests::set_from_header_malformed_is_silently_ignored_not_panicking` exits 0.
- **F03-AC-7** — A second cookie with the same domain, path and name replaces the first. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::cookies::tests::set_from_header_replaces_same_domain_path_name` exits 0.
- **F03-AC-8** — `Domain=example.com` from `attacker.test` is rejected whole (sent to neither host), and `Domain=com` from `shop.com` is rejected; `Domain=example.com` from `www.example.com` is accepted, and a cookie with no `Domain=` is unaffected. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::cookies::tests::set_from_header_domain_attribute_ fetch::cookies::tests::set_from_header_single_label_domain_is_rejected_as_a_supercookie fetch::cookies::tests::set_from_header_host_only_cookie_unaffected_by_domain_validation` exits 0.
- **F03-AC-9** — A cookie with `Domain=127.0.0.1` is sent to `127.0.0.1` and not to `foo.127.0.0.1`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::cookies::tests::domain_match_for_ip_literal_host_is_exact_not_suffix` exits 0.
- **F03-AC-10** — Several matching cookies are joined by `; `, and an empty jar gives `None`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::cookies::tests::header_for_joins_multiple_matching_cookies fetch::cookies::tests::header_for_no_matches_is_none` exits 0.
- **F03-AC-11** — `to_netscape` writes seven tab-separated fields per cookie, with the include-subdomains flag `FALSE` for a host-only cookie and `TRUE` for a `Domain=` cookie, and the secure field `TRUE` only for a `Secure` one. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::cookies::tests::to_netscape_format_has_tab_separated_fields_and_correct_flags` exits 0.
- **F03-AC-12** — A jar written by `to_netscape` and read back by `from_netscape` sends the same `Cookie:` values for `http://example.com/` and `https://api.example.com/sub/x`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::cookies::tests::to_netscape_from_netscape_round_trip_preserves_matching_behavior` exits 0.
- **F03-AC-13** — Longer paths are sent first; the expiry field is always 0; `from_netscape` reads the secure field back and skips short lines and comments. *Verify:* unpinned.

## Risks

- Nothing in `src/` calls `to_netscape` or `from_netscape`: the jar is not saved or loaded, and `fetch::fetch`
  (F01) makes a fresh jar per call. Cookie-jar file persistence is SPEC.md's planned work; it would change
  F01 and a caller, not only this file.
- With no expiry, a cookie a server meant to delete (an `Expires` in the past or `Max-Age=0`) is not removed: the
  deleting header replaces it with its own, often empty, value, which is still sent.
- The supercookie guard is a heuristic: a two-label public suffix such as `co.uk` passes it.
- `from_netscape` keeps the domain as written, so the leading `.` is what carries subdomain matching; a jar
  file edited by hand without it becomes host-only. The include-subdomains column is not read.
- The host from `Url::host` keeps its case (F01); matching lowercases or ignores case on both sides.
