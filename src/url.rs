pub const HOME: &str = "https://duckduckgo.com";

pub fn display_title(url: &str) -> String {
    if url == "about:blank" {
        return "New Tab".into();
    }
    if let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    {
        return rest
            .split(['/', '?', '#'])
            .next()
            .unwrap_or(rest)
            .trim_start_matches("www.")
            .to_string();
    }
    "Page".into()
}

/// Navigation policy. Keep web URLs intact: rewriting a sign-in redirect or
/// form submission can drop its body or invalidate a signed callback.
pub fn guard(url: &str) -> Option<String> {
    if url.starts_with("about:") || url.starts_with("data:") || url.starts_with("blob:") {
        return Some(url.into());
    }
    (url.starts_with("https://") || url.starts_with("http://")).then(|| url.into())
}

pub fn normalize(input: &str) -> String {
    let s = input.trim();
    if s.is_empty() {
        return HOME.into();
    }
    if s.starts_with("http://") || s.starts_with("https://") || s.starts_with("about:") {
        s.to_string()
    } else if let Some(rest) = s.strip_prefix("localhost") {
        format!("http://localhost{rest}")
    } else if s.starts_with("127.0.0.1") || s.starts_with("[::1]") {
        format!("http://{s}")
    } else if !s.contains(' ') && s.contains('.') {
        format!("https://{s}")
    } else {
        let q = s
            .replace('%', "%25")
            .replace('+', "%2B")
            .replace('&', "%26")
            .replace('?', "%3F")
            .replace('#', "%23")
            .replace(' ', "+");
        format!("https://duckduckgo.com/?q={q}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omnibar() {
        assert_eq!(normalize(""), HOME);
        assert_eq!(normalize("https://example.com"), "https://example.com");
        assert_eq!(normalize("example.com"), "https://example.com");
        assert_eq!(normalize("localhost:3000"), "http://localhost:3000");
        assert_eq!(normalize("http://example.com"), "http://example.com");
        assert_eq!(
            normalize("rust borrow checker"),
            "https://duckduckgo.com/?q=rust+borrow+checker"
        );
    }

    #[test]
    fn privacy_policy() {
        assert_eq!(
            guard("https://example.com/?fbclid=abc"),
            Some("https://example.com/?fbclid=abc".into())
        );
        assert_eq!(guard("file:///etc/passwd"), None);
        assert_eq!(guard("javascript:alert(1)"), None);
        assert_eq!(guard("about:blank"), Some("about:blank".into()));
    }
}
