//! Converts Adblock Plus / EasyList filter syntax into WebKit content rule lists.
//!
//! WebKit evaluates a rule list top to bottom, and `ignore-previous-rules` only
//! cancels rules that appear earlier in the same list. Every compiled chunk is
//! therefore laid out in the same order, with all exceptions copied into it:
//!
//! 1. generic element hiding
//! 2. `$generichide` exceptions
//! 3. site-specific element hiding
//! 4. `$elemhide` exceptions
//! 5. network blocking
//! 6. network exceptions (`@@`, including `$document`)
//! 7. `$important` blocking
//! 8. the user's per-site allowlist

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write;

/// WebKit has accepted 150k rules per list since Safari 15; stay well below it
/// so each chunk compiles quickly and older engines still accept it.
const RULES_PER_CHUNK: usize = 50_000;
const SELECTORS_PER_RULE: usize = 100;

/// Every resource type except top-level documents. Filters without a type
/// option should not block a page the person navigates to directly.
const SUBRESOURCE_TYPES: &[&str] = &[
    "image",
    "style-sheet",
    "script",
    "font",
    "raw",
    "svg-document",
    "media",
    "ping",
];

/// Procedural and scriptlet syntax from uBlock Origin and AdGuard that a plain
/// CSS selector cannot express.
const EXTENDED_SELECTORS: &[&str] = &[
    ":-abp-",
    ":has-text(",
    ":contains(",
    ":xpath(",
    ":matches-css",
    ":matches-attr(",
    ":matches-prop(",
    ":matches-path(",
    ":matches-media(",
    ":min-text-length(",
    ":upward(",
    ":nth-ancestor(",
    ":remove(",
    ":remove-attr(",
    ":remove-class(",
    ":style(",
    ":watch-attr(",
    ":others(",
    ":if(",
    ":if-not(",
    ":shadow(",
    ":spath(",
];

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Segment {
    GenericHide,
    GenericHideException,
    SpecificHide,
    ElemHideException,
    Block,
    Exception,
    Important,
}

#[derive(Default, Clone)]
struct Trigger {
    url_filter: String,
    case_sensitive: bool,
    resource_types: Vec<&'static str>,
    load_type: Option<&'static str>,
    load_context: Option<&'static str>,
    if_domain: Vec<String>,
    unless_domain: Vec<String>,
}

enum Action<'a> {
    Block,
    Ignore,
    Hide(&'a str),
}

pub struct Converted {
    /// JSON rule lists, each ready for `WKContentRuleListStore`.
    pub chunks: Vec<String>,
    /// Blocking and hiding rules, not counting exceptions copied into chunks.
    pub rule_count: usize,
    /// Stable fingerprint of `chunks`, used to reuse compiled lists.
    pub fingerprint: u64,
}

#[derive(Default)]
struct Builder {
    rules: Vec<(Segment, String)>,
    seen: HashSet<(Segment, String)>,
    generic_hide: BTreeSet<String>,
    skip_generic: bool,
    specific_hide: BTreeMap<String, BTreeSet<String>>,
    hide_exceptions: HashMap<String, BTreeSet<String>>,
}

/// `generic_hiding` keeps element-hiding rules that apply on every site. WebKit
/// adds them to every page's style sheet, which on heavy pages costs tens of
/// megabytes; without them, network blocking and per-site hiding still apply.
pub fn convert(sources: &[&str], allowlist: &[String], generic_hiding: bool) -> Converted {
    let mut builder = Builder {
        skip_generic: !generic_hiding,
        ..Default::default()
    };
    for source in sources {
        for line in source.lines() {
            builder.line(line.trim());
        }
    }
    builder.finish_cosmetics();

    let mut allow = Vec::new();
    for host in allowlist {
        if let Some(domain) = domain_entry(host) {
            let trigger = Trigger {
                url_filter: ".*".into(),
                if_domain: vec![domain],
                ..Default::default()
            };
            allow.push(rule_json(&trigger, &Action::Ignore));
        }
    }

    builder.rules.sort_by_key(|(segment, _)| *segment);
    let is_exception = |segment: Segment| {
        matches!(
            segment,
            Segment::GenericHideException | Segment::ElemHideException | Segment::Exception
        )
    };
    let exceptions: Vec<_> = builder
        .rules
        .iter()
        .filter(|(segment, _)| is_exception(*segment))
        .collect();
    let payload: Vec<_> = builder
        .rules
        .iter()
        .filter(|(segment, _)| !is_exception(*segment))
        .collect();
    let rule_count = payload.len();
    let room = RULES_PER_CHUNK
        .saturating_sub(exceptions.len() + allow.len())
        .max(1_000);

    let mut chunks = Vec::new();
    let mut groups: Vec<&[&(Segment, String)]> = payload.chunks(room).collect();
    if groups.is_empty() {
        groups.push(&[]);
    }
    for group in groups {
        let mut rules: Vec<&(Segment, String)> = group.to_vec();
        rules.extend(exceptions.iter().copied());
        rules.sort_by_key(|(segment, _)| *segment);
        let mut json = String::from("[");
        for (i, rule) in rules
            .iter()
            .map(|(_, rule)| rule)
            .chain(allow.iter())
            .enumerate()
        {
            if i > 0 {
                json.push(',');
            }
            json.push_str(rule);
        }
        if json.len() == 1 {
            // WebKit rejects an empty list; this rule never matches a real URL.
            json.push_str(
                r#"{"trigger":{"url-filter":"^browser-empty-list:"},"action":{"type":"block"}}"#,
            );
        }
        json.push(']');
        chunks.push(json);
    }

    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for chunk in &chunks {
        for byte in chunk.bytes().chain([0]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    Converted {
        chunks,
        rule_count,
        fingerprint: hash,
    }
}

impl Builder {
    fn push(&mut self, segment: Segment, trigger: &Trigger, action: &Action) {
        let json = rule_json(trigger, action);
        if self.seen.insert((segment, json.clone())) {
            self.rules.push((segment, json));
        }
    }

    fn line(&mut self, line: &str) {
        if line.is_empty() || line.starts_with('!') || line.starts_with('[') {
            return;
        }
        if let Some((domains, selector, exception)) = split_cosmetic(line) {
            self.cosmetic(domains, selector, exception);
        } else if !line.contains("$$") && !line.contains("$@$") && !line.contains("#%#") {
            self.network(line);
        }
    }

    fn cosmetic(&mut self, domains: &str, selector: &str, exception: bool) {
        if !usable_selector(selector) {
            return;
        }
        let (include, exclude) = parse_domains(domains, ',');
        let selector = selector.to_string();
        if exception {
            let entry = self.hide_exceptions.entry(selector).or_default();
            if domains.is_empty() {
                // `#@#selector` turns the selector off everywhere.
                entry.insert(String::new());
            } else {
                entry.extend(include);
            }
            return;
        }
        if domains.is_empty() {
            if !self.skip_generic {
                self.generic_hide.insert(selector);
            }
        } else if !include.is_empty() {
            let key = include.into_iter().collect::<Vec<_>>().join(",");
            self.specific_hide.entry(key).or_default().insert(selector);
        } else if !exclude.is_empty() {
            let key = format!("~{}", exclude.into_iter().collect::<Vec<_>>().join(","));
            self.specific_hide.entry(key).or_default().insert(selector);
        }
    }

    fn finish_cosmetics(&mut self) {
        let exceptions = std::mem::take(&mut self.hide_exceptions);
        let disabled = |selector: &str| {
            exceptions
                .get(selector)
                .is_some_and(|domains| domains.contains(""))
        };

        let mut plain = Vec::new();
        let mut restricted: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for selector in std::mem::take(&mut self.generic_hide) {
            if disabled(&selector) {
                continue;
            }
            match exceptions.get(&selector) {
                Some(domains) => {
                    let key = format!("~{}", domains.iter().cloned().collect::<Vec<_>>().join(","));
                    restricted.entry(key).or_default().insert(selector);
                }
                None => plain.push(selector),
            }
        }
        for group in plain.chunks(SELECTORS_PER_RULE) {
            let trigger = Trigger {
                url_filter: ".*".into(),
                ..Default::default()
            };
            self.push(
                Segment::GenericHide,
                &trigger,
                &Action::Hide(&selector_list(group)),
            );
        }

        for (key, selectors) in std::mem::take(&mut self.specific_hide) {
            let (negated, list) = match key.strip_prefix('~') {
                Some(list) => (true, list),
                None => (false, key.as_str()),
            };
            let domains: BTreeSet<String> = list.split(',').map(str::to_string).collect();
            for selector in selectors {
                if disabled(&selector) {
                    continue;
                }
                let mut domains = domains.clone();
                let mut negated = negated;
                if let Some(excepted) = exceptions.get(&selector) {
                    if negated {
                        domains.extend(excepted.iter().cloned());
                    } else {
                        domains.retain(|domain| !excepted.contains(domain));
                        if domains.is_empty() {
                            continue;
                        }
                    }
                }
                if domains.is_empty() {
                    negated = false;
                }
                let key = if negated {
                    format!("~{}", domains.into_iter().collect::<Vec<_>>().join(","))
                } else {
                    domains.into_iter().collect::<Vec<_>>().join(",")
                };
                restricted.entry(key).or_default().insert(selector);
            }
        }

        for (key, selectors) in restricted {
            let mut trigger = Trigger {
                url_filter: ".*".into(),
                ..Default::default()
            };
            let (list, negated) = match key.strip_prefix('~') {
                Some(list) => (list, true),
                None => (key.as_str(), false),
            };
            let domains: Vec<String> = list
                .split(',')
                .filter(|d| !d.is_empty())
                .map(|d| format!("*{d}"))
                .collect();
            let segment = if negated {
                trigger.unless_domain = domains;
                Segment::GenericHide
            } else {
                trigger.if_domain = domains;
                Segment::SpecificHide
            };
            let selectors: Vec<_> = selectors.into_iter().collect();
            for group in selectors.chunks(SELECTORS_PER_RULE) {
                self.push(segment, &trigger, &Action::Hide(&selector_list(group)));
            }
        }
    }

    fn network(&mut self, line: &str) {
        let (exception, line) = match line.strip_prefix("@@") {
            Some(rest) => (true, rest),
            None => (false, line),
        };
        // Regular-expression filters use syntax WebKit's matcher does not support.
        let regex_body = line.starts_with('/') && line.len() > 1;
        let (pattern, options) = match line.rfind('$') {
            Some(i) if !(regex_body && line[..i].ends_with('/') && i + 1 == line.len()) => {
                (&line[..i], &line[i + 1..])
            }
            _ => (line, ""),
        };
        if pattern.len() > 1 && pattern.starts_with('/') && pattern.ends_with('/') {
            return;
        }

        let mut trigger = Trigger::default();
        let mut types: Vec<&'static str> = Vec::new();
        let mut excluded: Vec<&'static str> = Vec::new();
        let mut frames = None;
        let mut object = false;
        let mut important = false;
        let mut document = false;
        let mut elemhide = None;
        let mut all = false;
        for option in options.split(',').filter(|o| !o.is_empty()) {
            let (negated, name) = match option.strip_prefix('~') {
                Some(name) => (true, name),
                None => (false, option),
            };
            let (name, value) = name.split_once('=').unwrap_or((name, ""));
            match name {
                "third-party" | "3p" => {
                    trigger.load_type = Some(if negated {
                        "first-party"
                    } else {
                        "third-party"
                    })
                }
                "first-party" | "1p" => {
                    trigger.load_type = Some(if negated {
                        "third-party"
                    } else {
                        "first-party"
                    })
                }
                "domain" | "from" => {
                    let (include, exclude) = parse_domains(value, '|');
                    if !include.is_empty() {
                        trigger.if_domain = include.into_iter().map(|d| format!("*{d}")).collect();
                    } else if !exclude.is_empty() {
                        trigger.unless_domain =
                            exclude.into_iter().map(|d| format!("*{d}")).collect();
                    } else {
                        return;
                    }
                }
                "match-case" => trigger.case_sensitive = true,
                "important" => important = true,
                "all" => all = true,
                "document" | "doc" if !negated => document = true,
                "elemhide" | "ehide" if exception => elemhide = Some(false),
                "generichide" | "ghide" if exception => elemhide = Some(true),
                "subdocument" | "frame" => frames = Some(!negated),
                "object" | "object-subrequest" => object |= !negated,
                _ => {
                    let Some(mapped) = resource_type(name) else {
                        // csp, redirect, removeparam, rewrite, header, badfilter, ...
                        return;
                    };
                    if negated {
                        excluded.push(mapped);
                    } else {
                        types.push(mapped);
                    }
                }
            }
        }
        if object && types.is_empty() && frames.is_none() && !document && !all {
            // Browser plug-ins no longer exist, so an `$object`-only filter never matches.
            return;
        }

        let Some((filter, host_anchored)) = url_filter(pattern) else {
            return;
        };
        if filter == ".*" && trigger.if_domain.is_empty() && !(exception && document) {
            // A bare `$third-party` or `$script` would block most of the web.
            return;
        }
        trigger.url_filter = filter;

        if let Some(generic_only) = elemhide {
            trigger.resource_types = vec!["document"];
            let segment = if generic_only {
                Segment::GenericHideException
            } else {
                Segment::ElemHideException
            };
            self.push(segment, &trigger, &Action::Ignore);
            return;
        }

        if exception && document {
            // `@@||site^$document` disables filtering on pages from that site.
            if let Some(domain) = pattern_domain(pattern) {
                let site = Trigger {
                    url_filter: ".*".into(),
                    if_domain: vec![format!("*{domain}")],
                    ..Default::default()
                };
                self.push(Segment::Exception, &site, &Action::Ignore);
            }
            return;
        }

        let segment = match (exception, important) {
            (true, _) => Segment::Exception,
            (false, true) => Segment::Important,
            (false, false) => Segment::Block,
        };
        let action = if exception {
            Action::Ignore
        } else {
            Action::Block
        };

        if all {
            self.push(segment, &trigger, &action);
            return;
        }
        if document {
            types.push("document");
        }
        if types.is_empty() && frames != Some(true) {
            // No positive type option: ABP applies the filter to every subresource.
            if host_anchored && !exception && frames.is_none() && excluded.is_empty() {
                // Domain filters also cover pages and frames from that domain.
                self.push(segment, &trigger, &action);
                return;
            }
            types = SUBRESOURCE_TYPES
                .iter()
                .copied()
                .filter(|t| !excluded.contains(t))
                .collect();
            if frames.is_none() {
                frames = Some(true);
            }
        }
        if frames == Some(true) {
            let mut frame = trigger.clone();
            frame.resource_types = vec!["document"];
            frame.load_context = Some("child-frame");
            self.push(segment, &frame, &action);
        }
        types.sort_unstable();
        types.dedup();
        if !types.is_empty() {
            trigger.resource_types = types;
            self.push(segment, &trigger, &action);
        }
    }
}

fn resource_type(name: &str) -> Option<&'static str> {
    Some(match name {
        "script" => "script",
        "image" => "image",
        "stylesheet" | "css" => "style-sheet",
        "font" => "font",
        "media" => "media",
        "xmlhttprequest" | "xhr" | "websocket" | "other" | "webrtc" => "raw",
        "ping" | "beacon" => "ping",
        "popup" => "popup",
        _ => return None,
    })
}

/// Splits `domains##selector` or `domains#@#selector`; returns `None` for
/// network filters and for cosmetic syntax WebKit cannot apply.
fn split_cosmetic(line: &str) -> Option<(&str, &str, bool)> {
    let start = line.find('#')?;
    let rest = &line[start..];
    let (exception, skip) = if rest.starts_with("#@#") {
        (true, 3)
    } else if rest.starts_with("##") {
        (false, 2)
    } else if rest.starts_with("#?#")
        || rest.starts_with("#@?#")
        || rest.starts_with("#$#")
        || rest.starts_with("#@$#")
        || rest.starts_with("#%#")
        || rest.starts_with("#@%#")
    {
        return Some(("", "", false));
    } else {
        return None;
    };
    let domains = &line[..start];
    if domains.contains('/') || domains.contains('$') {
        // `#` inside a URL pattern, not a cosmetic separator.
        return None;
    }
    Some((domains, &rest[skip..], exception))
}

fn usable_selector(selector: &str) -> bool {
    !selector.is_empty()
        && selector.is_ascii()
        && !selector.starts_with('+')
        && !selector.starts_with('^')
        && !selector.contains('{')
        // WebKit rejects `:has()` nested inside `:has()`.
        && selector.matches(":has(").count() < 2
        && !EXTENDED_SELECTORS.iter().any(|s| selector.contains(s))
}

fn selector_list(selectors: &[String]) -> String {
    selectors
        .iter()
        .map(|s| fold_case(s))
        .collect::<Vec<_>>()
        .join(", ")
}

/// WebKit lowercases content-blocker selectors, so `.AdBox` would never match
/// `class="AdBox"`. Rewrites mixed-case classes, IDs and attribute values into
/// case-insensitive attribute selectors, which survive the lowercasing.
fn fold_case(selector: &str) -> String {
    if !selector.bytes().any(|b| b.is_ascii_uppercase()) {
        return selector.to_string();
    }
    let chars: Vec<char> = selector.chars().collect();
    let mut out = String::with_capacity(selector.len() + 16);
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '[' => {
                let mut j = i + 1;
                let mut quote = None;
                while j < chars.len() {
                    match (quote, chars[j]) {
                        (Some(_), '\\') => j += 1,
                        (Some(q), c) if c == q => quote = None,
                        (None, c @ ('"' | '\'')) => quote = Some(c),
                        (None, ']') => break,
                        _ => {}
                    }
                    j += 1;
                }
                let inner: String = chars[i + 1..j.min(chars.len())].iter().collect();
                let value = inner.split_once('=').map_or("", |(_, v)| v).trim_end();
                let flagged = value.len() > 2
                    && value[value.len() - 2..].eq_ignore_ascii_case(" i")
                    || value.len() > 2 && value[value.len() - 2..].eq_ignore_ascii_case(" s");
                out.push('[');
                out.push_str(&inner);
                if !flagged && value.bytes().any(|b| b.is_ascii_uppercase()) {
                    out.push_str(" i");
                }
                out.push(']');
                i = j + 1;
            }
            c @ ('.' | '#') => {
                let mut j = i + 1;
                while j < chars.len()
                    && (chars[j].is_ascii_alphanumeric() || chars[j] == '-' || chars[j] == '_')
                {
                    j += 1;
                }
                let name: String = chars[i + 1..j].iter().collect();
                let escaped = chars.get(j) == Some(&'\\');
                if escaped || !name.bytes().any(|b| b.is_ascii_uppercase()) {
                    out.push(c);
                    out.push_str(&name);
                } else if c == '.' {
                    let _ = write!(out, "[class~=\"{name}\" i]");
                } else {
                    let _ = write!(out, "[id=\"{name}\" i]");
                }
                i = j;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Parses `a.com,~b.com` (cosmetic) or `a.com|~b.com` (`$domain=`).
fn parse_domains(list: &str, separator: char) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut include = BTreeSet::new();
    let mut exclude = BTreeSet::new();
    for entry in list.split(separator).map(str::trim) {
        let (negated, domain) = match entry.strip_prefix('~') {
            Some(domain) => (true, domain),
            None => (false, entry),
        };
        let domain = domain.to_ascii_lowercase();
        if domain.is_empty()
            || !domain.is_ascii()
            || domain.ends_with(".*")
            || !domain
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        {
            continue;
        }
        if negated {
            exclude.insert(domain);
        } else {
            include.insert(domain);
        }
    }
    (include, exclude)
}

/// `*domain` entry for `if-domain`, from a hostname the person allowed.
fn domain_entry(host: &str) -> Option<String> {
    let (include, _) = parse_domains(host.trim_start_matches("www."), '|');
    include.into_iter().next().map(|d| format!("*{d}"))
}

/// The hostname of a `||host^` pattern, for site-wide exceptions.
fn pattern_domain(pattern: &str) -> Option<String> {
    let host = pattern.strip_prefix("||")?;
    let host = host.trim_end_matches(['^', '/', '|']);
    let (include, _) = parse_domains(host, '|');
    (include.len() == 1)
        .then(|| include.into_iter().next())
        .flatten()
}

/// Translates an ABP URL pattern into WebKit's restricted regular expressions.
/// Returns the filter and whether the pattern is anchored to a hostname.
fn url_filter(pattern: &str) -> Option<(String, bool)> {
    if !pattern.is_ascii() {
        return None;
    }
    let mut rest = pattern;
    let mut out = String::new();
    let mut host_anchored = false;
    let mut in_host = false;
    if let Some(r) = rest.strip_prefix("||") {
        out.push_str("^[^:]+://+([^/:]+\\.)?");
        rest = r;
        host_anchored = true;
        in_host = true;
    } else if let Some(r) = rest.strip_prefix('|') {
        out.push('^');
        rest = r;
        host_anchored = r.contains("://");
    }
    let (rest, end_anchor) = match rest.strip_suffix('|') {
        Some(r) => (r, true),
        None => (rest, false),
    };
    let rest = rest.trim_start_matches('*');
    let trimmed = rest.trim_end_matches('*');
    let trailing_star = trimmed.len() != rest.len();
    let rest = trimmed;
    if rest.is_empty() {
        return Some((".*".to_string(), false));
    }
    if !host_anchored && out.is_empty() && rest.trim_matches('^').len() < 3 {
        // Two-letter substrings would match almost every URL.
        return None;
    }

    let bytes = rest.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        let c = b as char;
        match c {
            '*' => {
                if !out.ends_with(".*") {
                    out.push_str(".*");
                }
                in_host = false;
            }
            '^' => {
                let last = i + 1 == bytes.len();
                if in_host {
                    out.push_str("[/:]");
                } else if last && !trailing_star && !end_anchor {
                    out.push_str("([^-a-z0-9_.%].*)?$");
                    return Some((out, host_anchored));
                } else {
                    out.push_str("[^-a-z0-9_.%]");
                }
                in_host = false;
            }
            '|' => return None,
            '/' | '?' | '=' | '&' | ':' => {
                if c == '?' {
                    out.push_str("\\?");
                } else {
                    out.push(c);
                }
                in_host = false;
            }
            '.' | '+' | '(' | ')' | '[' | ']' | '{' | '}' | '\\' | '$' => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_ascii_graphic() => out.push(c),
            _ => return None,
        }
    }
    if end_anchor {
        out.push('$');
    }
    Some((out, host_anchored))
}

fn push_json_str(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn push_json_list(out: &mut String, key: &str, values: &[impl AsRef<str>]) {
    if values.is_empty() {
        return;
    }
    let _ = write!(out, ",\"{key}\":[");
    for (i, value) in values.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        push_json_str(out, value.as_ref());
    }
    out.push(']');
}

fn rule_json(trigger: &Trigger, action: &Action) -> String {
    let mut out = String::from("{\"trigger\":{\"url-filter\":");
    push_json_str(&mut out, &trigger.url_filter);
    if trigger.case_sensitive {
        out.push_str(",\"url-filter-is-case-sensitive\":true");
    }
    push_json_list(&mut out, "resource-type", &trigger.resource_types);
    if let Some(load_type) = trigger.load_type {
        push_json_list(&mut out, "load-type", &[load_type]);
    }
    if let Some(context) = trigger.load_context {
        push_json_list(&mut out, "load-context", &[context]);
    }
    push_json_list(&mut out, "if-domain", &trigger.if_domain);
    push_json_list(&mut out, "unless-domain", &trigger.unless_domain);
    out.push_str("},\"action\":{\"type\":");
    match action {
        Action::Block => out.push_str("\"block\""),
        Action::Ignore => out.push_str("\"ignore-previous-rules\""),
        Action::Hide(selector) => {
            out.push_str("\"css-display-none\",\"selector\":");
            push_json_str(&mut out, selector);
        }
    }
    out.push_str("}}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(list: &str) -> String {
        convert(&[list], &[], true).chunks.join("")
    }

    #[test]
    fn domain_anchor() {
        assert_eq!(
            url_filter("||ads.example.com^").unwrap().0,
            "^[^:]+://+([^/:]+\\.)?ads\\.example\\.com[/:]"
        );
        assert_eq!(
            url_filter("|https://x.com/a|").unwrap().0,
            "^https://x\\.com/a$"
        );
        assert_eq!(
            url_filter("/banner/*/ad.js?").unwrap().0,
            "/banner/.*/ad\\.js\\?"
        );
        assert_eq!(url_filter("/ads^").unwrap().0, "/ads([^-a-z0-9_.%].*)?$");
        assert!(url_filter("ab").is_none());
        assert!(url_filter("/путь/").is_none());
    }

    #[test]
    fn network_options() {
        let json = rules("||tracker.net^$third-party");
        assert!(json.contains(r#""load-type":["third-party"]"#));
        assert!(!json.contains("resource-type"));

        let json = rules("/adframe.$subdocument,domain=a.com|b.com");
        assert!(json.contains(r#""load-context":["child-frame"]"#));
        assert!(json.contains(r#""if-domain":["*a.com","*b.com"]"#));

        let json = rules("-ad-banner.");
        assert!(json.contains(r#""resource-type":["font","image","#));
        assert!(!json.contains(r#""document","font""#));

        let json = rules("/pixel.$~script");
        assert!(
            json.contains(r#""resource-type":["font","image","media","ping","raw","style-sheet""#)
        );

        assert_eq!(rules("$third-party,script"), rules(""));

        assert_eq!(rules("||x.com^$csp=script-src 'none'"), rules(""));
        assert_eq!(rules("||x.com^$redirect=noop.js"), rules(""));
    }

    #[test]
    fn generic_hiding_optional() {
        let lean = convert(&["##.ad\nsite.org##.promo\n||ads.com^"], &[], false).chunks.join("");
        assert!(!lean.contains(".ad\""));
        assert!(lean.contains(".promo") && lean.contains("ads\\\\.com"));
    }

    #[test]
    fn ordering() {
        let json = rules("@@||good.com^$document\n||ads.com^$important\n||ads.com^\n##.ad");
        let hide = json.find("css-display-none").unwrap();
        let block = json
            .find("ads\\\\.com[/:]\"},\"action\":{\"type\":\"block\"")
            .unwrap();
        let allow = json.find("*good.com").unwrap();
        let important = json.rfind("ads\\\\.com").unwrap();
        assert!(hide < block && block < allow && allow < important);
    }

    #[test]
    fn cosmetic_exceptions() {
        let json = rules("##.ad\nexample.com#@#.ad\nsite.org##.promo\n##div:has-text(Ad)");
        assert!(json.contains(r#""unless-domain":["*example.com"]"#));
        assert!(json.contains(r#""if-domain":["*site.org"]}"#));
        assert!(!json.contains("has-text"));
        assert_eq!(rules("##.ad\n#@#.ad"), rules(""));
    }

    #[test]
    fn mixed_case_selectors() {
        assert_eq!(fold_case(".ad-box #top"), ".ad-box #top");
        assert_eq!(
            fold_case("div.AdBox > #TopAd"),
            r#"div[class~="AdBox" i] > [id="TopAd" i]"#
        );
        assert_eq!(
            fold_case(r#"a[href*="Sponsor"]:not(.OK)"#),
            r#"a[href*="Sponsor" i]:not([class~="OK" i])"#
        );
        assert_eq!(fold_case(r#"[data-Ad="x" i]"#), r#"[data-Ad="x" i]"#);
        assert_eq!(fold_case(r#"[title="A.B#C"]"#), r#"[title="A.B#C" i]"#);
    }

    #[test]
    fn allowlist_last() {
        let out = convert(&["||ads.com^"], &["www.news.com".into()], true);
        assert!(out.chunks[0].ends_with(
            r#"{"trigger":{"url-filter":".*","if-domain":["*news.com"]},"action":{"type":"ignore-previous-rules"}}]"#
        ));
        assert_eq!(out.rule_count, 1);
    }

    #[test]
    fn chunks_repeat_exceptions() {
        let mut list = String::from("@@||ok.com^$document\n");
        for i in 0..60_000 {
            let _ = writeln!(list, "||ad{i}.com^");
        }
        let out = convert(&[&list], &[], true);
        assert_eq!(out.chunks.len(), 2);
        assert!(out.chunks.iter().all(|c| c.contains("*ok.com")));
        assert_eq!(out.rule_count, 60_000);
    }
}
