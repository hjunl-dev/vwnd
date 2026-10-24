// ============================================================
// nav
// - navigation classification: pure logic, no Win32 / WebView2
// - the policy values (which hosts, which schemes) are set by gui.rs
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavDecision {
    /// Stays in the app
    Allow,
    /// Handed to the system (default browser, mail app, ...)
    External,
    /// Cancelled
    Block,
}

#[derive(Clone, Copy, Debug)]
pub struct NavPolicy<'a> {
    /// http(s) hosts that stay in the app (subdomains included).
    /// None: every http(s) host stays in the app; Some: other hosts go External.
    pub in_app_hosts: Option<&'a [&'a str]>,
    /// Non-http(s) schemes handed to the system (lowercase). Other schemes are blocked.
    pub external_schemes: &'a [&'a str],
}

impl NavPolicy<'_> {
    pub fn classify(&self, uri: &str) -> NavDecision {
        let Some((scheme, rest)) = split_scheme(uri) else {
            return NavDecision::Block;
        };
        match scheme.as_str() {
            // NavigateToString / window.open('') / blob documents
            "about" | "data" | "blob" => NavDecision::Allow,
            "http" | "https" => {
                let Some(allowed) = self.in_app_hosts else {
                    return NavDecision::Allow;
                };
                match url_host(rest) {
                    Some(host) if host_matches(&host, allowed) => NavDecision::Allow,
                    Some(_) => NavDecision::External,
                    None => NavDecision::Block,
                }
            }
            s if self.external_schemes.contains(&s) => NavDecision::External,
            _ => NavDecision::Block,
        }
    }

    /// Only http(s) and `external_schemes` are ever handed to the system
    /// (about:/data:/blob: only make sense inside the app; unknown schemes may launch apps).
    pub fn can_open_externally(&self, uri: &str) -> bool {
        split_scheme(uri).is_some_and(|(scheme, _)| {
            matches!(scheme.as_str(), "http" | "https")
                || self.external_schemes.contains(&scheme.as_str())
        })
    }
}

/// Turns what a user typed (command line, address box) into a URL to navigate to:
/// trimmed, and "https://" added when there is no scheme ("example.com", "localhost:8080").
/// Kept as-is: "<scheme>://...", "about:...", "data:...".
pub fn normalize_user_url(input: &str) -> String {
    let input = input.trim();
    let lower = input.to_ascii_lowercase();
    if input.contains("://") || lower.starts_with("about:") || lower.starts_with("data:") {
        input.to_owned()
    } else {
        format!("https://{input}")
    }
}

/// Which documents may talk to the host over the WebMessage bridge.
/// Separate from NavPolicy: a page may be allowed to load in the app without being
/// allowed to call into it.
#[derive(Clone, Copy, Debug)]
pub struct BridgePolicy<'a> {
    /// https hosts whose pages may message the host (exact host or subdomain).
    /// Plain http is never trusted.
    pub trusted_hosts: &'a [&'a str],
    /// Trust "about:blank": the app's own NavigateToString pages report this source.
    /// A blank popup a page opens by script (window.open('')) reports it too, so this
    /// also trusts whatever that page writes into it.
    pub trust_about_blank: bool,
}

impl BridgePolicy<'_> {
    /// `source`: WebMessage::source(), the URI of the document that posted the message
    pub fn trusts(&self, source: &str) -> bool {
        let Some((scheme, rest)) = split_scheme(source) else {
            return false;
        };
        match scheme.as_str() {
            "about" => self.trust_about_blank && rest.eq_ignore_ascii_case("blank"),
            "https" => url_host(rest).is_some_and(|host| host_matches(&host, self.trusted_hosts)),
            _ => false,
        }
    }
}

// "HTTPS://x" -> ("https", "//x")
fn split_scheme(uri: &str) -> Option<(String, &str)> {
    let (scheme, rest) = uri.split_once(':')?;
    (!scheme.is_empty()).then(|| (scheme.to_ascii_lowercase(), rest))
}

// "//user@Host:8080/path?q" -> "host"
fn url_host(after_scheme: &str) -> Option<String> {
    let authority = after_scheme.strip_prefix("//")?;
    let authority = authority.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, hp)| hp);
    let host = if let Some(v6) = host_port.strip_prefix('[') {
        v6.split_once(']')?.0
    } else {
        host_port.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

// exact host or a subdomain of it ("a.example.com" matches "example.com",
// "badexample.com" does not)
fn host_matches(host: &str, allowed: &[&str]) -> bool {
    allowed.iter().any(|a| {
        host == *a
            || host
                .strip_suffix(a)
                .is_some_and(|prefix| prefix.ends_with('.'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPEN: NavPolicy<'static> = NavPolicy {
        in_app_hosts: None,
        external_schemes: &["mailto", "tel"],
    };

    const LIMITED: NavPolicy<'static> = NavPolicy {
        in_app_hosts: Some(&["example.com"]),
        external_schemes: &["mailto", "tel"],
    };

    #[test]
    fn in_app_documents_are_allowed() {
        for uri in [
            "about:blank",
            "data:text/html,<p>x</p>",
            "blob:https://example.com/id",
        ] {
            assert_eq!(OPEN.classify(uri), NavDecision::Allow, "{uri}");
            assert_eq!(LIMITED.classify(uri), NavDecision::Allow, "{uri}");
        }
    }

    #[test]
    fn every_http_host_is_allowed_without_a_host_list() {
        assert_eq!(OPEN.classify("https://www.google.com/"), NavDecision::Allow);
        assert_eq!(OPEN.classify("http://localhost:8080/"), NavDecision::Allow);
    }

    #[test]
    fn host_list_keeps_listed_hosts_and_subdomains() {
        for uri in [
            "https://example.com/",
            "https://sub.Example.com:8443/path",
            "HTTP://EXAMPLE.COM",
            "https://example.com?q=1",
            "https://example.com#frag",
        ] {
            assert_eq!(LIMITED.classify(uri), NavDecision::Allow, "{uri}");
        }
    }

    #[test]
    fn host_list_sends_other_hosts_out() {
        for uri in [
            "https://example.org/",
            "https://badexample.com/",
            "https://example.com.evil.com/x",
            // userinfo: the real host is after the last '@'
            "https://user@example.com@evil.com/",
            "http://[::1]:80/",
        ] {
            assert_eq!(LIMITED.classify(uri), NavDecision::External, "{uri}");
        }
    }

    #[test]
    fn http_without_a_host_is_blocked() {
        for uri in ["https:", "https:///path", "https://:443/"] {
            assert_eq!(LIMITED.classify(uri), NavDecision::Block, "{uri}");
        }
    }

    #[test]
    fn listed_schemes_go_external_case_insensitively() {
        assert_eq!(OPEN.classify("mailto:a@b.c"), NavDecision::External);
        assert_eq!(OPEN.classify("MAILTO:a@b.c"), NavDecision::External);
        assert_eq!(OPEN.classify("tel:+82-10-0000-0000"), NavDecision::External);
    }

    #[test]
    fn other_schemes_are_blocked() {
        for uri in [
            "javascript:alert(1)",
            "file:///C:/Windows/",
            "ms-settings:display",
            "edge://settings",
            "C:\\Windows\\notepad.exe",
            "no-scheme",
            ":empty-scheme",
        ] {
            assert_eq!(OPEN.classify(uri), NavDecision::Block, "{uri}");
        }
    }

    #[test]
    fn only_http_and_listed_schemes_open_externally() {
        for uri in ["https://x.y/", "HTTP://x.y", "mailto:a@b.c", "tel:123"] {
            assert!(OPEN.can_open_externally(uri), "{uri}");
        }
        for uri in [
            "about:blank",
            "data:,x",
            "file:///C:/",
            "ms-settings:",
            "no-scheme",
        ] {
            assert!(!OPEN.can_open_externally(uri), "{uri}");
        }
    }

    #[test]
    fn user_url_gets_https_when_scheme_is_missing() {
        assert_eq!(normalize_user_url("example.com"), "https://example.com");
        assert_eq!(
            normalize_user_url("localhost:8080/x"),
            "https://localhost:8080/x"
        );
        assert_eq!(
            normalize_user_url("  example.com/a?b  "),
            "https://example.com/a?b"
        );
    }

    #[test]
    fn user_url_with_scheme_is_kept() {
        for url in [
            "https://a.b/",
            "http://localhost:3000",
            "about:blank",
            "ABOUT:blank",
            "data:text/html,x",
        ] {
            assert_eq!(normalize_user_url(url), url, "{url}");
        }
    }

    const BRIDGE: BridgePolicy<'static> = BridgePolicy {
        trusted_hosts: &["example.com"],
        trust_about_blank: true,
    };

    #[test]
    fn bridge_trusts_listed_https_hosts_and_subdomains() {
        for src in [
            "https://example.com/",
            "https://app.example.com:8443/x",
            "HTTPS://EXAMPLE.COM",
        ] {
            assert!(BRIDGE.trusts(src), "{src}");
        }
    }

    #[test]
    fn bridge_rejects_other_sources() {
        for src in [
            "http://example.com/",
            "https://example.org/",
            "https://badexample.com/",
            "https://user@example.com@evil.com/",
            "data:text/html,x",
            "about:srcdoc",
            "about:blank#x",
            "file:///C:/x.html",
            "",
        ] {
            assert!(!BRIDGE.trusts(src), "{src}");
        }
    }

    #[test]
    fn bridge_about_blank_follows_the_flag() {
        assert!(BRIDGE.trusts("about:blank"));
        assert!(BRIDGE.trusts("ABOUT:BLANK"));
        let strict = BridgePolicy {
            trust_about_blank: false,
            ..BRIDGE
        };
        assert!(!strict.trusts("about:blank"));
    }

    #[test]
    fn url_host_extracts_lowercase_host() {
        assert_eq!(
            url_host("//Example.COM/path").as_deref(),
            Some("example.com")
        );
        assert_eq!(url_host("//u:p@host:1234/").as_deref(), Some("host"));
        assert_eq!(
            url_host("//[2001:db8::1]:8080/").as_deref(),
            Some("2001:db8::1")
        );
        assert_eq!(url_host("no-slashes"), None);
        assert_eq!(url_host("//"), None);
    }
}
