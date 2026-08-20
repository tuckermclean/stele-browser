# `data:` URI scheme (Acid2 Packet 4) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Serve `data:[<mediatype>][;base64],<data>` through `fetch::fetch`, decoding base64/percent + content
type so it feeds the existing image decoders.

**Architecture:** New `src/fetch/data.rs` (parse + in-repo base64 + percent decoders + a `fetch` fn), wired as
one arm in `fetch::fetch`'s scheme match. No new dependency.

**Spec:** `docs/superpowers/specs/2026-08-20-data-uri-design.md`

## Global Constraints
- No new dependency (write base64/percent). 1.44 MB floppy — report i486 delta. Parsing TOTAL (malformed ⇒
  `FetchError::Protocol(...)`, never panic). Golden-safe (new scheme arm only; no `data:` URL ⇒ unchanged).
  No local i486 builds; PNG golden pixel-verified (controller). No JS / C3.

---

### Task 1: `src/fetch/data.rs` + wire the scheme table + unit tests

**Files:** Create `src/fetch/data.rs`; Modify `src/fetch/mod.rs` (`mod data;` + `"data" =>` arm).

- [ ] **Step 1: Write failing tests** (in `data.rs` `#[cfg(test)]`): see Step 3's `fetch` for the API. Assert:
  `decode_base64(b"SGk=") == Ok(b"Hi")`; padding-less `SGk` also ok; whitespace `"SG k=\n"` ok; invalid `"@@"`
  ⇒ Err. `percent_decode(b"a%20b") == b"a b"`; `%2G` (bad hex) ⇒ literal or Err (pick: literal passthrough of a
  malformed `%` is fine — match the code). `fetch` on a `Request::get(Url::new("data:text/plain,Hello"))` ⇒
  body `b"Hello"`, `header("content-type") == "text/plain"`; `data:;base64,SGk=` ⇒ body `b"Hi"`, ct
  `text/plain;charset=US-ASCII`; `data:image/png;base64,<B64>` ⇒ body == the raw PNG bytes; `data:nocomma` ⇒
  `Err(FetchError::Protocol(_))`.
- [ ] **Step 2: Verify fail** — CI: `cargo test --lib fetch::data` → FAIL (module missing).
- [ ] **Step 3: Implement `src/fetch/data.rs`:**
```rust
//! `data:` URI scheme (Acid2 Packet 4): `data:[<mediatype>][;base64],<data>`
//! decoded to bytes + content-type, no socket. RFC 2397.
use super::{FetchError, Request, Response};

pub fn fetch(request: &Request) -> Result<Response, FetchError> {
    let s = request.url.as_str();
    // Strip the `data:` scheme (case-insensitive), keeping the opaque remainder.
    let rest = s
        .get(..5)
        .filter(|p| p.eq_ignore_ascii_case("data:"))
        .map(|_| &s[5..])
        .ok_or_else(|| FetchError::Protocol(format!("not a data: URL: {s}")))?;
    let (meta, payload) = rest
        .split_once(',')
        .ok_or_else(|| FetchError::Protocol("malformed data: URL (no comma)".to_string()))?;

    let (media_type, is_base64) = match meta.strip_suffix_ci(";base64") {
        Some(mt) => (mt, true),
        None => (meta, false),
    };
    let media_type = if media_type.is_empty() { "text/plain;charset=US-ASCII" } else { media_type }.to_string();

    let body = if is_base64 {
        decode_base64(payload.as_bytes())?
    } else {
        percent_decode(payload.as_bytes())
    };

    Ok(Response {
        status: 200,
        final_url: request.url.clone(),
        headers: vec![("content-type".to_string(), media_type)],
        body,
    })
}

// `str::strip_suffix` is case-sensitive; `;BASE64` is valid. Small helper:
trait StripSuffixCi { fn strip_suffix_ci(&self, suffix: &str) -> Option<&str>; }
impl StripSuffixCi for str {
    fn strip_suffix_ci(&self, suffix: &str) -> Option<&str> {
        let n = self.len().checked_sub(suffix.len())?;
        if self[n..].eq_ignore_ascii_case(suffix) { Some(&self[..n]) } else { None }
    }
}

fn b64_val(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn decode_base64(input: &[u8]) -> Result<Vec<u8>, FetchError> {
    // Ignore ASCII whitespace; stop at padding. Total: invalid char => Err.
    let mut quad = [0u8; 4];
    let mut qn = 0usize;
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut seen_pad = false;
    for &c in input {
        if c.is_ascii_whitespace() { continue; }
        if c == b'=' { seen_pad = true; qn += 1; if qn == 4 { flush_quad(&quad, qn_pads(&quad, qn), &mut out); qn = 0; } continue; }
        if seen_pad { return Err(FetchError::Protocol("data: base64 char after padding".into())); }
        let v = b64_val(c).ok_or_else(|| FetchError::Protocol("data: invalid base64".into()))?;
        quad[qn] = v; qn += 1;
        if qn == 4 { out.push((quad[0] << 2) | (quad[1] >> 4)); out.push((quad[1] << 4) | (quad[2] >> 2)); out.push((quad[2] << 6) | quad[3]); qn = 0; }
    }
    // Trailing group without explicit '=': 2 chars => 1 byte, 3 chars => 2 bytes.
    match qn {
        0 => {}
        2 => out.push((quad[0] << 2) | (quad[1] >> 4)),
        3 => { out.push((quad[0] << 2) | (quad[1] >> 4)); out.push((quad[1] << 4) | (quad[2] >> 2)); }
        _ => return Err(FetchError::Protocol("data: truncated base64".into())),
    }
    Ok(out)
}
```
NOTE to implementer: the `=`-padding path above is sketched loosely (the `flush_quad`/`qn_pads` helpers are
NOT real) — REPLACE the padding handling with a correct, simple implementation: accumulate up to 4 non-whitespace
symbols where `=` marks end-of-data; a group `XX==` → 1 byte, `XXX=` → 2 bytes, `XXXX` → 3 bytes; treat `=` as
terminator (compute output length from the count of real symbols before padding). Keep it total (no panic;
invalid alphabet ⇒ `Err`). Prefer clarity over cleverness. Add tests for `"SGVsbG8="`→`Hello`? (that's not
valid — use real vectors: `"SGk="`→`Hi`, `"SGVsbG8="`→? compute a correct one, e.g. `"TWFu"`→`Man`,
`"TWE="`→`Ma`, `"TQ=="`→`M`).
```rust
fn percent_decode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' && i + 2 < input.len() + 1 && i + 2 <= input.len().saturating_sub(0) {
            // guard bounds properly:
        }
        if input[i] == b'%' && i + 2 < input.len() {
            let hi = (input[i + 1] as char).to_digit(16);
            let lo = (input[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) { out.push((h * 16 + l) as u8); i += 3; continue; }
        }
        out.push(input[i]); i += 1;
    }
    out
}
```
NOTE: clean up `percent_decode`'s bounds check to a single correct `if input[i] == b'%' && i + 2 < input.len()`
form (drop the bogus first `if`); `%` without two following hex digits passes through literally.
- [ ] **Step 4: Wire** `src/fetch/mod.rs`: add `mod data;` near the other `mod` decls and
  `"data" => data::fetch(request),` in the `fetch` scheme `match` (before the `UnsupportedScheme` fallback).
- [ ] **Step 5: Verify pass** — CI: `cargo test --lib fetch` green; `cargo test --lib` green.
- [ ] **Step 6: Commit** — `feat(fetch): data: URI scheme (base64/percent decode, no deps) (Acid2 P4)`

---

### Task 2: `data:` image fixture + accept.sh + controller bless
**Files:** Create `fixtures/data-img.html`; Modify `accept.sh` (A5n block); Bless `goldens/data-img.png` (controller).
- [ ] **Step 1:** Generate a tiny known PNG as base64 (implementer: `python3 -c` with PIL — a 16×16 solid
  red square → base64 string), and write `fixtures/data-img.html` (single line):
  `<html><body><img src="data:image/png;base64,<B64>" width="16" height="16"></body></html>`.
  Report the exact base64 used in the task report.
- [ ] **Step 2:** Wire `accept.sh` A5n (mirror the A5k `gc-before-string` block exactly): var stem `DATA_IMG`,
  fixture `fixtures/data-img.html`, golden `goldens/data-img.png`, tmp `/tmp/stele_a5n.png`. No golden created.
- [ ] **Step 3:** Push; CI renders. (Implementer stops; blessing is controller.)
- [ ] **Step 4 (CONTROLLER):** pixel-verify `data-img.png` shows the 16×16 red square (decoded from the data
  URI), then bless. `bash -n accept.sh` clean. Commit: `test(fetch): data: image fixture + accept.sh (Acid2 P4)`.

---

### Task 3: charter + DECISIONS + JOURNAL
- [ ] Charter C2: add `data:` URI scheme (Acid2 Packet 4). DECISIONS: `data:` as one fetch arm + in-repo
  base64/percent (no dep), parsed from `Url::as_str()` (opaque), total. JOURNAL: P4 landed, fixture, i486 size.
  Commit: `docs(fetch): charter + DECISIONS + JOURNAL for data: URIs (Acid2 P4)`.

## Self-Review
Spec coverage: data.rs+wire → T1; fixture/tests → T1 (unit) + T2 (golden); charter/decisions → T3. base64
padding sketch flagged for the implementer to implement correctly. No new dep. ✓
