use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_foundation::{MainThreadMarker, NSArray, NSError, NSString};
use objc2_web_kit::{WKContentRuleList, WKContentRuleListStore};
use wry::{WebView, WebViewExtMacOS};

use crate::filters;
use crate::log;
use crate::msg::{Msg, MsgSender};

pub struct FilterList {
    pub name: &'static str,
    id: &'static str,
    url: &'static str,
    default_on: bool,
}

pub const LISTS: [FilterList; 3] = [
    FilterList {
        name: "Ads — EasyList",
        id: "easylist",
        url: "https://easylist.to/easylist/easylist.txt",
        default_on: true,
    },
    FilterList {
        name: "Trackers — EasyPrivacy",
        id: "easyprivacy",
        url: "https://easylist.to/easylist/easyprivacy.txt",
        default_on: true,
    },
    FilterList {
        name: "Cookie Notices — Fanboy",
        id: "cookies",
        url: "https://secure.fanboy.co.nz/fanboy-cookiemonster.txt",
        default_on: false,
    },
];

const BUILTIN: &str = include_str!("builtin_filters.txt");
const IDENTIFIER_PREFIX: &str = "filters-";
const LEGACY_IDENTIFIERS: &[&str] = &["browser-protection-v2"];
/// EasyList asks clients to refresh every four days.
const LIST_MAX_AGE: Duration = Duration::from_secs(4 * 24 * 60 * 60);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const RETRY_INTERVAL: Duration = Duration::from_secs(30 * 60);
const CUSTOM_TEMPLATE: &str = "\
! Custom filters, one per line, in Adblock Plus syntax. Changes apply when you save.
! Examples:
!   ||ads.example.com^           block a domain
!   example.com##.promo-banner   hide an element on one site
!   @@||example.com^$document    turn filtering off for a site
";

pub fn data_dir() -> PathBuf {
    let home = std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into());
    PathBuf::from(home).join("Library/Application Support/Browser")
}

fn filters_dir() -> PathBuf {
    data_dir().join("Filters")
}

fn list_path(list: &FilterList) -> PathBuf {
    filters_dir().join(format!("{}.txt", list.id))
}

fn custom_path() -> PathBuf {
    filters_dir().join("custom.txt")
}

fn modified(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

struct Settings {
    enabled: bool,
    lists: Vec<bool>,
    allowed: BTreeSet<String>,
    /// Element hiding on every site; costs memory in each page.
    generic_hiding: bool,
    /// WebKit Lockdown Mode: no JavaScript JIT, WebGL, or other complex
    /// features, so pages use much less memory but run scripts slower.
    lockdown: bool,
}

impl Settings {
    fn path() -> PathBuf {
        data_dir().join("protection.txt")
    }

    fn load() -> Self {
        let mut settings = Self {
            enabled: true,
            lists: LISTS.iter().map(|list| list.default_on).collect(),
            allowed: BTreeSet::new(),
            generic_hiding: false,
            lockdown: true,
        };
        let Ok(text) = std::fs::read_to_string(Self::path()) else {
            return settings;
        };
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("enabled=") {
                settings.enabled = value != "0";
            } else if let Some(value) = line.strip_prefix("generic=") {
                settings.generic_hiding = value != "0";
            } else if let Some(value) = line.strip_prefix("lockdown=") {
                settings.lockdown = value != "0";
            } else if let Some(rest) = line.strip_prefix("list:")
                && let Some((id, value)) = rest.split_once('=')
                && let Some(index) = LISTS.iter().position(|list| list.id == id)
            {
                settings.lists[index] = value != "0";
            } else if let Some(site) = line.strip_prefix("allow:") {
                settings.allowed.insert(site.to_string());
            }
        }
        settings
    }

    fn save(&self) {
        let mut text = format!(
            "enabled={}\ngeneric={}\nlockdown={}\n",
            u8::from(self.enabled),
            u8::from(self.generic_hiding),
            u8::from(self.lockdown)
        );
        for (list, on) in LISTS.iter().zip(&self.lists) {
            text.push_str(&format!("list:{}={}\n", list.id, u8::from(*on)));
        }
        for site in &self.allowed {
            text.push_str(&format!("allow:{site}\n"));
        }
        let result =
            std::fs::create_dir_all(data_dir()).and_then(|()| std::fs::write(Self::path(), text));
        if let Err(e) = result {
            log::log(&format!("saving protection settings failed: {e}"));
        }
    }
}

struct Pending {
    generation: u64,
    lists: Vec<Option<Retained<WKContentRuleList>>>,
    identifiers: Vec<String>,
    remaining: usize,
    rule_count: usize,
    failed: bool,
    builtin_only: bool,
    /// Fingerprint of the inputs, saved once these lists compile.
    key: u64,
    /// Looked up from a previous run without converting the filter text.
    cached: bool,
}

/// Rule lists compiled by an earlier run, and the inputs they came from.
struct Cached {
    key: u64,
    rule_count: usize,
    identifiers: Vec<String>,
}

fn cache_path() -> PathBuf {
    data_dir().join("rules.txt")
}

impl Cached {
    fn load() -> Option<Self> {
        let text = std::fs::read_to_string(cache_path()).ok()?;
        let mut lines = text.lines();
        let key = u64::from_str_radix(lines.next()?, 16).ok()?;
        let rule_count = lines.next()?.parse().ok()?;
        let identifiers: Vec<String> = lines.map(str::to_string).collect();
        (!identifiers.is_empty()).then_some(Self { key, rule_count, identifiers })
    }

    fn save(key: u64, rule_count: usize, identifiers: &[String]) {
        let text = format!("{key:016x}\n{rule_count}\n{}\n", identifiers.join("\n"));
        if let Err(e) = std::fs::write(cache_path(), text) {
            log::log(&format!("saving rule cache failed: {e}"));
        }
    }

    fn forget() {
        let _ = std::fs::remove_file(cache_path());
    }
}

/// Changes whenever the filter files, the allowlist, or this program change,
/// so converting the lists again could give a different result.
fn input_key(paths: &[PathBuf], allowed: &[String], generic_hiding: bool) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let stamp = |path: &PathBuf| {
        std::fs::metadata(path)
            .ok()
            .map(|m| (m.len(), m.modified().ok()))
    };
    let mut hasher = DefaultHasher::new();
    BUILTIN.hash(&mut hasher);
    std::env::current_exe().ok().map(|exe| stamp(&exe)).hash(&mut hasher);
    for path in paths {
        path.hash(&mut hasher);
        stamp(path).hash(&mut hasher);
    }
    allowed.hash(&mut hasher);
    generic_hiding.hash(&mut hasher);
    hasher.finish()
}

/// What the sidebar needs to describe protection for the current page.
#[derive(Clone, PartialEq)]
pub struct Status {
    pub enabled: bool,
    pub site: Option<String>,
    pub site_allowed: bool,
    pub rule_count: usize,
    pub busy: bool,
    pub updated: Option<SystemTime>,
    pub error: Option<String>,
    pub lists: Vec<bool>,
    pub generic_hiding: bool,
    pub lockdown: bool,
}

pub struct Blocker {
    settings: Settings,
    store: Option<Retained<WKContentRuleListStore>>,
    active: Vec<Retained<WKContentRuleList>>,
    active_ids: Vec<String>,
    pending: Option<Pending>,
    generation: u64,
    rule_count: usize,
    started: bool,
    updating: bool,
    next_update: Instant,
    custom_modified: Option<SystemTime>,
    compile_failed: bool,
    error: Option<String>,
    /// `input_key` of the latest rebuild.
    rebuild_key: u64,
}

impl Blocker {
    pub fn new() -> Self {
        let mtm = MainThreadMarker::new().expect("content rules need the main thread");
        Self {
            settings: Settings::load(),
            store: unsafe { WKContentRuleListStore::defaultStore(mtm) },
            active: Vec::new(),
            active_ids: Vec::new(),
            pending: None,
            generation: 0,
            rule_count: 0,
            started: false,
            updating: false,
            next_update: Instant::now(),
            custom_modified: modified(&custom_path()),
            compile_failed: false,
            error: None,
            rebuild_key: 0,
        }
    }

    /// Converts the enabled lists on a worker thread; `on_converted` compiles them.
    pub fn rebuild(&mut self, tx: &MsgSender) {
        self.start_rebuild(tx, false);
    }

    fn start_rebuild(&mut self, tx: &MsgSender, builtin_only: bool) {
        self.generation += 1;
        let generation = self.generation;
        let mut paths: Vec<PathBuf> = if builtin_only {
            Vec::new()
        } else {
            LISTS
                .iter()
                .zip(&self.settings.lists)
                .filter(|(_, on)| **on)
                .map(|(list, _)| list_path(list))
                .collect()
        };
        if !builtin_only {
            paths.push(custom_path());
        }
        let allowed: Vec<String> = self.settings.allowed.iter().cloned().collect();
        let generic_hiding = self.settings.generic_hiding;
        let key = input_key(&paths, &allowed, generic_hiding);
        self.rebuild_key = key;
        // Nothing changed since the last compile: reuse WebKit's compiled lists
        // without reading or converting megabytes of filter text.
        if !builtin_only
            && let Some(store) = self.store.clone()
            && let Some(cached) = Cached::load().filter(|c| c.key == key)
        {
            let count = cached.identifiers.len();
            self.pending = Some(Pending {
                generation,
                lists: (0..count).map(|_| None).collect(),
                identifiers: cached.identifiers.clone(),
                remaining: count,
                rule_count: cached.rule_count,
                failed: false,
                builtin_only,
                key,
                cached: true,
            });
            for (index, identifier) in cached.identifiers.into_iter().enumerate() {
                load_rule_list(&store, identifier, None, generation, index, tx.clone());
            }
            return;
        }
        let tx = tx.clone();
        std::thread::spawn(move || {
            let texts: Vec<String> = paths
                .iter()
                .filter_map(|path| std::fs::read_to_string(path).ok())
                .collect();
            let mut sources: Vec<&str> = vec![BUILTIN];
            sources.extend(texts.iter().map(String::as_str));
            let converted = filters::convert(&sources, &allowed, generic_hiding);
            let _ = tx.send(Msg::FiltersConverted {
                generation,
                converted,
                builtin_only,
            });
        });
    }

    pub fn on_converted(
        &mut self,
        generation: u64,
        converted: filters::Converted,
        builtin_only: bool,
        tx: &MsgSender,
    ) {
        if generation != self.generation {
            return;
        }
        let Some(store) = self.store.clone() else {
            self.compile_failed = true;
            self.started = true;
            return;
        };
        let count = converted.chunks.len();
        let identifiers: Vec<String> = (0..count)
            .map(|i| format!("{IDENTIFIER_PREFIX}{:016x}-{i}", converted.fingerprint))
            .collect();
        log::log(&format!(
            "filters: {} rules in {count} lists",
            converted.rule_count
        ));
        self.pending = Some(Pending {
            generation,
            lists: (0..count).map(|_| None).collect(),
            identifiers: identifiers.clone(),
            remaining: count,
            rule_count: converted.rule_count,
            failed: false,
            builtin_only,
            key: self.rebuild_key,
            cached: false,
        });
        for (index, (json, identifier)) in converted.chunks.into_iter().zip(identifiers).enumerate()
        {
            load_rule_list(&store, identifier, Some(json), generation, index, tx.clone());
        }
    }

    /// Returns true when a new set of rule lists became active.
    pub fn on_rule_list(
        &mut self,
        generation: u64,
        index: usize,
        pointer: Option<usize>,
        error: Option<String>,
        tx: &MsgSender,
    ) -> bool {
        // Take ownership first so a stale list is released.
        let list = pointer.and_then(|p| unsafe { Retained::from_raw(p as *mut WKContentRuleList) });
        let Some(pending) = self.pending.as_mut().filter(|p| p.generation == generation) else {
            return false;
        };
        if let Some(error) = error {
            log::log(&format!("rule list {index} failed: {error}"));
            pending.failed = true;
        }
        if let Some(slot) = pending.lists.get_mut(index) {
            *slot = list;
        }
        pending.remaining = pending.remaining.saturating_sub(1);
        if pending.remaining > 0 {
            return false;
        }
        let pending = self.pending.take().expect("pending compile");
        if pending.cached && (pending.failed || pending.lists.iter().any(Option::is_none)) {
            log::log("cached rule lists missing; converting filters again");
            Cached::forget();
            self.start_rebuild(tx, false);
            return false;
        }
        if pending.failed || pending.lists.iter().any(Option::is_none) {
            self.compile_failed = true;
            if !pending.builtin_only {
                // Keep protecting with the built-in rules rather than nothing.
                self.start_rebuild(tx, true);
                return false;
            }
            let first_start = !self.started;
            self.started = true;
            return first_start;
        }
        if !pending.builtin_only {
            self.compile_failed = false;
        }
        if !pending.builtin_only && !pending.cached {
            Cached::save(pending.key, pending.rule_count, &pending.identifiers);
        }
        self.active = pending.lists.into_iter().flatten().collect();
        self.active_ids = pending.identifiers;
        self.rule_count = pending.rule_count;
        self.started = true;
        self.remove_stale_lists();
        true
    }

    fn remove_stale_lists(&self) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let keep = self.active_ids.clone();
        let remover = store.clone();
        let callback = RcBlock::new(move |identifiers: *mut NSArray<NSString>| {
            let Some(identifiers) = (unsafe { identifiers.as_ref() }) else {
                return;
            };
            for identifier in identifiers.iter() {
                let name = identifier.to_string();
                let ours = name.starts_with(IDENTIFIER_PREFIX)
                    || LEGACY_IDENTIFIERS.contains(&name.as_str());
                if ours && !keep.contains(&name) {
                    let done = RcBlock::new(|_: *mut NSError| {});
                    unsafe {
                        remover.removeContentRuleListForIdentifier_completionHandler(
                            Some(&identifier),
                            Some(&done),
                        );
                    }
                }
            }
        });
        unsafe { store.getAvailableContentRuleListIdentifiers(Some(&callback)) };
    }

    /// Lets pages load without waiting any longer for the first compile.
    pub fn force_start(&mut self) {
        if !self.started {
            log::log("filters still compiling; opening pages without them");
            self.started = true;
        }
    }

    pub fn started(&self) -> bool {
        self.started
    }

    pub fn rules(&self) -> &[Retained<WKContentRuleList>] {
        if self.settings.enabled {
            &self.active
        } else {
            &[]
        }
    }

    pub fn apply(&self, view: &WebView) {
        let controller = view.manager();
        unsafe {
            controller.removeAllContentRuleLists();
            for list in self.rules() {
                controller.addContentRuleList(list);
            }
        }
    }

    pub fn apply_all<'a>(&self, views: impl IntoIterator<Item = &'a WebView>) {
        for view in views {
            self.apply(view);
        }
    }

    /// Takes the rules off one view immediately, before a recompile lands.
    pub fn detach(&self, view: &WebView) {
        unsafe { view.manager().removeAllContentRuleLists() };
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.settings.enabled = enabled;
        self.settings.save();
    }

    pub fn enabled(&self) -> bool {
        self.settings.enabled
    }

    /// Returns whether filtering is now off for `site`.
    pub fn toggle_site(&mut self, site: &str) -> bool {
        let allowed = if self.settings.allowed.remove(site) {
            false
        } else {
            self.settings.allowed.insert(site.to_string());
            true
        };
        self.settings.save();
        allowed
    }

    pub fn toggle_list(&mut self, index: usize, tx: &MsgSender) {
        let Some(on) = self.settings.lists.get_mut(index) else {
            return;
        };
        *on = !*on;
        let needs_download = *on && modified(&list_path(&LISTS[index])).is_none();
        self.settings.save();
        self.rebuild(tx);
        if needs_download {
            self.update(tx, false);
        }
    }

    pub fn toggle_generic_hiding(&mut self, tx: &MsgSender) {
        self.settings.generic_hiding = !self.settings.generic_hiding;
        self.settings.save();
        self.rebuild(tx);
    }

    pub fn lockdown(&self) -> bool {
        self.settings.lockdown
    }

    pub fn toggle_lockdown(&mut self) {
        self.settings.lockdown = !self.settings.lockdown;
        self.settings.save();
    }

    pub fn status(&self, site: Option<String>) -> Status {
        let site_allowed = site
            .as_ref()
            .is_some_and(|site| self.settings.allowed.contains(site));
        let updated = LISTS
            .iter()
            .zip(&self.settings.lists)
            .filter(|(_, on)| **on)
            .filter_map(|(list, _)| modified(&list_path(list)))
            .max();
        Status {
            enabled: self.settings.enabled,
            site,
            site_allowed,
            rule_count: self.rule_count,
            busy: self.pending.is_some() || self.updating || !self.started,
            updated,
            error: if self.compile_failed {
                Some("Filter lists couldn't be compiled".into())
            } else {
                self.error.clone()
            },
            lists: self.settings.lists.clone(),
            generic_hiding: self.settings.generic_hiding,
            lockdown: self.settings.lockdown,
        }
    }

    /// Periodic work from the event loop: list refreshes and custom-filter edits.
    pub fn tick(&mut self, tx: &MsgSender) {
        if !self.started {
            return;
        }
        let custom = modified(&custom_path());
        if custom != self.custom_modified {
            self.custom_modified = custom;
            log::log("custom filters changed");
            self.rebuild(tx);
        }
        if !self.updating && Instant::now() >= self.next_update {
            self.update(tx, false);
        }
    }

    /// Downloads enabled lists that are missing or stale (or all, when forced).
    pub fn update(&mut self, tx: &MsgSender, force: bool) {
        if self.updating {
            return;
        }
        let jobs: Vec<(&'static str, &'static str, PathBuf)> = LISTS
            .iter()
            .zip(&self.settings.lists)
            .filter(|(_, on)| **on)
            .map(|(list, _)| (list.name, list.url, list_path(list)))
            .filter(|(_, _, path)| force || is_stale(path))
            .collect();
        if jobs.is_empty() {
            self.next_update = Instant::now() + CHECK_INTERVAL;
            return;
        }
        self.updating = true;
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut changed = false;
            let mut failed = Vec::new();
            for (name, url, path) in jobs {
                match download(url, &path) {
                    Ok(different) => changed |= different,
                    Err(e) => {
                        log::log(&format!("download {url} failed: {e}"));
                        failed.push(name.to_string());
                    }
                }
            }
            let _ = tx.send(Msg::FiltersDownloaded { changed, failed });
        });
    }

    pub fn on_downloaded(&mut self, changed: bool, failed: Vec<String>, tx: &MsgSender) {
        self.updating = false;
        let retry = if failed.is_empty() {
            CHECK_INTERVAL
        } else {
            RETRY_INTERVAL
        };
        self.next_update = Instant::now() + retry;
        self.error = (!failed.is_empty()).then(|| format!("Couldn't update {}", failed.join(", ")));
        if changed {
            self.rebuild(tx);
        }
    }

    pub fn custom_filters_file(&self) -> PathBuf {
        let path = custom_path();
        if !path.exists() {
            let result = std::fs::create_dir_all(filters_dir())
                .and_then(|()| std::fs::write(&path, CUSTOM_TEMPLATE));
            if let Err(e) = result {
                log::log(&format!("creating custom filters failed: {e}"));
            }
        }
        path
    }
}

fn load_rule_list(
    store: &Retained<WKContentRuleListStore>,
    identifier: String,
    json: Option<String>,
    generation: u64,
    index: usize,
    tx: MsgSender,
) {
    let compiler = store.clone();
    let name = NSString::from_str(&identifier);
    let lookup = RcBlock::new(move |list: *mut WKContentRuleList, _: *mut NSError| {
        if let Some(list) = unsafe { Retained::retain(list) } {
            // WKContentRuleList is Sendable. Carry the retained pointer through
            // the event loop, then restore ownership on its main thread.
            let pointer = Some(Retained::into_raw(list) as usize);
            let _ = tx.send(Msg::RuleListReady {
                generation,
                index,
                pointer,
                error: None,
            });
            return;
        }
        let Some(json) = json.as_deref() else {
            let _ = tx.send(Msg::RuleListReady {
                generation,
                index,
                pointer: None,
                error: Some(format!("{identifier} is not compiled")),
            });
            return;
        };
        log::log(&format!("compiling rule list {identifier}"));
        let tx = tx.clone();
        let compiled = RcBlock::new(move |list: *mut WKContentRuleList, error: *mut NSError| {
            let pointer = unsafe { Retained::retain(list) }.map(|l| Retained::into_raw(l) as usize);
            let error = pointer.is_none().then(|| {
                unsafe { error.as_ref() }
                    .map(|e| e.localizedDescription().to_string())
                    .unwrap_or_else(|| "unknown error".into())
            });
            let _ = tx.send(Msg::RuleListReady {
                generation,
                index,
                pointer,
                error,
            });
        });
        unsafe {
            compiler.compileContentRuleListForIdentifier_encodedContentRuleList_completionHandler(
                Some(&NSString::from_str(&identifier)),
                Some(&NSString::from_str(json)),
                Some(&compiled),
            );
        }
    });
    unsafe {
        store.lookUpContentRuleListForIdentifier_completionHandler(Some(&name), Some(&lookup))
    };
}

fn is_stale(path: &PathBuf) -> bool {
    modified(path)
        .and_then(|time| time.elapsed().ok())
        .is_none_or(|age| age > LIST_MAX_AGE)
}

/// Fetches a list with the system `curl`; returns whether its content changed.
fn download(url: &str, path: &PathBuf) -> Result<bool, String> {
    std::fs::create_dir_all(filters_dir()).map_err(|e| e.to_string())?;
    let partial = path.with_extension("part");
    let output = Command::new("/usr/bin/curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--compressed",
        ])
        .args(["--max-time", "120", "--output"])
        .arg(&partial)
        .arg(url)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&partial);
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let text = std::fs::read_to_string(&partial).map_err(|e| e.to_string())?;
    let looks_valid = text.len() > 1_000
        && text
            .lines()
            .take(10)
            .any(|line| line.starts_with("[Adblock") || line.starts_with("! Title"));
    if !looks_valid {
        let _ = std::fs::remove_file(&partial);
        return Err("response is not a filter list".into());
    }
    let changed = std::fs::read_to_string(path).map_or(true, |old| old != text);
    std::fs::rename(&partial, path).map_err(|e| e.to_string())?;
    Ok(changed)
}
