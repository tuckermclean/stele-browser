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
    let media_type = if media_type.is_empty() {
        "text/plain;charset=US-ASCII"
    } else {
        media_type
    }
    .to_string();

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
trait StripSuffixCi {
    fn strip_suffix_ci(&self, suffix: &str) -> Option<&str>;
}
impl StripSuffixCi for str {
    fn strip_suffix_ci(&self, suffix: &str) -> Option<&str> {
        let n = self.len().checked_sub(suffix.len())?;
        if self[n..].eq_ignore_ascii_case(suffix) {
            Some(&self[..n])
        } else {
            None
        }
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

/// Decode standard base64 (RFC 4648 alphabet). Total: ASCII whitespace is
/// skipped anywhere; `=` marks end-of-data (trailing padding is optional —
/// we accept both `SGk=` and unpadded `SGk`); any other non-alphabet byte is
/// an error. A trailing group of 1 symbol (with no padding) is invalid,
/// since base64 cannot encode a partial byte from a single 6-bit symbol.
fn decode_base64(input: &[u8]) -> Result<Vec<u8>, FetchError> {
    // Accumulate up to 4 non-whitespace alphabet symbols per group. `=`
    // terminates the input: everything after it (other than whitespace) is
    // ignored/validated-away by simply stopping the scan.
    let mut group = [0u8; 4];
    let mut n = 0usize; // number of real (non-padding) symbols in `group`
    let mut out = Vec::with_capacity(input.len() / 4 * 3 + 3);

    for &c in input {
        if c.is_ascii_whitespace() {
            continue;
        }
        if c == b'=' {
            // End of data. Flush whatever partial group we have (2 or 3
            // symbols are valid; 0 means nothing pending; 1 is invalid).
            break;
        }
        let v = b64_val(c).ok_or_else(|| FetchError::Protocol("data: invalid base64".into()))?;
        group[n] = v;
        n += 1;
        if n == 4 {
            out.push((group[0] << 2) | (group[1] >> 4));
            out.push((group[1] << 4) | (group[2] >> 2));
            out.push((group[2] << 6) | group[3]);
            n = 0;
        }
    }

    // Trailing group without (or with) explicit padding: 2 symbols => 1
    // byte, 3 symbols => 2 bytes. 0 symbols => nothing left. 1 symbol is
    // truncated/invalid (a single 6-bit symbol can't yield a full byte).
    match n {
        0 => {}
        2 => out.push((group[0] << 2) | (group[1] >> 4)),
        3 => {
            out.push((group[0] << 2) | (group[1] >> 4));
            out.push((group[1] << 4) | (group[2] >> 2));
        }
        _ => return Err(FetchError::Protocol("data: truncated base64".into())),
    }
    Ok(out)
}

fn percent_decode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' && i + 2 < input.len() {
            let hi = (input[i + 1] as char).to_digit(16);
            let lo = (input[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(input[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::{Method, Url};

    // --- decode_base64 -----------------------------------------------

    #[test]
    fn decode_base64_with_padding() {
        assert_eq!(decode_base64(b"SGk=").unwrap(), b"Hi");
        assert_eq!(decode_base64(b"TWFu").unwrap(), b"Man");
        assert_eq!(decode_base64(b"TWE=").unwrap(), b"Ma");
        assert_eq!(decode_base64(b"TQ==").unwrap(), b"M");
    }

    #[test]
    fn decode_base64_without_padding() {
        // Unpadded equivalents of the padded vectors above.
        assert_eq!(decode_base64(b"SGk").unwrap(), b"Hi");
        assert_eq!(decode_base64(b"TWE").unwrap(), b"Ma");
        assert_eq!(decode_base64(b"TQ").unwrap(), b"M");
    }

    #[test]
    fn decode_base64_skips_whitespace() {
        assert_eq!(decode_base64(b"SG k=\n").unwrap(), b"Hi");
        assert_eq!(decode_base64(b" TWFu \n").unwrap(), b"Man");
    }

    #[test]
    fn decode_base64_invalid_alphabet_is_err() {
        assert!(decode_base64(b"@@").is_err());
    }

    #[test]
    fn decode_base64_empty_is_empty() {
        assert_eq!(decode_base64(b"").unwrap(), b"");
    }

    #[test]
    fn decode_base64_truncated_single_symbol_is_err() {
        assert!(decode_base64(b"S").is_err());
    }

    // --- percent_decode -----------------------------------------------

    #[test]
    fn percent_decode_basic() {
        assert_eq!(percent_decode(b"a%20b"), b"a b");
    }

    #[test]
    fn percent_decode_bad_hex_is_literal_passthrough() {
        assert_eq!(percent_decode(b"a%2Gb"), b"a%2Gb");
    }

    #[test]
    fn percent_decode_no_percent_is_identity() {
        assert_eq!(percent_decode(b"Hello, World!"), b"Hello, World!");
    }

    // --- fetch (end-to-end) --------------------------------------------

    fn get(url: &str) -> Request {
        Request {
            method: Method::Get,
            url: Url::new(url),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    #[test]
    fn fetch_plain_text_data_url() {
        let resp = fetch(&get("data:text/plain,Hello")).unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"Hello");
        assert_eq!(resp.header("content-type"), Some("text/plain"));
    }

    #[test]
    fn fetch_default_media_type_with_base64() {
        let resp = fetch(&get("data:;base64,SGk=")).unwrap();
        assert_eq!(resp.body, b"Hi");
        assert_eq!(
            resp.header("content-type"),
            Some("text/plain;charset=US-ASCII")
        );
    }

    #[test]
    fn fetch_base64_image_round_trips_raw_bytes() {
        // A tiny "PNG-like" byte sequence (not a real PNG, just binary data
        // with a null byte) — proves base64 round-trips raw bytes, not just
        // ASCII text.
        let raw: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0xff];
        let b64 = encode_base64_for_test(raw);
        let url = format!("data:image/png;base64,{b64}");
        let resp = fetch(&get(&url)).unwrap();
        assert_eq!(resp.body, raw);
        assert_eq!(resp.header("content-type"), Some("image/png"));
    }

    #[test]
    fn fetch_malformed_no_comma_is_protocol_error() {
        let err = fetch(&get("data:nocomma")).unwrap_err();
        assert!(matches!(err, FetchError::Protocol(_)), "got {err:?}");
    }

    #[test]
    fn fetch_non_data_scheme_is_protocol_error() {
        let err = fetch(&get("http://example.com")).unwrap_err();
        assert!(matches!(err, FetchError::Protocol(_)), "got {err:?}");
    }

    // Minimal base64 encoder used only by tests, to avoid hand-typing base64
    // literals for binary test vectors.
    fn encode_base64_for_test(data: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let b0 = chunk[0];
            let b1 = *chunk.get(1).unwrap_or(&0);
            let b2 = *chunk.get(2).unwrap_or(&0);
            let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
            out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
            out.push(if chunk.len() > 1 {
                ALPHABET[((n >> 6) & 0x3f) as usize] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                ALPHABET[(n & 0x3f) as usize] as char
            } else {
                '='
            });
        }
        out
    }
}
