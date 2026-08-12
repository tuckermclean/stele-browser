//! The cookie jar: Netscape-format text the user owns (charter C6). P3 (Wave 1)
//! implements domain/path matching and Set-Cookie parsing; no third-party
//! cookies in v0 (brief §4 — a DECISIONS note if that bites a fixture). Cookie
//! rules are contract-tested, so this arrives as a typed skeleton.
//!
//! v0 choices recorded in DECISIONS.md: (1) `Expires`/`Max-Age` are parsed-
//! and-ignored — every cookie behaves as a session cookie (the frozen
//! `Cookie` shape has no expiry field to store them in); (2) a cookie's
//! `domain` is stored WITH a leading `.` when it came from an explicit
//! `Domain=` attribute (subdomains match) and WITHOUT one for a host-only
//! cookie (exact host match only) — this doubles as the Netscape-format
//! "include subdomains" flag with no extra field needed.

use super::Url;

/// One stored cookie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cookie {
    pub domain: String,
    pub path: String,
    pub name: String,
    pub value: String,
    pub secure: bool,
}

/// The jar. Backed on disk by a Netscape-format text file; grep is the API.
#[derive(Debug, Clone, Default)]
pub struct CookieJar {
    cookies: Vec<Cookie>,
}

impl CookieJar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ingest one `Set-Cookie` header value against the response URL.
    pub fn set_from_header(&mut self, url: &Url, set_cookie: &str) {
        let Some(cookie) = parse_set_cookie(url, set_cookie) else {
            return; // malformed: silently ignored, never panics (brief §9)
        };
        match self.cookies.iter_mut().find(|c| {
            c.domain.eq_ignore_ascii_case(&cookie.domain)
                && c.path == cookie.path
                && c.name == cookie.name
        }) {
            Some(existing) => *existing = cookie,
            None => self.cookies.push(cookie),
        }
    }

    /// The `Cookie:` header value to send for `url`, or `None` if the jar has
    /// nothing that matches.
    pub fn header_for(&self, url: &Url) -> Option<String> {
        let host = url.host();
        let path = url.path();
        let path = if path.is_empty() { "/" } else { path.as_str() };
        let is_secure_origin = url.scheme().eq_ignore_ascii_case("https");

        let mut matching: Vec<&Cookie> = self
            .cookies
            .iter()
            .filter(|c| {
                domain_matches(&c.domain, &host)
                    && path_matches(&c.path, path)
                    && (!c.secure || is_secure_origin)
            })
            .collect();
        if matching.is_empty() {
            return None;
        }
        // More specific (longer) paths first, per RFC 6265 §5.4's ordering.
        matching.sort_by(|a, b| b.path.len().cmp(&a.path.len()));
        Some(
            matching
                .iter()
                .map(|c| format!("{}={}", c.name, c.value))
                .collect::<Vec<_>>()
                .join("; "),
        )
    }

    /// Serialize the whole jar to Netscape jar text.
    pub fn to_netscape(&self) -> String {
        let mut out = String::from("# Netscape HTTP Cookie File\n");
        for c in &self.cookies {
            let flag = if c.domain.starts_with('.') { "TRUE" } else { "FALSE" };
            let secure = if c.secure { "TRUE" } else { "FALSE" };
            let path = if c.path.is_empty() { "/" } else { &c.path };
            // Expiration column: always 0 (session cookie; v0 does not track
            // Expires/Max-Age — see the module doc comment).
            out.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                c.domain, flag, path, secure, 0, c.name, c.value
            ));
        }
        out
    }

    /// Parse a Netscape-format cookie jar text file back into a jar. Additive
    /// to the frozen skeleton (not required by the trait/type freeze) — the
    /// load half of charter C6's plain-file cookie jar, needed to persist the
    /// jar across runs.
    pub fn from_netscape(text: &str) -> Self {
        let mut jar = Self::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 7 {
                continue; // malformed line: skip, never panic
            }
            let domain = fields[0];
            let path = fields[2];
            let secure = fields[3];
            let name = fields[5];
            let value = fields[6];
            if name.is_empty() {
                continue;
            }
            jar.cookies.push(Cookie {
                domain: domain.to_string(),
                path: path.to_string(),
                name: name.to_string(),
                value: value.to_string(),
                secure: secure.eq_ignore_ascii_case("TRUE"),
            });
        }
        jar
    }
}

/// RFC 6265 §5.1.3 domain-match, adapted to our leading-dot convention: a
/// cookie domain starting with `.` matches its exact suffix plus any
/// subdomain; a bare (host-only) domain matches only that exact host.
fn domain_matches(cookie_domain: &str, host: &str) -> bool {
    match cookie_domain.strip_prefix('.') {
        Some(suffix) => {
            host.eq_ignore_ascii_case(suffix)
                || host
                    .to_ascii_lowercase()
                    .ends_with(&format!(".{}", suffix.to_ascii_lowercase()))
        }
        None => host.eq_ignore_ascii_case(cookie_domain),
    }
}

/// RFC 6265 §5.1.4 path-match: exact match, or `request_path` extends
/// `cookie_path` at a `/` segment boundary (not merely as a string prefix).
fn path_matches(cookie_path: &str, request_path: &str) -> bool {
    let cookie_path = if cookie_path.is_empty() { "/" } else { cookie_path };
    if cookie_path == request_path {
        return true;
    }
    if let Some(rest) = request_path.strip_prefix(cookie_path) {
        if cookie_path.ends_with('/') {
            return true;
        }
        return rest.starts_with('/');
    }
    false
}

/// RFC 6265 §5.1.4 default-path: the directory of the request path (drop the
/// last `/`-terminated segment), or `/` if there isn't one to drop.
fn default_path(request_path: &str) -> String {
    if !request_path.starts_with('/') {
        return "/".to_string();
    }
    match request_path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(i) => request_path[..i].to_string(),
    }
}

/// Parse one `Set-Cookie` header value. Total: any malformed input (no `=`,
/// empty name, empty header) yields `None` rather than panicking.
fn parse_set_cookie(url: &Url, set_cookie: &str) -> Option<Cookie> {
    let mut parts = set_cookie.split(';');
    let first = parts.next()?.trim();
    let eq = first.find('=')?;
    let name = first[..eq].trim();
    let value = first[eq + 1..].trim();
    if name.is_empty() {
        return None;
    }

    let mut domain: Option<String> = None;
    let mut path: Option<String> = None;
    let mut secure = false;

    for attr in parts {
        let attr = attr.trim();
        if attr.is_empty() {
            continue;
        }
        let (key, val) = match attr.find('=') {
            Some(i) => (attr[..i].trim(), Some(attr[i + 1..].trim())),
            None => (attr, None),
        };
        match key.to_ascii_lowercase().as_str() {
            "domain" => {
                if let Some(v) = val {
                    let v = v.trim().trim_start_matches('.');
                    if !v.is_empty() {
                        domain = Some(format!(".{}", v.to_ascii_lowercase()));
                    }
                }
            }
            "path" => {
                if let Some(v) = val {
                    if v.starts_with('/') {
                        path = Some(v.to_string());
                    }
                }
            }
            "secure" => secure = true,
            // HttpOnly, Expires, Max-Age, SameSite: parsed-and-ignored in v0
            // (see the module doc comment — every cookie is a session cookie).
            _ => {}
        }
    }

    let domain = domain.unwrap_or_else(|| url.host().to_ascii_lowercase());
    let path = path.unwrap_or_else(|| {
        let p = url.path();
        default_path(if p.is_empty() { "/" } else { &p })
    });

    Some(Cookie {
        domain,
        path,
        name: name.to_string(),
        value: value.to_string(),
        secure,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::new(s)
    }

    // -- Set-Cookie parsing ---------------------------------------------

    #[test]
    fn set_from_header_basic_name_value_is_host_only_default_path() {
        let mut jar = CookieJar::new();
        jar.set_from_header(&url("http://example.com/a/b/c"), "sid=abc123");
        let h = jar.header_for(&url("http://example.com/a/b/x")).unwrap();
        assert_eq!(h, "sid=abc123");
        // Different host must not match a host-only cookie.
        assert_eq!(jar.header_for(&url("http://other.com/a/b/c")), None);
    }

    #[test]
    fn set_from_header_domain_attribute_enables_subdomain_matching() {
        let mut jar = CookieJar::new();
        jar.set_from_header(
            &url("http://www.example.com/"),
            "sid=abc; Domain=example.com; Path=/",
        );
        assert!(jar
            .header_for(&url("http://example.com/"))
            .unwrap()
            .contains("sid=abc"));
        assert!(jar
            .header_for(&url("http://api.example.com/"))
            .unwrap()
            .contains("sid=abc"));
        assert_eq!(jar.header_for(&url("http://notexample.com/")), None);
        assert_eq!(jar.header_for(&url("http://evilexample.com/")), None);
    }

    #[test]
    fn set_from_header_path_attribute_is_respected() {
        let mut jar = CookieJar::new();
        jar.set_from_header(&url("http://example.com/"), "sid=abc; Path=/admin");
        assert!(jar.header_for(&url("http://example.com/admin")).is_some());
        assert!(jar
            .header_for(&url("http://example.com/admin/x"))
            .is_some());
        assert_eq!(jar.header_for(&url("http://example.com/other")), None);
    }

    #[test]
    fn set_from_header_secure_flag_is_parsed() {
        let mut jar = CookieJar::new();
        jar.set_from_header(&url("https://example.com/"), "sid=abc; Secure");
        assert!(jar.header_for(&url("https://example.com/")).is_some());
        assert_eq!(jar.header_for(&url("http://example.com/")), None);
    }

    #[test]
    fn set_from_header_ignores_unknown_attributes_without_breaking_parse() {
        let mut jar = CookieJar::new();
        jar.set_from_header(
            &url("http://example.com/"),
            "sid=abc; HttpOnly; Expires=Wed, 21 Oct 2026 07:28:00 GMT; Max-Age=3600; SameSite=Lax",
        );
        assert_eq!(
            jar.header_for(&url("http://example.com/")),
            Some("sid=abc".to_string())
        );
    }

    #[test]
    fn set_from_header_malformed_is_silently_ignored_not_panicking() {
        let mut jar = CookieJar::new();
        jar.set_from_header(&url("http://example.com/"), "");
        jar.set_from_header(&url("http://example.com/"), "notanamevalue");
        jar.set_from_header(&url("http://example.com/"), "=novalue");
        jar.set_from_header(&url("http://example.com/"), ";;;");
        assert_eq!(jar.header_for(&url("http://example.com/")), None);
    }

    #[test]
    fn set_from_header_replaces_same_domain_path_name() {
        let mut jar = CookieJar::new();
        jar.set_from_header(&url("http://example.com/"), "sid=first");
        jar.set_from_header(&url("http://example.com/"), "sid=second");
        assert_eq!(
            jar.header_for(&url("http://example.com/")),
            Some("sid=second".to_string())
        );
    }

    // -- Domain= cross-origin validation (RFC 6265 §5.3 step 6) ----------

    #[test]
    fn set_from_header_domain_attribute_rejected_when_responding_host_is_unrelated() {
        let mut jar = CookieJar::new();
        // attacker.test must not be able to mint a cookie scoped to
        // example.com just by naming it in Domain=.
        jar.set_from_header(&url("http://attacker.test/"), "sid=x; Domain=example.com");
        assert_eq!(jar.header_for(&url("http://example.com/")), None);
        // The whole cookie is rejected, not silently downgraded to
        // host-only for attacker.test either.
        assert_eq!(jar.header_for(&url("http://attacker.test/")), None);
    }

    #[test]
    fn set_from_header_single_label_domain_is_rejected_as_a_supercookie() {
        let mut jar = CookieJar::new();
        // No public-suffix list in v0 (heuristic, see DECISIONS): a
        // single-label Domain= (no embedded dot) is always rejected, even
        // when it *would* otherwise domain-match the responding host.
        jar.set_from_header(&url("http://shop.com/"), "sid=x; Domain=com");
        assert_eq!(jar.header_for(&url("http://shop.com/")), None);
        assert_eq!(jar.header_for(&url("http://other.com/")), None);
    }

    #[test]
    fn set_from_header_domain_attribute_accepted_when_responding_host_domain_matches() {
        let mut jar = CookieJar::new();
        jar.set_from_header(&url("http://www.example.com/"), "sid=x; Domain=example.com");
        assert_eq!(
            jar.header_for(&url("http://example.com/")),
            Some("sid=x".to_string())
        );
    }

    #[test]
    fn set_from_header_host_only_cookie_unaffected_by_domain_validation() {
        let mut jar = CookieJar::new();
        jar.set_from_header(&url("http://example.com/"), "sid=x");
        assert_eq!(
            jar.header_for(&url("http://example.com/")),
            Some("sid=x".to_string())
        );
    }

    // -- IP-literal host: domain-match must be exact, not suffix ---------

    #[test]
    fn domain_match_for_ip_literal_host_is_exact_not_suffix() {
        let mut jar = CookieJar::new();
        jar.set_from_header(&url("http://127.0.0.1/"), "sid=x; Domain=127.0.0.1");
        assert_eq!(
            jar.header_for(&url("http://127.0.0.1/")),
            Some("sid=x".to_string())
        );
        // ".127.0.0.1" must not suffix-match "foo.127.0.0.1" as if it were
        // a subdomain of an IP address (RFC 6265 §5.1.3).
        assert_eq!(jar.header_for(&url("http://foo.127.0.0.1/")), None);
    }

    // -- domain/path matching (direct, via header_for) -------------------

    #[test]
    fn header_for_path_boundary_is_a_segment_not_a_prefix() {
        let mut jar = CookieJar::new();
        jar.set_from_header(&url("http://example.com/"), "sid=abc; Path=/foo");
        // "/foobar" has "/foo" as a string prefix but not as a path segment.
        assert_eq!(jar.header_for(&url("http://example.com/foobar")), None);
        assert!(jar.header_for(&url("http://example.com/foo")).is_some());
        assert!(jar.header_for(&url("http://example.com/foo/bar")).is_some());
    }

    #[test]
    fn header_for_joins_multiple_matching_cookies() {
        let mut jar = CookieJar::new();
        jar.set_from_header(&url("http://example.com/"), "a=1; Path=/");
        jar.set_from_header(&url("http://example.com/"), "b=2; Path=/");
        let h = jar.header_for(&url("http://example.com/")).unwrap();
        assert!(h.contains("a=1"));
        assert!(h.contains("b=2"));
        assert!(h.contains("; "));
    }

    #[test]
    fn header_for_no_matches_is_none() {
        let jar = CookieJar::new();
        assert_eq!(jar.header_for(&url("http://example.com/")), None);
    }

    // -- to_netscape / from_netscape --------------------------------------

    #[test]
    fn to_netscape_format_has_tab_separated_fields_and_correct_flags() {
        let mut jar = CookieJar::new();
        jar.set_from_header(&url("http://example.com/"), "sid=abc; Path=/");
        jar.set_from_header(
            &url("http://www.example.com/"),
            "wide=1; Domain=example.com; Path=/; Secure",
        );
        let text = jar.to_netscape();
        let lines: Vec<&str> = text.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(lines.len(), 2);
        for line in &lines {
            let fields: Vec<&str> = line.split('\t').collect();
            assert_eq!(fields.len(), 7, "line {:?} must have 7 tab-separated fields", line);
        }
        let host_only = lines.iter().find(|l| l.contains("sid")).unwrap();
        let host_only_fields: Vec<&str> = host_only.split('\t').collect();
        assert_eq!(host_only_fields[1], "FALSE"); // host-only -> flag FALSE
        assert_eq!(host_only_fields[3], "FALSE"); // not secure

        let wide = lines.iter().find(|l| l.contains("wide")).unwrap();
        let wide_fields: Vec<&str> = wide.split('\t').collect();
        assert_eq!(wide_fields[1], "TRUE"); // Domain attribute -> flag TRUE
        assert_eq!(wide_fields[3], "TRUE"); // secure
    }

    #[test]
    fn to_netscape_from_netscape_round_trip_preserves_matching_behavior() {
        let mut jar = CookieJar::new();
        jar.set_from_header(&url("http://example.com/"), "sid=abc; Path=/");
        jar.set_from_header(
            &url("http://www.example.com/"),
            "wide=1; Domain=example.com; Path=/sub; Secure",
        );
        let text = jar.to_netscape();
        let reloaded = CookieJar::from_netscape(&text);

        assert_eq!(
            reloaded.header_for(&url("http://example.com/")),
            jar.header_for(&url("http://example.com/"))
        );
        assert_eq!(
            reloaded.header_for(&url("https://api.example.com/sub/x")),
            jar.header_for(&url("https://api.example.com/sub/x"))
        );
    }
}
