//! Browsing history and bookmarks, kept as small text files and matched
//! against what the user types in the address field.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::log;

/// Oldest visits are forgotten past this, which keeps history under ~0.5 MB.
const HISTORY_LIMIT: usize = 3000;
pub const SUGGESTION_LIMIT: usize = 6;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

fn write(path: &PathBuf, text: &str) {
    let tmp = path.with_extension("tmp");
    let result = std::fs::create_dir_all(crate::blocker::data_dir())
        .and_then(|_| std::fs::write(&tmp, text))
        .and_then(|_| std::fs::rename(&tmp, path));
    if let Err(e) = result {
        log::log(&format!("save {} failed: {e}", path.display()));
    }
}

/// Only real web pages belong in history and bookmarks.
fn keepable(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// The part of a URL people type: no scheme, no `www.`.
fn typed_form(url: &str) -> &str {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    rest.strip_prefix("www.").unwrap_or(rest)
}

pub struct Visit {
    pub url: String,
    pub title: String,
    pub visits: u32,
    pub last: u64,
}

#[derive(Default)]
pub struct History {
    /// Most recent last.
    entries: Vec<Visit>,
    pub dirty: bool,
}

impl History {
    fn path() -> PathBuf {
        crate::blocker::data_dir().join("history.txt")
    }

    pub fn load() -> Self {
        let mut history = Self::default();
        if let Ok(text) = std::fs::read_to_string(Self::path()) {
            history.decode(&text);
        }
        history
    }

    fn decode(&mut self, text: &str) {
        for line in text.lines() {
            let mut parts = line.splitn(4, '\t');
            let (Some(visits), Some(last), Some(url)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            if !keepable(url) {
                continue;
            }
            self.entries.push(Visit {
                url: url.to_string(),
                title: parts.next().unwrap_or("").to_string(),
                visits: visits.parse().unwrap_or(1),
                last: last.parse().unwrap_or(0),
            });
        }
        self.entries.sort_by_key(|v| v.last);
    }

    fn encode(&self) -> String {
        let mut out = String::new();
        for v in &self.entries {
            out.push_str(&format!(
                "{}\t{}\t{}\t{}\n",
                v.visits,
                v.last,
                clean(&v.url),
                clean(&v.title)
            ));
        }
        out
    }

    pub fn save(&mut self) {
        if self.dirty {
            self.dirty = false;
            write(&Self::path(), &self.encode());
        }
    }

    pub fn record(&mut self, url: &str, title: &str) {
        if !keepable(url) {
            return;
        }
        let visit = match self.entries.iter().position(|v| v.url == url) {
            Some(i) => {
                let mut v = self.entries.remove(i);
                v.visits = v.visits.saturating_add(1);
                v
            }
            None => Visit {
                url: url.to_string(),
                title: String::new(),
                visits: 1,
                last: 0,
            },
        };
        self.entries.push(Visit {
            last: now(),
            title: if title.is_empty() { visit.title } else { title.to_string() },
            ..visit
        });
        if self.entries.len() > HISTORY_LIMIT {
            let excess = self.entries.len() - HISTORY_LIMIT;
            self.entries.drain(..excess);
        }
        self.dirty = true;
    }

    pub fn set_title(&mut self, url: &str, title: &str) {
        if let Some(v) = self.entries.iter_mut().rev().find(|v| v.url == url)
            && v.title != title
            && !title.is_empty()
        {
            v.title = title.to_string();
            self.dirty = true;
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.dirty = true;
    }

    /// Most recent first.
    pub fn recent(&self, n: usize) -> impl Iterator<Item = &Visit> {
        self.entries.iter().rev().take(n)
    }
}

pub struct Bookmark {
    pub url: String,
    pub title: String,
}

#[derive(Default)]
pub struct Bookmarks {
    pub items: Vec<Bookmark>,
    pub dirty: bool,
    modified: Option<SystemTime>,
}

impl Bookmarks {
    pub fn path() -> PathBuf {
        crate::blocker::data_dir().join("bookmarks.txt")
    }

    fn file_time() -> Option<SystemTime> {
        std::fs::metadata(Self::path()).and_then(|m| m.modified()).ok()
    }

    pub fn load() -> Self {
        let mut bookmarks = Self {
            modified: Self::file_time(),
            ..Self::default()
        };
        if let Ok(text) = std::fs::read_to_string(Self::path()) {
            bookmarks.decode(&text);
        }
        bookmarks
    }

    fn decode(&mut self, text: &str) {
        self.items = text
            .lines()
            .filter_map(|line| {
                let (url, title) = line.split_once('\t').unwrap_or((line, ""));
                let url = url.trim();
                keepable(url).then(|| Bookmark {
                    url: url.to_string(),
                    title: title.trim().to_string(),
                })
            })
            .collect();
    }

    fn encode(&self) -> String {
        self.items
            .iter()
            .map(|b| format!("{}\t{}\n", clean(&b.url), clean(&b.title)))
            .collect()
    }

    pub fn save(&mut self) {
        if self.dirty {
            self.dirty = false;
            write(&Self::path(), &self.encode());
            self.modified = Self::file_time();
        }
    }

    /// Picks up edits made in a text editor. Returns true if the list changed.
    pub fn reload_if_edited(&mut self) -> bool {
        let modified = Self::file_time();
        if modified == self.modified || self.dirty {
            return false;
        }
        self.modified = modified;
        let text = std::fs::read_to_string(Self::path()).unwrap_or_default();
        self.decode(&text);
        true
    }

    pub fn contains(&self, url: &str) -> bool {
        self.items.iter().any(|b| b.url == url)
    }

    /// Adds or removes `url`; returns whether it is now bookmarked.
    pub fn toggle(&mut self, url: &str, title: &str) -> bool {
        self.dirty = true;
        if let Some(i) = self.items.iter().position(|b| b.url == url) {
            self.items.remove(i);
            false
        } else if keepable(url) {
            self.items.push(Bookmark {
                url: url.to_string(),
                title: title.to_string(),
            });
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub url: String,
    pub title: String,
    pub bookmarked: bool,
}

/// How well `query` matches a page; `None` when it does not match at all.
fn score(query: &str, words: &[&str], url: &str, title: &str) -> Option<i64> {
    let typed = typed_form(url).to_lowercase();
    let title = title.to_lowercase();
    if typed.starts_with(query) {
        // Shorter URLs first: "git" should offer github.com before a deep link.
        return Some(1000 - typed.len().min(200) as i64);
    }
    let in_words = |text: &str| {
        text.split(|c: char| !c.is_alphanumeric())
            .any(|w| w.starts_with(query))
    };
    if in_words(&title) || in_words(&typed) {
        return Some(400);
    }
    words
        .iter()
        .all(|w| typed.contains(w) || title.contains(w))
        .then_some(100)
}

/// Pages from bookmarks and history that match what the user typed.
pub fn suggest(input: &str, history: &History, bookmarks: &Bookmarks) -> Vec<Suggestion> {
    let query = input.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let words: Vec<&str> = query.split_whitespace().collect();
    let now = now();
    let mut found: Vec<(i64, Suggestion)> = Vec::new();
    for b in &bookmarks.items {
        if let Some(s) = score(&query, &words, &b.url, &b.title) {
            found.push((
                s + 300,
                Suggestion {
                    url: b.url.clone(),
                    title: b.title.clone(),
                    bookmarked: true,
                },
            ));
        }
    }
    for v in &history.entries {
        let Some(s) = score(&query, &words, &v.url, &v.title) else {
            continue;
        };
        let age = now.saturating_sub(v.last);
        let recency = match age {
            0..=86_400 => 120,
            86_401..=604_800 => 60,
            _ => 0,
        };
        let frequency = (f64::from(v.visits).ln() * 60.0) as i64;
        let total = s + recency + frequency;
        match found.iter_mut().find(|(_, f)| f.url == v.url) {
            Some((existing, f)) => {
                *existing += recency + frequency;
                if f.title.is_empty() {
                    f.title = v.title.clone();
                }
            }
            None => found.push((
                total,
                Suggestion {
                    url: v.url.clone(),
                    title: v.title.clone(),
                    bookmarked: false,
                },
            )),
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.truncate(SUGGESTION_LIMIT);
    found.into_iter().map(|(_, s)| s).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(pages: &[(&str, &str, u32)]) -> History {
        let mut h = History::default();
        for (url, title, visits) in pages {
            for _ in 0..*visits {
                h.record(url, title);
            }
        }
        h
    }

    #[test]
    fn prefix_matches_rank_first() {
        let h = history(&[
            ("https://github.com/rust-lang/rust/issues", "Issues", 1),
            ("https://github.com/", "GitHub", 1),
            ("https://news.example.com/git-tips", "Git tips", 5),
        ]);
        let urls: Vec<String> = suggest("git", &h, &Bookmarks::default())
            .into_iter()
            .map(|s| s.url)
            .collect();
        assert_eq!(urls[0], "https://github.com/");
        assert_eq!(urls[1], "https://github.com/rust-lang/rust/issues");
        assert!(urls.contains(&"https://news.example.com/git-tips".to_string()));
    }

    #[test]
    fn bookmarks_boost_and_dedupe() {
        let h = history(&[("https://docs.rs/", "Docs.rs", 1)]);
        let mut b = Bookmarks::default();
        b.toggle("https://docs.rs/", "Docs");
        let s = suggest("docs", &h, &b);
        assert_eq!(s.len(), 1);
        assert!(s[0].bookmarked);
    }

    #[test]
    fn words_match_titles_and_nothing_else() {
        let h = history(&[("https://example.com/a", "Rust Borrow Checker", 1)]);
        assert_eq!(suggest("borrow", &h, &Bookmarks::default()).len(), 1);
        assert_eq!(suggest("rust checker", &h, &Bookmarks::default()).len(), 1);
        assert!(suggest("python", &h, &Bookmarks::default()).is_empty());
        assert!(suggest("  ", &h, &Bookmarks::default()).is_empty());
    }

    #[test]
    fn history_round_trip_and_limits() {
        let mut h = history(&[("https://a.com/", "A\tb", 2), ("about:blank", "x", 1)]);
        assert_eq!(h.entries.len(), 1);
        assert_eq!(h.entries[0].visits, 2);
        let mut back = History::default();
        back.decode(&h.encode());
        assert_eq!(back.entries[0].title, "A b");
        for i in 0..HISTORY_LIMIT + 5 {
            h.record(&format!("https://x.com/{i}"), "");
        }
        assert_eq!(h.entries.len(), HISTORY_LIMIT);
    }

    #[test]
    fn bookmark_toggle() {
        let mut b = Bookmarks::default();
        assert!(b.toggle("https://a.com/", "A"));
        assert!(b.contains("https://a.com/"));
        assert!(!b.toggle("https://a.com/", "A"));
        assert!(!b.toggle("about:blank", ""));
        assert!(b.items.is_empty());
    }
}
