mod blocker;
mod chrome;
mod favicon;
mod filters;
mod idle;
mod log;
mod memory;
mod msg;
mod page;
mod places;
mod session;
mod start_page;
mod tabs;
mod url;

use std::collections::{HashMap, HashSet};
use std::panic::{self, AssertUnwindSafe};
use std::time::{Duration, Instant};

use blocker::Blocker;
use chrome::Chrome;
use idle::IdleSetting;
use msg::{Msg, MsgSender};
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use page::Page;
use places::{Bookmarks, History};
use tabs::TabManager;
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    platform::macos::WindowBuilderExtMacOS,
    window::WindowBuilder,
};
use wry::WebContext;

/// How the first window waits for filter lists before loading pages anyway.
const STARTUP_RULES_WAIT: Duration = Duration::from_secs(3);
const HOUSEKEEPING: Duration = Duration::from_secs(2);
/// Session writes wait for a quiet moment so page loads never touch the disk.
const SESSION_SAVE_DELAY: Duration = Duration::from_secs(1);
const IDLE_CHECK_EVERY: Duration = Duration::from_secs(30);

struct Browser {
    chrome: Chrome,
    blocker: Blocker,
    tabs: TabManager,
    /// Web views for tabs that have been shown; restored tabs load on first view.
    pages: HashMap<u32, Page>,
    ctx: WebContext,
    tx: MsgSender,
    pending_urls: Vec<String>,
    /// Tab to reload once recompiled rules land (after re-enabling a site).
    reload_after_rules: Option<u32>,
    save_at: Option<Instant>,
    history: History,
    bookmarks: Bookmarks,
    idle_setting: IdleSetting,
    /// The address last written to history for each tab.
    recorded: HashMap<u32, String>,
    /// When each tab was last in front, for unloading idle ones.
    last_seen: HashMap<u32, Instant>,
    /// Tabs with an unload check in flight.
    idle_checks: HashSet<u32>,
    next_idle_check: Instant,
}

fn main() -> wry::Result<()> {
    log::install_panic_hook();
    let _ = std::fs::remove_file(log::PATH);
    log::log("minibrowser start");

    let event_loop = EventLoopBuilder::<Msg>::with_user_event().build();
    let window = WindowBuilder::new()
        .with_title("Browser")
        .with_inner_size(LogicalSize::new(1200.0, 800.0))
        .with_min_inner_size(LogicalSize::new(640.0, 420.0))
        .with_titlebar_transparent(true)
        .with_fullsize_content_view(true)
        .with_title_hidden(true)
        .build(&event_loop)
        .unwrap();

    let tx = MsgSender::new(event_loop.create_proxy());
    let mut browser = Browser {
        chrome: Chrome::build(&window, tx.clone()),
        blocker: Blocker::new(),
        tabs: session::load(),
        pages: HashMap::new(),
        ctx: WebContext::new(None),
        tx: tx.clone(),
        pending_urls: Vec::new(),
        reload_after_rules: None,
        save_at: None,
        history: History::load(),
        bookmarks: Bookmarks::load(),
        idle_setting: IdleSetting::load(),
        recorded: HashMap::new(),
        last_seen: HashMap::new(),
        idle_checks: HashSet::new(),
        next_idle_check: Instant::now() + IDLE_CHECK_EVERY,
    };
    memory::watch_pressure(tx.clone());
    browser.blocker.rebuild(&tx);
    if session::sidebar_hidden() {
        browser.chrome.set_sidebar_hidden(true, false);
    }
    browser.chrome.set_bookmark_menu(&browser.bookmarks);
    browser.chrome.set_history_menu(&browser.history);
    browser
        .chrome
        .set_idle_minutes(browser.idle_setting.minutes);
    browser.refresh();
    browser.sync_protection();

    let mut next_housekeeping = Instant::now();
    let startup_deadline = Instant::now() + STARTUP_RULES_WAIT;

    event_loop.run(move |event, _, control_flow| {
        // Keep the tao window alive for the life of the loop.
        let _ = &window;
        let now = Instant::now();
        if !browser.blocker.started() && now >= startup_deadline {
            browser.blocker.force_start();
            browser.start_content();
        }
        if now >= next_housekeeping {
            let usage = memory::usage();
            browser.chrome.set_memory_usage(usage);
            browser.blocker.tick(&tx);
            browser.sync_protection();
            if browser.bookmarks.reload_if_edited() {
                browser.chrome.set_bookmark_menu(&browser.bookmarks);
                browser.sync_bookmark();
            }
            if now >= browser.next_idle_check {
                browser.next_idle_check = now + IDLE_CHECK_EVERY;
                browser.unload_idle_tabs();
            }
            next_housekeeping = now + HOUSEKEEPING;
        }
        if browser.save_at.is_some_and(|at| now >= at) {
            browser.save_all();
        }
        let wake = browser
            .save_at
            .map_or(next_housekeeping, |at| at.min(next_housekeeping));
        *control_flow = ControlFlow::WaitUntil(wake);

        match event {
            Event::UserEvent(msg) => {
                #[cfg(debug_assertions)]
                if !matches!(msg, Msg::PageChanged(_) | Msg::Hover(..)) {
                    log::log(&format!("handle {}", msg.name()));
                }
                if panic::catch_unwind(AssertUnwindSafe(|| browser.handle(msg))).is_err() {
                    log::log("action panicked, caught");
                    browser.refresh();
                }
                browser.sync_protection();
            }
            Event::Opened { urls } => {
                for url in urls {
                    let address = url.to_string();
                    if url::guard(&address).is_some() {
                        if browser.blocker.started() {
                            let _ = tx.send(Msg::New(Some(address)));
                        } else {
                            browser.pending_urls.push(address);
                        }
                    }
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                browser.save_all();
                *control_flow = ControlFlow::Exit;
            }
            Event::LoopDestroyed => browser.save_all(),
            _ => {}
        }
    });
}

impl Browser {
    fn active_page(&self) -> Option<&Page> {
        self.pages.get(&self.tabs.active)
    }

    fn refresh(&mut self) {
        self.chrome.refresh(&self.tabs);
        self.sync_bookmark();
    }

    fn sync_bookmark(&mut self) {
        let state = self
            .tabs
            .active_tab()
            .filter(|tab| tab.url.starts_with("https://") || tab.url.starts_with("http://"))
            .map(|tab| self.bookmarks.contains(&tab.url));
        self.chrome.set_bookmarked(state);
    }

    fn save_all(&mut self) {
        self.save_at = None;
        session::save(&self.tabs);
        if self.history.dirty {
            self.history.save();
            self.chrome.set_history_menu(&self.history);
        }
        self.bookmarks.save();
    }

    /// Frees web views of background tabs after the chosen inactive interval.
    /// Pages playing media or using the camera or microphone are kept.
    fn unload_idle_tabs(&mut self) {
        let Some(timeout) = self.idle_setting.duration() else {
            return;
        };
        let now = Instant::now();
        let background: Vec<(u32, Instant)> = self
            .pages
            .keys()
            .filter(|id| **id != self.tabs.active)
            .map(|id| (*id, self.last_seen.get(id).copied().unwrap_or(now)))
            .collect();
        for (id, seen) in background {
            let idle = now.duration_since(seen) >= timeout;
            let loading = self.tabs.get(id).is_some_and(|t| t.loading);
            if idle
                && !loading
                && self.idle_checks.insert(id)
                && let Some(page) = self.pages.get(&id)
            {
                page.check_idle(&self.tx);
            }
        }
    }

    /// Frees every background tab that is not playing media or capturing.
    fn unload_background_tabs(&mut self) {
        let ids: Vec<u32> = self
            .pages
            .keys()
            .copied()
            .filter(|id| *id != self.tabs.active)
            .collect();
        for id in ids {
            if self.idle_checks.insert(id)
                && let Some(page) = self.pages.get(&id)
            {
                page.check_idle(&self.tx);
            }
        }
    }

    fn session_changed(&mut self) {
        if self.save_at.is_none() {
            self.save_at = Some(Instant::now() + SESSION_SAVE_DELAY);
        }
    }

    /// Creates the web view for a tab if it has none yet.
    fn ensure_page(&mut self, id: u32) {
        if self.pages.contains_key(&id) {
            return;
        }
        let Some(tab) = self.tabs.get(id) else {
            return;
        };
        let (url, zoom) = (tab.url.clone(), tab.zoom);
        let start_page = if url == url::NEW_TAB {
            start_page::render(&self.bookmarks, &self.history)
        } else {
            String::new()
        };
        match Page::new(
            self.chrome.pages(),
            &mut self.ctx,
            id,
            &url,
            &start_page,
            self.tx.clone(),
            self.blocker.rules(),
            self.blocker.lockdown(),
        ) {
            Ok(page) => {
                if zoom != 1.0 {
                    page.set_zoom(zoom);
                }
                self.pages.insert(id, page);
                self.last_seen.insert(id, Instant::now());
                if let Some(tab) = self.tabs.get_mut(id) {
                    tab.asleep = false;
                }
            }
            Err(e) => log::log(&format!("web view for tab {id} failed: {e:?}")),
        }
    }

    /// Selects a tab: shows its page (loading it if needed) and updates the sidebar.
    fn activate(&mut self, id: u32) {
        self.activate_from(self.tabs.active, id);
    }

    /// Like [`Self::activate`], for when the tab list already selected `id`.
    fn activate_from(&mut self, previous: u32, id: u32) {
        if !self.tabs.has(id) {
            log::log(&format!("switch to unknown tab {id}, ignored"));
            return;
        }
        self.tabs.switch(id);
        if self.blocker.started() {
            self.ensure_page(id);
        }
        if previous != id {
            if let Some(page) = self.pages.get(&previous) {
                page.show(false);
            }
            self.last_seen.insert(previous, Instant::now());
            self.chrome.reset_progress();
            self.chrome.set_hover("");
            if self.chrome.find_open() {
                self.chrome.hide_find();
            }
        }
        self.present_active();
        self.refresh();
        self.chrome.scroll_to_active_tab(&self.tabs);
        self.session_changed();
    }

    /// Shows the active page and focuses it, or the address field for a new tab.
    fn present_active(&mut self) {
        let new_tab = self
            .tabs
            .active_tab()
            .is_some_and(|tab| tab.url == url::NEW_TAB);
        if let Some(page) = self.active_page() {
            page.show(true);
            page.focus();
        }
        if new_tab {
            self.chrome.reset_address(&self.tabs);
            self.chrome.focus_url();
        }
        self.sync_navigation();
    }

    fn close(&mut self, id: u32) {
        if !self.tabs.has(id) {
            return;
        }
        let was_active = self.tabs.active == id;
        self.pages.remove(&id);
        self.recorded.remove(&id);
        self.last_seen.remove(&id);
        self.idle_checks.remove(&id);
        self.tabs.close_tab(id);
        if was_active && self.tabs.active != 0 {
            self.activate_from(id, self.tabs.active);
        } else {
            if self.tabs.active == 0 {
                self.chrome.reset_progress();
                self.chrome.set_hover("");
                self.chrome.hide_find();
                self.sync_navigation();
            }
            self.refresh();
            self.session_changed();
        }
    }

    fn start_content(&mut self) {
        let active = self.tabs.active;
        if active != 0 {
            self.ensure_page(active);
            log::log("first content built");
        }
        self.present_active();
        for address in self.pending_urls.drain(..) {
            let _ = self.tx.send(Msg::New(Some(address)));
        }
    }

    fn sync_protection(&mut self) {
        let site = self.tabs.active_tab().and_then(|tab| url::site(&tab.url));
        self.chrome.set_protection(self.blocker.status(site));
    }

    fn sync_navigation(&mut self) {
        match self.active_page().map(Page::state) {
            Some(state) => self.chrome.set_navigation(
                state.can_back,
                state.can_forward,
                state.loading,
                state.progress,
            ),
            None => self.chrome.set_navigation(false, false, false, 0.0),
        }
    }

    /// Mirrors a web view's address, title, and loading state into its tab.
    fn page_changed(&mut self, id: u32) {
        let Some(state) = self.pages.get(&id).map(Page::state) else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(id) else {
            return;
        };
        let mut changed = false;
        if !state.url.is_empty() && tab.url != state.url {
            tab.url = state.url;
            changed = true;
        }
        let title = state.title.split_whitespace().collect::<Vec<_>>().join(" ");
        let title = if title.is_empty() {
            url::display_title(&tab.url)
        } else {
            title
        };
        if tab.title != title {
            tab.title = title;
            changed = true;
        }
        tab.loading = state.loading;
        // A finished load of a web page counts as a visit.
        let (url, title) = (tab.url.clone(), tab.title.clone());
        if !state.loading && (url.starts_with("https://") || url.starts_with("http://")) {
            if self.recorded.get(&id) != Some(&url) {
                self.history.record(&url, &title);
                self.recorded.insert(id, url);
            } else if changed {
                self.history.set_title(&url, &title);
            }
            if self.history.dirty {
                changed = true;
            }
        }
        if id == self.tabs.active {
            self.chrome.set_navigation(
                state.can_back,
                state.can_forward,
                state.loading,
                state.progress,
            );
        }
        self.refresh();
        if changed {
            self.session_changed();
        }
    }

    fn handle(&mut self, msg: Msg) {
        let tx = self.tx.clone();
        match msg {
            Msg::MemoryPressure { critical } => {
                log::log(&format!("memory pressure (critical: {critical})"));
                self.unload_background_tabs();
                self.chrome.trim_caches();
                memory::trim();
            }
            Msg::SetIdleMinutes(minutes) => {
                self.idle_setting.minutes = minutes;
                self.idle_setting.save();
                self.chrome.set_idle_minutes(minutes);
            }
            Msg::FiltersConverted {
                generation,
                converted,
                builtin_only,
            } => {
                self.blocker
                    .on_converted(generation, converted, builtin_only, &tx);
            }
            Msg::RuleListReady {
                generation,
                index,
                pointer,
                error,
            } => {
                let first_start = !self.blocker.started();
                if self
                    .blocker
                    .on_rule_list(generation, index, pointer, error, &tx)
                {
                    log::log("content rules ready");
                    // Converting the lists leaves a large, now-empty heap behind.
                    memory::trim();
                    if first_start {
                        self.start_content();
                    } else {
                        self.blocker.apply_all(self.pages.values().map(Page::view));
                    }
                    if let Some(id) = self.reload_after_rules.take()
                        && let Some(page) = self.pages.get(&id)
                    {
                        page.reload();
                    }
                }
            }
            Msg::FiltersDownloaded { changed, failed } => {
                self.blocker.on_downloaded(changed, failed, &tx);
            }
            Msg::ToggleProtection => {
                self.blocker.set_enabled(!self.blocker.enabled());
                self.blocker.apply_all(self.pages.values().map(Page::view));
                if let Some(page) = self.active_page() {
                    page.reload();
                }
            }
            Msg::ToggleSiteProtection => {
                let Some(site) = self.tabs.active_tab().and_then(|tab| url::site(&tab.url)) else {
                    return;
                };
                let allowed = self.blocker.toggle_site(&site);
                if let Some(page) = self.pages.get(&self.tabs.active) {
                    if allowed {
                        // Takes effect now; the recompiled lists include the exception.
                        self.blocker.detach(page.view());
                        page.reload();
                    } else {
                        self.reload_after_rules = Some(self.tabs.active);
                    }
                }
                self.blocker.rebuild(&tx);
            }
            Msg::ToggleFilterList(index) => self.blocker.toggle_list(index, &tx),
            Msg::ToggleGenericHiding => self.blocker.toggle_generic_hiding(&tx),
            Msg::ToggleLockdown => {
                self.blocker.toggle_lockdown();
                // The mode is fixed when a web view is made, so rebuild them:
                // background tabs sleep, and the front one reloads.
                let active = self.tabs.active;
                self.pages.clear();
                self.idle_checks.clear();
                for tab in self.tabs.tabs.iter_mut() {
                    tab.asleep = tab.id != active;
                    tab.loading = false;
                }
                if active != 0 {
                    self.ensure_page(active);
                    self.present_active();
                }
                memory::trim();
                self.refresh();
            }
            Msg::UpdateFilters => self.blocker.update(&tx, true),
            Msg::EditCustomFilters => {
                let path = self.blocker.custom_filters_file();
                if let Err(e) = std::process::Command::new("/usr/bin/open")
                    .arg("-t")
                    .arg(&path)
                    .spawn()
                {
                    log::log(&format!("open custom filters failed: {e}"));
                }
            }

            Msg::PageChanged(id) => self.page_changed(id),
            Msg::Hover(id, link) => {
                if id == self.tabs.active {
                    self.chrome.set_hover(&link);
                }
            }

            Msg::New(requested) => {
                if !self.blocker.started() {
                    self.pending_urls
                        .push(requested.unwrap_or_else(|| url::NEW_TAB.to_string()));
                    return;
                }
                let target = requested.unwrap_or_else(|| url::NEW_TAB.to_string());
                let previous = self.tabs.active;
                let id = self.tabs.new_tab(&target);
                self.activate_from(previous, id);
            }
            Msg::OpenLink { url, select } => {
                if !self.blocker.started() {
                    return;
                }
                let previous = self.tabs.active;
                let id = self.tabs.open_child(&url, select);
                // Background tabs get no web view until viewed (like restored
                // tabs), so ⌘-clicking links never costs a renderer each.
                if select {
                    self.activate_from(previous, id);
                } else {
                    self.refresh();
                    self.session_changed();
                }
            }
            Msg::Switch(id) => self.activate(id),
            Msg::Close(id) => self.close(id),
            Msg::CloseActive => {
                if self.tabs.active != 0 {
                    self.close(self.tabs.active);
                }
            }
            Msg::CloseOthers(id) => {
                let others: Vec<u32> = self
                    .tabs
                    .tabs
                    .iter()
                    .map(|t| t.id)
                    .filter(|t| *t != id)
                    .collect();
                self.activate(id);
                for other in others {
                    self.close(other);
                }
            }
            Msg::CloseBelow(id) => {
                let Some(index) = self.tabs.index_of(id) else {
                    return;
                };
                let below: Vec<u32> = self.tabs.tabs[index + 1..].iter().map(|t| t.id).collect();
                if below.contains(&self.tabs.active) {
                    self.activate(id);
                }
                for other in below {
                    self.close(other);
                }
            }
            Msg::Duplicate(id) => {
                let zoom = self.tabs.get(id).map_or(1.0, |t| t.zoom);
                if let Some(new) = self.tabs.duplicate(id) {
                    if let Some(tab) = self.tabs.get_mut(new) {
                        tab.zoom = zoom;
                    }
                    self.activate_from(id, new);
                }
            }
            Msg::ReloadTab(id) => match self.pages.get(&id) {
                Some(page) => page.reload(),
                None => self.activate(id),
            },
            Msg::CopyAddress(id) => {
                if let Some(tab) = self.tabs.get(id) {
                    let board = NSPasteboard::generalPasteboard();
                    board.clearContents();
                    board.setString_forType(
                        &objc2_foundation::NSString::from_str(&tab.url),
                        unsafe { NSPasteboardTypeString },
                    );
                }
            }
            Msg::Move(id, index) => {
                self.tabs.move_tab(id, index);
                self.refresh();
                self.session_changed();
            }
            Msg::Reopen => {
                if !self.blocker.started() {
                    return;
                }
                let previous = self.tabs.active;
                if let Some(id) = self.tabs.reopen() {
                    self.activate_from(previous, id);
                }
            }

            Msg::Nav(input) => {
                self.chrome.hide_suggestions();
                let address = url::normalize(&input);
                if self.tabs.active == 0 {
                    let _ = tx.send(Msg::New(Some(address)));
                    return;
                }
                let active = self.tabs.active;
                if let Some(t) = self.tabs.get_mut(active) {
                    t.url = address.clone();
                    t.title = url::display_title(&address);
                }
                match self.pages.get(&active) {
                    Some(page) => {
                        let start_page = if address == url::NEW_TAB {
                            start_page::render(&self.bookmarks, &self.history)
                        } else {
                            String::new()
                        };
                        page.load(&address, &start_page);
                        page.focus();
                    }
                    None => self.activate(active),
                }
                self.refresh();
                self.session_changed();
            }
            Msg::Back => {
                if let Some(page) = self.active_page() {
                    page.back();
                }
            }
            Msg::Fwd => {
                if let Some(page) = self.active_page() {
                    page.forward();
                }
            }
            Msg::Reload => {
                if let Some(page) = self.active_page() {
                    page.reload();
                }
            }
            Msg::Stop => {
                if let Some(page) = self.active_page() {
                    page.stop();
                }
            }
            Msg::FocusAddress => self.chrome.focus_url(),
            Msg::CancelAddress => {
                if self.chrome.suggestions_open() {
                    self.chrome.dismiss_suggestions();
                    return;
                }
                self.chrome.reset_address(&self.tabs);
                if let Some(page) = self.active_page() {
                    page.focus();
                }
            }
            Msg::NextTab | Msg::PreviousTab => {
                let len = self.tabs.tabs.len();
                if len > 1
                    && let Some(index) = self.tabs.index_of(self.tabs.active)
                {
                    let next = if matches!(msg, Msg::NextTab) {
                        (index + 1) % len
                    } else {
                        (index + len - 1) % len
                    };
                    self.activate(self.tabs.tabs[next].id);
                }
            }
            Msg::SwitchIndex(number) => {
                let index = if number == 9 {
                    self.tabs.tabs.len().checked_sub(1)
                } else {
                    Some(number.saturating_sub(1) as usize)
                };
                if let Some(id) = index.and_then(|i| self.tabs.tabs.get(i)).map(|t| t.id) {
                    self.activate(id);
                }
            }

            Msg::ShowFind => {
                if self.active_page().is_some() {
                    self.chrome.show_find();
                }
            }
            Msg::HideFind => {
                self.chrome.hide_find();
                if let Some(page) = self.active_page() {
                    page.focus();
                }
            }
            Msg::Find {
                query,
                backwards,
                typing,
            } => {
                if query.is_empty() {
                    self.chrome.set_find_result(None);
                }
                if let Some(page) = self.active_page() {
                    page.find(&query, backwards, typing, &tx);
                }
            }
            Msg::FindNext(backwards) => {
                if !self.chrome.find_open() {
                    self.chrome.show_find();
                }
                let query = self.chrome.find_query();
                if let Some(page) = self.active_page() {
                    page.find(&query, backwards, false, &tx);
                }
            }
            Msg::FindResult(found) => self.chrome.set_find_result(Some(found)),
            Msg::Zoom(step) => {
                let active = self.tabs.active;
                if let Some(tab) = self.tabs.get_mut(active) {
                    tab.zoom = page::zoom_step(tab.zoom, step);
                    let zoom = tab.zoom;
                    if let Some(page) = self.pages.get(&active) {
                        page.set_zoom(zoom);
                    }
                    self.refresh();
                }
            }
            Msg::Print => {
                if let Some(page) = self.active_page() {
                    page.print(self.chrome.window());
                }
            }

            Msg::AddressTyped(text) => {
                let found = places::suggest(&text, &self.history, &self.bookmarks);
                self.chrome.show_suggestions(found, &text);
            }
            Msg::AddressEnded => {
                self.chrome.hide_suggestions();
                if self.chrome.peeking() && !self.chrome.pointer_in_sidebar() {
                    self.chrome.peek(false);
                }
            }
            Msg::SuggestMove(delta) => self.chrome.step_suggestion(delta),
            Msg::PickSuggestion(index) => {
                if let Some(address) = self.chrome.suggestion(index) {
                    self.handle(Msg::Nav(address));
                }
            }
            Msg::OpenUrl(address) => self.handle(Msg::Nav(address)),
            Msg::ToggleBookmark => {
                let Some((address, title)) = self
                    .tabs
                    .active_tab()
                    .map(|t| (t.url.clone(), t.title.clone()))
                else {
                    return;
                };
                self.bookmarks.toggle(&address, &title);
                self.chrome.set_bookmark_menu(&self.bookmarks);
                self.sync_bookmark();
                self.session_changed();
            }
            Msg::EditBookmarks => {
                self.bookmarks.dirty = true;
                self.bookmarks.save();
                if let Err(e) = std::process::Command::new("/usr/bin/open")
                    .arg("-t")
                    .arg(Bookmarks::path())
                    .spawn()
                {
                    log::log(&format!("open bookmarks failed: {e}"));
                }
            }
            Msg::ClearHistory => {
                self.history.clear();
                self.recorded.clear();
                self.chrome.set_history_menu(&self.history);
                self.session_changed();
            }
            Msg::IconFound(id, icon) => {
                if let Some(site) = self.tabs.get(id).and_then(|t| url::site(&t.url)) {
                    self.chrome.icon_found(&site, &icon, &tx);
                }
            }
            Msg::IconLoaded(site, bytes) => {
                if self.chrome.icon_loaded(&site, bytes) {
                    self.refresh();
                    self.chrome.set_bookmark_menu(&self.bookmarks);
                    self.chrome.set_history_menu(&self.history);
                }
            }
            Msg::ToggleSidebar => {
                let hidden = !self.chrome.sidebar_hidden();
                self.chrome.set_sidebar_hidden(hidden, true);
                session::set_sidebar_hidden(hidden);
            }
            Msg::SidebarPeek(show) => {
                if show {
                    self.chrome.peek(true);
                } else if !self.chrome.address_editing() && !self.chrome.suggestions_open() {
                    self.chrome.peek(false);
                }
            }
            Msg::UnloadCheck { id, playing } => {
                self.idle_checks.remove(&id);
                if playing || id == self.tabs.active || !self.pages.contains_key(&id) {
                    return;
                }
                self.pages.remove(&id);
                if let Some(tab) = self.tabs.get_mut(id) {
                    tab.asleep = true;
                    tab.loading = false;
                }
                log::log(&format!("unloaded idle tab {id}"));
                memory::trim();
                self.refresh();
            }
        }
    }
}
