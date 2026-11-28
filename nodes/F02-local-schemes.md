# F02 — local-schemes

| Track | Deps | Unlocks | Size |
|---|---|---|---|
| done | F01 | none | S |

**Owns:** `src/fetch/file.rs`, `src/fetch/data.rs`

## Purpose

Imported from the code. The two schemes that never touch the network: `file:` reads the local filesystem and
`data:` decodes the URL itself (RFC 2397). Both answer with status 200 and a content type.

`src/fetch/file.rs`:

- `FileFetcher` implements `Fetch`. A URL whose scheme is not `file` is `UnsupportedScheme`. Otherwise it
  reads the file at `file_path(url)` and answers 200 with the bytes, `final_url` the request's URL and one
  `Content-Type` header from `content_type_for_path`. A read error is `Io("<path>: <error>")`.
- `file_path(url)` takes the path from `file:///abs/path`, from `file://host/abs/path` (the host is ignored)
  and from the no-authority `file:relative/path`; an empty path is a `Protocol` error. It does no
  percent-decoding.
- `content_type_for_path` maps the extension, ignoring case: `html` and `htm` to `text/html`, `css` to
  `text/css`, `gif`, `jpg`/`jpeg` and `png` to their image types, `txt` to `text/plain`, and anything else
  to `application/octet-stream`.

`src/fetch/data.rs`, `data::fetch(request)`:

- The `data:` prefix is matched ignoring case; a URL without it, or without a comma, is a `Protocol` error.
- The media type (everything before the comma) is percent-decoded first (lossy UTF-8), then a `;base64`
  suffix is looked for, ignoring case, on the decoded text, so `%3Bbase64` counts too. An empty media type
  becomes `text/plain;charset=US-ASCII`. The answer has one `content-type` header with that media type.
- The payload is percent-decoded; with the base64 flag it is then base64-decoded (RFC 4648 alphabet, ASCII
  whitespace skipped, `=` ends the data, padding optional, a trailing lone symbol or a byte outside the
  alphabet a `Protocol` error).
- Percent-decoding turns `%XY` with two hex digits into a byte and passes any other `%` through.

## Tests

`tests/fetch_file.rs` (8 tests: reading a fixture, a missing file, the `.txt` type, a non-`file` scheme,
and `file_path` on absolute and resolved URLs). The `tests` module in `src/fetch/data.rs` (22 tests:
base64, percent-decoding, and `fetch` end to end, including percent-escaped payloads and media types and
the totality cases). `fetch::dispatch_tests` (F01) also reaches `FileFetcher` through `fetch::fetch`.

## Changes

none: imported

## Gate

- **F02-AC-1** — Fetching `fixtures/basic.html` by `file://` answers 200 with the file's bytes, a `Content-Type` of `text/html`, and the request's URL as `final_url`. *Verify:* `cargo test --locked --jobs 2 --test fetch_file -- --test-threads=2 reads_existing_html_fixture_with_html_content_type` exits 0.
- **F02-AC-2** — A `.txt` file is served as `text/plain`. *Verify:* `cargo test --locked --jobs 2 --test fetch_file -- --test-threads=2 content_type_by_extension_txt` exits 0.
- **F02-AC-3** — A file that does not exist is an error, not a panic. *Verify:* `cargo test --locked --jobs 2 --test fetch_file -- --test-threads=2 missing_file_is_a_fetch_error_not_a_panic` exits 0.
- **F02-AC-4** — `FileFetcher` refuses an `http:` URL with `UnsupportedScheme`. *Verify:* `cargo test --locked --jobs 2 --test fetch_file -- --test-threads=2 non_file_scheme_is_unsupported` exits 0.
- **F02-AC-5** — `file_path` gives `/tmp/dir/doc.html` for `file:///tmp/dir/doc.html`, and the filesystem path of a relative or absolute reference resolved against such a base, at any directory depth. *Verify:* `cargo test --locked --jobs 2 --test fetch_file -- --test-threads=2 file_path_` exits 0.
- **F02-AC-6** — `data:text/plain,Hello` answers 200 with the body `Hello` and the content type `text/plain`; `data:;base64,SGk=` has the body `Hi` and the content type `text/plain;charset=US-ASCII`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::data::tests::fetch_plain_text_data_url fetch::data::tests::fetch_default_media_type_with_base64` exits 0.
- **F02-AC-7** — Base64 decodes padded (`SGk=`, `TQ==`) and unpadded (`SGk`, `TQ`) input alike, skips ASCII whitespace, and decodes the empty string to nothing. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::data::tests::decode_base64_with_padding fetch::data::tests::decode_base64_without_padding fetch::data::tests::decode_base64_skips_whitespace fetch::data::tests::decode_base64_empty_is_empty` exits 0.
- **F02-AC-8** — A byte outside the base64 alphabet and a trailing lone symbol are errors; a payload with a bad percent escape (`AAAA%ZZAAAA`) is a `Protocol` error, not a panic. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::data::tests::decode_base64_invalid_alphabet_is_err fetch::data::tests::decode_base64_truncated_single_symbol_is_err fetch::data::tests::fetch_base64_payload_with_malformed_percent_escape_does_not_panic` exits 0.
- **F02-AC-9** — A base64 `image/png` payload round-trips raw bytes, `0x00` and `0xFF` included, with the content type `image/png`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::data::tests::fetch_base64_image_round_trips_raw_bytes` exits 0.
- **F02-AC-10** — A base64 payload is percent-decoded before it is base64-decoded: `%2B`, `%2F` and `%3D` decode as `+`, `/` and `=`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::data::tests::fetch_base64_payload_with_percent_escaped` exits 0.
- **F02-AC-11** — Percent-decoding turns `%20` into a space, passes a bad escape (`%2G`) through literally, and leaves text without `%` unchanged. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::data::tests::percent_decode_` exits 0.
- **F02-AC-12** — The media type is percent-decoded before the base64 flag is looked for: `text%2Fplain` reads `text/plain`, `text/plain%3Bcharset=utf-8` reads `text/plain;charset=utf-8`, and `text/plain%3Bbase64` is base64 with the media type `text/plain`. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::data::tests::fetch_percent_escaped_` exits 0.
- **F02-AC-13** — `data:nocomma` and `http://example.com` are refused by `data::fetch` with a `Protocol` error. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::data::tests::fetch_malformed_no_comma_is_protocol_error fetch::data::tests::fetch_non_data_scheme_is_protocol_error` exits 0.
- **F02-AC-14** — A hostile media type does not panic: `text%ZZplain` is kept literally, and `ñabcdef` (where the `;base64` length falls inside a multi-byte character) reads as no base64 flag. *Verify:* `cargo test --locked --jobs 2 --lib -- --test-threads=2 fetch::data::tests::fetch_malformed_percent_escape_in_media_type_does_not_panic fetch::data::tests::fetch_non_ascii_media_type_does_not_panic_on_char_boundary` exits 0.
- **F02-AC-15** — The `file://host/path` and `file:relative` forms, the other extensions' content types, an uppercase `DATA:` prefix and an uppercase `;BASE64` flag. *Verify:* unpinned.

## Risks

- `file_path` does no percent-decoding, so a `file:` URL with `%20` names a file whose name holds `%20`. The
  `file://host/` host is ignored, so such a URL reads the local path.
- `FileFetcher` reads any path the process can read; any limit on which documents may name a `file:` URL
  is the callers' to apply, not this module's.
- The content type is a guess by extension only, so a file with a wrong or missing extension gets a wrong
  type or `application/octet-stream`.
- `data:` bodies have no size bound beyond the URL's own length.
- `percent_decode` decodes only when two characters follow the `%`, so `%4` at the very end passes through.
