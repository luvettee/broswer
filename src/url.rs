pub const HOME: &str = "https://duckduckgo.com";
/// New tabs show a blank start page; the address field stays empty for it.
pub const NEW_TAB: &str = "about:blank";

/// What the address field shows for a page.
pub fn address(url: &str) -> &str {
    if url == NEW_TAB { "" } else { url }
}

/// An address as shown in lists: no `https://`, no trailing slash on a bare host.
pub fn display_address(url: &str) -> &str {
    let rest = url.strip_prefix("https://").unwrap_or(url);
    match rest.strip_suffix('/') {
        Some(host) if !host.contains('/') => host,
        _ => rest,
    }
}

pub fn display_title(url: &str) -> String {
    if url == NEW_TAB {
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

/// The site a page belongs to, for per-site settings: lowercase host without
/// `www.` or port. Only web pages have one.
pub fn site(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = if host.starts_with('[') {
        host.split_inclusive(']').next()?
    } else {
        host.split(':').next()?
    };
    let host = host.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    (!host.is_empty()).then(|| host.to_string())
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
    fn site_names() {
        assert_eq!(
            site("https://www.Example.com:8443/a?b"),
            Some("example.com".into())
        );
        assert_eq!(
            site("http://user@news.example.org/"),
            Some("news.example.org".into())
        );
        assert_eq!(site("about:blank"), None);
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
