//! The cookie jar: Netscape-format text the user owns (charter C6). P3 (Wave 1)
//! implements domain/path matching and Set-Cookie parsing; no third-party
//! cookies in v0 (brief §4 — a DECISIONS note if that bites a fixture). Cookie
//! rules are contract-tested, so this arrives as a typed skeleton.

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
    pub fn set_from_header(&mut self, _url: &Url, _set_cookie: &str) {
        todo!("P3: parse Set-Cookie, apply domain/path rules")
    }

    /// The `Cookie:` header value to send for `url`, or `None` if the jar has
    /// nothing that matches.
    pub fn header_for(&self, _url: &Url) -> Option<String> {
        todo!("P3: domain/path match + serialize")
    }

    /// Serialize the whole jar to Netscape jar text.
    pub fn to_netscape(&self) -> String {
        todo!("P3: Netscape jar format")
    }

    /// Parse a Netscape-format cookie jar text file back into a jar. Additive
    /// to the frozen skeleton (not required by the trait/type freeze) — the
    /// load half of charter C6's plain-file cookie jar, needed to persist the
    /// jar across runs.
    pub fn from_netscape(_text: &str) -> Self {
        todo!("P3: Netscape jar format -> CookieJar")
    }
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
