//! Bespoke minimal URL parsing and relative-reference resolution over the
//! frozen `Url` newtype (brief §4: "URL parsing: bespoke minimal ... your
//! call, record it" — DECISIONS.md records the bespoke-vs-crate call for the
//! HTTP layer; this module is the same spirit applied to URLs: enough syntax
//! to drive `http1`/`file` and to resolve `Location` headers, not a full
//! RFC 3986 implementation).
//!
//! Syntax handled: `scheme:[//[userinfo@]host[:port]]path[?query][#fragment]`.
//! Never panics: malformed input degrades to best-effort/empty components
//! rather than erroring (brief §9 — parsing is TOTAL).

use super::Url;

/// The pieces of a parsed URL, all owned (parsing is cheap and infrequent
/// relative to network IO, so we favor simplicity over borrowing).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UrlParts {
    pub scheme: String,
    pub host: String,
    pub port: Option<u16>,
    /// Always empty or starting with `/`.
    pub path: String,
    pub query: Option<String>,
}

impl Url {
    /// Parse this URL into its component parts. Total: never panics.
    pub fn parts(&self) -> UrlParts {
        parse(self.as_str())
    }

    /// The scheme, lowercased (e.g. `"http"`, `"file"`). Empty if unparseable.
    pub fn scheme(&self) -> String {
        self.parts().scheme
    }

    /// The host, as written (no punycode/normalization). Empty if absent.
    pub fn host(&self) -> String {
        self.parts().host
    }

    /// The port, or `default` if none was given.
    pub fn port(&self, default: u16) -> u16 {
        self.parts().port.unwrap_or(default)
    }

    /// The path, always starting with `/` unless the URL genuinely has none
    /// (in which case this is empty and callers should treat it as `/`).
    pub fn path(&self) -> String {
        self.parts().path
    }

    /// The query string, without the leading `?`.
    pub fn query(&self) -> Option<String> {
        self.parts().query
    }

    /// Resolve `reference` (as found in e.g. a `Location:` header or an
    /// `href`) against `self` as the base URL, per RFC 3986 §5 (simplified:
    /// no fragment is retained, since `Url`/`Request` never need one).
    pub fn resolve(&self, reference: &str) -> Url {
        Url::new(resolve_reference(self, reference))
    }
}

// -- parsing -----------------------------------------------------------

/// `scheme` must start with a letter and contain only letters, digits, `+`,
/// `-`, `.` (RFC 3986 §3.1) — enough to reject "not a url at all" (no colon
/// mishaps aside) as schemeless rather than misparsing `not` as a scheme.
fn split_scheme(s: &str) -> Option<(&str, &str)> {
    let idx = s.find(':')?;
    let (scheme, rest) = s.split_at(idx);
    let rest = rest.get(1..)?;
    if scheme.is_empty() {
        return None;
    }
    let mut chars = scheme.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return None;
    }
    Some((scheme, rest))
}

fn parse(raw: &str) -> UrlParts {
    let mut parts = UrlParts::default();

    let (scheme, mut rest) = match split_scheme(raw) {
        Some(x) => x,
        None => {
            // No recognizable scheme: total fallback — treat the whole thing
            // as an opaque path, all other components empty.
            parts.path = raw.to_string();
            return parts;
        }
    };
    parts.scheme = scheme.to_ascii_lowercase();

    if let Some(after_slashes) = rest.strip_prefix("//") {
        let authority_end = after_slashes
            .find(['/', '?', '#'])
            .unwrap_or(after_slashes.len());
        let authority = &after_slashes[..authority_end];
        rest = &after_slashes[authority_end..];

        let host_port = match authority.rfind('@') {
            Some(i) => authority.get(i + 1..).unwrap_or(""),
            None => authority,
        };
        match host_port.rfind(':') {
            Some(colon) => {
                let host = &host_port[..colon];
                let port_str = &host_port[colon + 1..];
                parts.host = host.to_string();
                parts.port = port_str.parse::<u16>().ok();
            }
            None => {
                parts.host = host_port.to_string();
            }
        }
    }

    // `rest` is now path[?query][#fragment].
    let frag_start = rest.find('#').unwrap_or(rest.len());
    let rest_no_frag = &rest[..frag_start];
    let q_start = rest_no_frag.find('?').unwrap_or(rest_no_frag.len());
    let path = &rest_no_frag[..q_start];
    parts.path = path.to_string();
    if q_start < rest_no_frag.len() {
        // skip the '?'
        parts.query = rest_no_frag.get(q_start + 1..).map(|s| s.to_string());
    }

    parts
}

// -- relative-reference resolution (RFC 3986 §5, simplified) -----------

fn resolve_reference(base: &Url, reference: &str) -> String {
    // Fragments are never retained (`Url` has no field for one); strip first
    // so every branch below can ignore them uniformly.
    let reference = match reference.find('#') {
        Some(i) => &reference[..i],
        None => reference,
    };

    if split_scheme(reference).is_some() {
        // Reference carries its own scheme: it is already absolute.
        return reference.to_string();
    }

    let base_parts = base.parts();
    let base_path = if base_parts.path.is_empty() {
        "/"
    } else {
        base_parts.path.as_str()
    };

    if let Some(auth_ref) = reference.strip_prefix("//") {
        // Network-path reference: new authority, base scheme.
        return format!("{}://{}", base_parts.scheme, auth_ref);
    }

    if reference.is_empty() {
        let q = base_parts
            .query
            .as_deref()
            .map(|q| format!("?{}", q))
            .unwrap_or_default();
        return build_url(&base_parts, &format!("{}{}", base_path, q));
    }

    if let Some(q) = reference.strip_prefix('?') {
        return build_url(&base_parts, &format!("{}?{}", base_path, q));
    }

    if let Some(abs_path_and_query) = reference.strip_prefix('/') {
        let full = format!("/{}", abs_path_and_query);
        let (path, query) = split_path_query(&full);
        let normalized = remove_dot_segments(path);
        return build_url(&base_parts, &with_query(&normalized, query));
    }

    // Relative-path reference: merge with the base path's directory, then
    // remove dot segments (RFC 3986 §5.3 step 6, §5.2.4).
    let (ref_path, ref_query) = split_path_query(reference);
    let merged = merge_paths(base_path, ref_path);
    let mut normalized = remove_dot_segments(&merged);
    if normalized.is_empty() {
        normalized = "/".to_string();
    }
    build_url(&base_parts, &with_query(&normalized, ref_query))
}

fn split_path_query(s: &str) -> (&str, Option<&str>) {
    match s.find('?') {
        Some(i) => (&s[..i], s.get(i + 1..)),
        None => (s, None),
    }
}

fn with_query(path: &str, query: Option<&str>) -> String {
    match query {
        Some(q) => format!("{}?{}", path, q),
        None => path.to_string(),
    }
}

fn build_url(base_parts: &UrlParts, path_and_query: &str) -> String {
    match base_parts.port {
        Some(p) => format!(
            "{}://{}:{}{}",
            base_parts.scheme, base_parts.host, p, path_and_query
        ),
        None => format!("{}://{}{}", base_parts.scheme, base_parts.host, path_and_query),
    }
}

fn merge_paths(base_path: &str, ref_path: &str) -> String {
    if !base_path.is_empty() {
        match base_path.rfind('/') {
            Some(i) => format!("{}{}", &base_path[..=i], ref_path),
            None => ref_path.to_string(),
        }
    } else {
        format!("/{}", ref_path)
    }
}

/// RFC 3986 §5.2.4 "Remove Dot Segments", total (bounded loop — malformed
/// input can never spin forever).
fn remove_dot_segments(path: &str) -> String {
    let mut input = path;
    let mut output = String::new();
    let mut guard = 0usize;

    while !input.is_empty() {
        guard += 1;
        if guard > 10_000 {
            break; // pathological input; bail out rather than loop forever
        }

        if let Some(rest) = input.strip_prefix("../") {
            input = rest;
        } else if let Some(rest) = input.strip_prefix("./") {
            input = rest;
        } else if input.starts_with("/./") {
            input = &input[2..]; // "/./x" -> "/x"
        } else if input == "/." {
            input = "/";
        } else if input.starts_with("/../") {
            input = &input[3..]; // "/../x" -> "/x"
            remove_last_segment(&mut output);
        } else if input == "/.." {
            input = "/";
            remove_last_segment(&mut output);
        } else if input == "." || input == ".." {
            input = "";
        } else {
            let end = if let Some(tail) = input.strip_prefix('/') {
                1 + tail.find('/').unwrap_or(tail.len())
            } else {
                input.find('/').unwrap_or(input.len())
            };
            let end = end.min(input.len());
            output.push_str(&input[..end]);
            input = &input[end..];
        }
    }

    output
}

fn remove_last_segment(output: &mut String) {
    match output.rfind('/') {
        Some(idx) => output.truncate(idx),
        None => output.clear(),
    }
}
