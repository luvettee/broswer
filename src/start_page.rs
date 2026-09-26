//! Local new-tab page built from the user's bookmarks and browsing history.

use std::collections::HashSet;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::favicon;
use crate::places::{Bookmarks, History};
use crate::url;

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn tile(address: &str, title: &str) -> String {
    let name = if title.trim().is_empty() {
        url::display_title(address)
    } else {
        title.trim().to_string()
    };
    let site = url::site(address).unwrap_or_default();
    let icon = favicon::cached_png(&site)
        .map(|bytes| {
            format!(
                "<img alt=\"\" src=\"data:image/png;base64,{}\">",
                STANDARD.encode(bytes)
            )
        })
        .unwrap_or_else(|| {
            let initial = name
                .chars()
                .next()
                .unwrap_or('•')
                .to_uppercase()
                .collect::<String>();
            format!("<span class=\"initial\">{}</span>", escape(&initial))
        });
    format!(
        "<a class=\"tile\" href=\"{}\"><span class=\"icon\">{icon}</span><span class=\"name\">{}</span></a>",
        escape(address),
        escape(&name)
    )
}

pub fn render(bookmarks: &Bookmarks, history: &History) -> String {
    let mut saved = String::new();
    let mut seen = HashSet::new();
    for bookmark in bookmarks.items.iter().take(12) {
        if seen.insert(bookmark.url.as_str()) {
            saved.push_str(&tile(&bookmark.url, &bookmark.title));
        }
    }
    let mut frequent = String::new();
    let mut count = 0;
    for visit in history.frequent(100) {
        if seen.insert(&visit.url) {
            frequent.push_str(&tile(&visit.url, &visit.title));
            count += 1;
            if count == 8 {
                break;
            }
        }
    }
    let sections = if saved.is_empty() && frequent.is_empty() {
        "<p class=\"empty\">Bookmark a page or browse a few sites to see them here.</p>".to_string()
    } else {
        let mut sections = String::new();
        if !saved.is_empty() {
            sections.push_str(&format!(
                "<section><h2>Bookmarks</h2><div class=\"grid\">{saved}</div></section>"
            ));
        }
        if !frequent.is_empty() {
            sections.push_str(&format!(
                "<section><h2>Frequently visited</h2><div class=\"grid\">{frequent}</div></section>"
            ));
        }
        sections
    };
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="color-scheme" content="light dark">
<meta name="viewport" content="width=device-width, initial-scale=1"><title>New Tab</title>
<style>
:root {{ font-family: -apple-system, BlinkMacSystemFont, sans-serif; color-scheme: light dark; }}
* {{ box-sizing: border-box; }}
body {{ margin: 0; min-height: 100vh; background: Canvas; color: CanvasText; }}
main {{ width: min(800px, calc(100% - 64px)); margin: 0 auto; padding: max(64px, 12vh) 0 80px; }}
h1 {{ margin: 0 0 52px; font-size: clamp(32px, 5vw, 50px); font-weight: 650; letter-spacing: -.045em; }}
section {{ margin: 0 0 42px; }}
h2 {{ margin: 0 0 16px; color: GrayText; font-size: 12px; font-weight: 650; letter-spacing: .08em; text-transform: uppercase; }}
.grid {{ display: grid; grid-template-columns: repeat(auto-fill, minmax(140px, 1fr)); gap: 12px; }}
.tile {{ min-height: 108px; padding: 16px 12px; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 11px; overflow: hidden; border-radius: 17px; background: color-mix(in srgb, CanvasText 6%, Canvas); color: CanvasText; text-decoration: none; transition: background .15s, transform .15s; }}
.tile:hover, .tile:focus-visible {{ background: color-mix(in srgb, CanvasText 12%, Canvas); transform: translateY(-2px); outline: none; }}
.icon {{ width: 32px; height: 32px; display: grid; place-items: center; border-radius: 9px; background: color-mix(in srgb, CanvasText 9%, Canvas); overflow: hidden; }}
.icon img {{ width: 25px; height: 25px; object-fit: contain; }}
.initial {{ font-size: 19px; font-weight: 650; }}
.name {{ width: 100%; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; text-align: center; font-size: 13px; font-weight: 550; }}
.empty {{ color: GrayText; font-size: 14px; }}
@media (max-width: 560px) {{ main {{ width: calc(100% - 32px); padding-top: 48px; }} .grid {{ grid-template-columns: repeat(2, minmax(0, 1fr)); }} }}
</style></head><body><main><h1>New Tab</h1>{sections}</main></body></html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_bookmark_text_and_never_prints_urls_as_labels() {
        let mut bookmarks = Bookmarks::default();
        bookmarks.toggle("https://example.com/a?x=1&y=2", "<Example>");
        let html = render(&bookmarks, &History::default());
        assert!(html.contains("href=\"https://example.com/a?x=1&amp;y=2\""));
        assert!(html.contains("&lt;Example&gt;"));
        assert!(!html.contains(">https://example.com"));
    }
}
