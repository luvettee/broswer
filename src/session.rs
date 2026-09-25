//! Remembers open tabs between launches. Only the selected tab loads at start;
//! the others load when first selected, so a large session costs no memory.

use std::path::PathBuf;

use crate::log;
use crate::tabs::TabManager;

fn path() -> PathBuf {
    crate::blocker::data_dir().join("session.txt")
}

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

fn encode(tabs: &TabManager) -> String {
    let active = tabs.index_of(tabs.active).unwrap_or(0);
    let mut out = format!("{active}\n");
    for tab in &tabs.tabs {
        out.push_str(&clean(&tab.url));
        out.push('\t');
        out.push_str(&clean(&tab.title));
        out.push('\n');
    }
    out
}

fn decode(text: &str, tabs: &mut TabManager) {
    let mut lines = text.lines();
    let active: usize = lines.next().and_then(|l| l.parse().ok()).unwrap_or(0);
    for line in lines {
        let (url, title) = line.split_once('\t').unwrap_or((line, ""));
        if crate::url::guard(url).is_some() {
            tabs.restore(url, title);
        }
    }
    if let Some(tab) = tabs.tabs.get(active.min(tabs.tabs.len().saturating_sub(1))) {
        let id = tab.id;
        tabs.switch(id);
    }
}

/// Tabs from the last session, or a single start page.
pub fn load() -> TabManager {
    let mut tabs = TabManager::new();
    if let Ok(text) = std::fs::read_to_string(path()) {
        decode(&text, &mut tabs);
    }
    if tabs.tabs.is_empty() {
        tabs.new_tab(crate::url::HOME);
    }
    tabs
}

pub fn save(tabs: &TabManager) {
    let path = path();
    let tmp = path.with_extension("tmp");
    let result = std::fs::create_dir_all(crate::blocker::data_dir())
        .and_then(|_| std::fs::write(&tmp, encode(tabs)))
        .and_then(|_| std::fs::rename(&tmp, &path));
    if let Err(e) = result {
        log::log(&format!("session save failed: {e}"));
    }
}

fn sidebar_flag() -> PathBuf {
    crate::blocker::data_dir().join("sidebar-hidden")
}

pub fn sidebar_hidden() -> bool {
    sidebar_flag().exists()
}

pub fn set_sidebar_hidden(hidden: bool) {
    let result = if hidden {
        std::fs::create_dir_all(crate::blocker::data_dir())
            .and_then(|_| std::fs::write(sidebar_flag(), b""))
    } else {
        std::fs::remove_file(sidebar_flag()).or_else(|e| {
            if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) }
        })
    };
    if let Err(e) = result {
        log::log(&format!("sidebar setting save failed: {e}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut tabs = TabManager::new();
        tabs.new_tab("https://a.com");
        let b = tabs.new_tab("https://b.com/x");
        tabs.get_mut(b).unwrap().title = "Tab\twith\nbreaks".into();
        tabs.new_tab("about:blank");
        tabs.switch(b);

        let mut restored = TabManager::new();
        decode(&encode(&tabs), &mut restored);
        let urls: Vec<&str> = restored.tabs.iter().map(|t| t.url.as_str()).collect();
        assert_eq!(urls, ["https://a.com", "https://b.com/x", "about:blank"]);
        assert_eq!(restored.active_tab().unwrap().url, "https://b.com/x");
        assert_eq!(restored.active_tab().unwrap().title, "Tab with breaks");
    }

    #[test]
    fn skips_unsafe_urls() {
        let mut restored = TabManager::new();
        decode("5\nfile:///etc/passwd\tx\njavascript:1\t\n", &mut restored);
        assert!(restored.tabs.is_empty());
    }
}
