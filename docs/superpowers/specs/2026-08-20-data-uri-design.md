# `data:` URI scheme (Acid2 Packet 4) — design

**Date:** 2026-08-20 · **Status:** approved design · **Program:** Acid2 roadmap Packet 4 of 7.

## Goal
Serve `data:[<mediatype>][;base64],<data>` URLs through the existing `fetch::fetch` scheme table (no socket),
decoding base64/percent bytes + content-type so they feed the existing image decoders (`img::decode_bytes`)
and any other consumer. Acid2 uses `url(data:…)` ×4. **Self-contained**, no new dependency (a small in-repo
base64 + percent decoder — the floppy budget forbids a crate).

## Non-negotiables
- **No new dependency** (write the base64/percent decoders; ~60 lines). Report the i486 delta.
- **Parsing is TOTAL:** a malformed `data:` URL returns a `FetchError` (never panics); bad base64/percent →
  error, not a panic. Golden-safe: a document with no `data:` URL is unaffected (new scheme arm only).
- **Test-first; no local i486 builds** (CI compiles/tests); PNG golden pixel-verified (controller, AGENTS §4).
- **No JavaScript / C3:** a data URL is inert bytes; nothing executes.

## Current state (ground-truthed)
- `fetch::fetch(request) -> Result<Response, FetchError>` is a `match request.url.scheme()` table
  (`src/fetch/mod.rs:107`): `"file"`, `"http"|"https"`. A new scheme is ONE arm.
- `Response { status: u16, final_url: Url, headers: Vec<(String,String)>, body: Vec<u8> }`; consumers read the
  content type via `response.header("content-type")` (see `images.rs:220` → `img::decode_bytes(&body, ct)`).
  `file.rs:29` is a `Response { .. }` constructor to mirror.
- `Url::as_str()` returns the FULL original string (`file.rs`/`parse` prove it); `data:` opaque content
  (mediatype, `;base64`, commas, base64 payload) survives intact — parse the data portion from `as_str()`,
  not from `url.path()` (the generic path-splitter isn't used). `Url::resolve` keeps an absolute `data:`
  reference as-is, so `<img src="data:…">` reaches `fetch` unmangled.
- No base64/percent decoder exists in-repo (grep-confirmed).

## Design
### `src/fetch/data.rs` (new module)
`pub fn fetch(request: &Request) -> Result<Response, FetchError>`:
1. `let s = request.url.as_str();` strip the leading `data:` (case-insensitive on the scheme) → `rest`.
2. `let (meta, payload) = rest.split_once(',')` → malformed (no comma) ⇒ `FetchError::Io("malformed data: URL")`
   (or the closest existing variant — reuse, don't add a variant unless needed).
3. Parse `meta`: it is `[<mediatype>][;base64]`. If it ends (case-insensitive) with `;base64`, set a base64
   flag and strip that suffix. The remaining `meta` is the media type; if empty, default to
   `text/plain;charset=US-ASCII` (RFC 2397).
4. Decode `payload`: base64 (ignore ASCII whitespace) if the flag is set, else percent-decode (`%XX` → byte,
   other bytes literal). Decode failure ⇒ `FetchError`.
5. `Ok(Response { status: 200, final_url: request.url.clone(), headers: vec![("content-type".into(),
   media_type)], body: decoded })`.
- **base64 decoder** (private fn): standard alphabet `A–Za–z0–9+/`, `=` padding, whitespace-skipping, total
  (invalid char ⇒ `Err`). ~40 lines, no dep.
- **percent decoder** (private fn): `%` + two hex ⇒ byte; `+` stays `+` (data: is not form-encoding); other
  bytes literal. ~15 lines.

### Wire it (`src/fetch/mod.rs`)
Add `mod data;` and the arm `"data" => data::fetch(request),` to the scheme `match`.

### Testing / fixtures
- **Unit (CI):** base64 decode (with/without padding, whitespace, invalid ⇒ Err); percent decode; end-to-end
  `data::fetch` on `data:text/plain,Hello` (body `Hello`, ct `text/plain`), `data:;base64,SGk=` (body `Hi`),
  `data:image/png;base64,<png>` (body == the PNG bytes, ct `image/png`), and a malformed `data:nocomma` ⇒ Err.
- **Golden micro-fixture** `fixtures/data-img.html`: `<img src="data:image/png;base64,…">` of a tiny known
  PNG (e.g. a 16×16 solid-color square) → PNG golden showing the decoded image. Controller pixel-verifies the
  square's color/size before blessing.

### Charter / decisions
- Charter C2 "What Stele Speaks": add the `data:` URI scheme (Acid2 Packet 4).
- `DECISIONS.md`: entry — `data:` as one `fetch::fetch` arm + in-repo base64/percent decoders (no dep);
  totality; `data:` parsed from `Url::as_str()` (opaque, not path-split).

## Out of scope (YAGNI)
- `data:` in non-image contexts beyond what the decoders already feed (CSS `url(data:)` backgrounds work for
  free IF the bg-image pipeline routes through `fetch`; verify, don't build a second path).
- Charset transcoding of `text/*` data (bytes handed through as-is).
- `mediatype` parameter parsing beyond `;base64` detection + charset default.
