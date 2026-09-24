mod blocker;
mod log;
mod memory;
mod msg;
mod native;
mod tabs;
mod ui;
mod url;

use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::time::{Duration, Instant};

use blocker::Blocker;
use msg::{Msg, MsgSender};
use tabs::TabManager;
use tao::{
    dpi::LogicalSize,
    event::{ElementState, Event, KeyEvent, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    keyboard::{Key, ModifiersState},
    window::WindowBuilder,
};
use ui::{build_content, content_bounds, eval, show_active};
use wry::{WebContext, WebView, WebViewExtMacOS};

struct BrowserState {
    chrome: native::Chrome,
    blocker: Blocker,
    tabs: TabManager,
    contents: HashMap<u32, WebView>,
    pending_urls: Vec<String>,
}

fn main() -> wry::Result<()> {
    log::install_panic_hook();
    let _ = std::fs::remove_file(log::PATH);
    log::log("minibrowser start");

    let event_loop = EventLoopBuilder::<Msg>::with_user_event().build();
    let window = WindowBuilder::new()
        .with_title("Browser")
        .with_inner_size(LogicalSize::new(1200.0, 800.0))
        .build(&event_loop)
        .unwrap();

    let mut ctx = WebContext::new(None);
    let tx = MsgSender::new(event_loop.create_proxy());

    let mut state = BrowserState {
        chrome: native::Chrome::build(&window, tx.clone()),
        blocker: Blocker::new(),
        tabs: TabManager::new(url::HOME),
        contents: HashMap::new(),
        pending_urls: Vec::new(),
    };
    state.blocker.compile(tx.clone());
    state.chrome.refresh(&state.tabs);

    let mut mods = ModifiersState::empty();
    let mut next_memory_refresh = Instant::now();

    event_loop.run(move |event, _, control_flow| {
        let now = Instant::now();
        if now >= next_memory_refresh {
            state.chrome.set_memory_usage(memory::resident_memory());
            let active = state.tabs.active;
            if sync_tab_url(&mut state.tabs, &state.contents, active) {
                state.chrome.refresh(&state.tabs);
            }
            next_memory_refresh = now + Duration::from_secs(2);
        }
        *control_flow = ControlFlow::WaitUntil(next_memory_refresh);

        if let Event::UserEvent(msg) = event {
            #[cfg(debug_assertions)]
            log::log(&format!("handle {}", msg.name()));
            if panic::catch_unwind(AssertUnwindSafe(|| {
                handle(&window, &mut ctx, &tx, &mut state, msg);
            }))
            .is_err()
            {
                log::log("action panicked, caught");
                state.chrome.refresh(&state.tabs);
            }
            return;
        }

        if let Event::Opened { urls } = event {
            for url in urls {
                let address = url.to_string();
                if url::guard(&address).is_some() {
                    if state.blocker.ready() {
                        let _ = tx.send(Msg::New(Some(address)));
                    } else {
                        state.pending_urls.push(address);
                    }
                }
            }
            return;
        }

        if let Event::WindowEvent { event, .. } = event {
            match event {
                WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
                WindowEvent::Resized(s) => {
                    let s = s.to_logical::<f64>(window.scale_factor());
                    state.chrome.resize_empty(s.width, s.height);
                    for wv in state.contents.values() {
                        if let Err(e) = wv.set_bounds(content_bounds(s.width, s.height)) {
                            log::log(&format!("content resize failed: {e:?}"));
                        }
                    }
                }
                WindowEvent::ModifiersChanged(m) => mods = m,
                WindowEvent::KeyboardInput {
                    event:
                        KeyEvent {
                            logical_key,
                            state: ElementState::Pressed,
                            ..
                        },
                    ..
                } => {
                    if !mods.super_key() {
                        return;
                    }
                    let key: String = match &logical_key {
                        Key::Character(c) => c.to_string().to_lowercase(),
                        _ => return,
                    };
                    match key.as_str() {
                        "t" => {
                            let _ = tx.send(Msg::New(None));
                        }
                        "w" => {
                            let _ = tx.send(Msg::Close(state.tabs.active));
                        }
                        "l" => {
                            state.chrome.focus_url();
                        }
                        "[" => {
                            let _ = tx.send(Msg::Back);
                        }
                        "]" => {
                            let _ = tx.send(Msg::Fwd);
                        }
                        "r" => {
                            let _ = tx.send(Msg::Reload);
                        }
                        digit @ ("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9") => {
                            if let Ok(index) = digit.parse() {
                                let _ = tx.send(Msg::SwitchIndex(index));
                            }
                        }
                        _ => {}
                    }
                    window.request_redraw();
                }
                _ => {}
            }
        }
    });
}

fn handle(
    window: &tao::window::Window,
    ctx: &mut WebContext,
    tx: &MsgSender,
    state: &mut BrowserState,
    msg: Msg,
) {
    let BrowserState {
        chrome,
        blocker,
        tabs,
        contents,
        pending_urls,
    } = state;
    let size = window.inner_size().to_logical::<f64>(window.scale_factor());
    let cb = || content_bounds(size.width, size.height);
    match msg {
        Msg::BlockerReady(pointer) => {
            blocker.accept_compiled(pointer);
            log::log("content blocker ready");
            chrome.set_protection_state(Some(blocker.enabled()));
            build_initial_content(window, ctx, tx, tabs, contents, blocker, cb());
            show_active(contents, None, tabs.active);
            chrome.focus_url();
            for address in pending_urls.drain(..) {
                let _ = tx.send(Msg::New(Some(address)));
            }
        }
        Msg::BlockerFailed => {
            blocker.fail();
            log::log("content blocker unavailable");
            chrome.set_protection_state(None);
            build_initial_content(window, ctx, tx, tabs, contents, blocker, cb());
            show_active(contents, None, tabs.active);
            chrome.focus_url();
            for address in pending_urls.drain(..) {
                let _ = tx.send(Msg::New(Some(address)));
            }
        }
        Msg::ToggleProtection => {
            blocker.toggle(contents);
            chrome.set_protection_state(blocker.available().then_some(blocker.enabled()));
            if let Some(view) = contents.get(&tabs.active) {
                eval(view, "location.reload()", "protection toggle reload");
            }
        }
        Msg::Title(i, title) => {
            let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
            if let Some(t) = tabs.get_mut(i)
                && !title.is_empty()
            {
                t.title = title;
            }
            chrome.refresh(tabs);
        }
        Msg::Page(i, uri) => {
            if let Some(t) = tabs.get_mut(i)
                && t.url != uri
            {
                t.title = url::display_title(&uri);
                t.url = uri;
            }
            chrome.refresh(tabs);
        }
        Msg::New(requested_url) => {
            if !blocker.ready() {
                return;
            }
            sync_tab_url(tabs, contents, tabs.active);
            let previous = tabs.active;
            let focus_address = requested_url.is_none();
            let target = requested_url.unwrap_or_else(|| url::HOME.to_string());
            let nid = tabs.new_tab(&target);
            match build_content(
                window,
                ctx,
                nid,
                &target,
                tx.clone(),
                cb(),
                blocker.active_rule(),
            ) {
                Ok(wv) => {
                    contents.insert(nid, wv);
                }
                Err(e) => {
                    log::log(&format!("new_tab webview failed: {e:?}"));
                    tabs.close_tab(nid);
                }
            }
            show_active(contents, Some(previous), tabs.active);
            chrome.refresh(tabs);
            chrome.scroll_to_active_tab(tabs);
            if focus_address {
                chrome.focus_url();
            }
        }
        Msg::Switch(id) => {
            if !tabs.has(id) {
                log::log(&format!("switch to unknown tab {id}, ignored"));
                return;
            }
            sync_tab_url(tabs, contents, tabs.active);
            let previous = tabs.active;
            tabs.switch(id);
            if let std::collections::hash_map::Entry::Vacant(entry) = contents.entry(id) {
                let u = tabs.active_tab().expect("selected tab exists").url.clone();
                match build_content(window, ctx, id, &u, tx.clone(), cb(), blocker.active_rule()) {
                    Ok(wv) => {
                        entry.insert(wv);
                    }
                    Err(e) => log::log(&format!("missing tab view {id} failed: {e:?}")),
                }
            }
            show_active(contents, Some(previous), id);
            chrome.refresh(tabs);
            chrome.scroll_to_active_tab(tabs);
        }
        Msg::Close(id) => {
            if !tabs.has(id) {
                return;
            }
            contents.remove(&id);
            tabs.close_tab(id);
            let a = tabs.active;
            if let Some(tab) = tabs.active_tab()
                && let std::collections::hash_map::Entry::Vacant(entry) = contents.entry(a)
            {
                let u = tab.url.clone();
                match build_content(window, ctx, a, &u, tx.clone(), cb(), blocker.active_rule()) {
                    Ok(wv) => {
                        entry.insert(wv);
                    }
                    Err(e) => log::log(&format!("rebuild after close failed: {e:?}")),
                }
            }
            show_active(contents, None, a);
            chrome.refresh(tabs);
            chrome.scroll_to_active_tab(tabs);
        }
        Msg::CloseActive => {
            if tabs.active != 0 {
                let _ = tx.send(Msg::Close(tabs.active));
            }
        }
        Msg::Nav(input) => {
            let u = url::normalize(&input);
            if tabs.active == 0 {
                let _ = tx.send(Msg::New(Some(u)));
                return;
            }
            if let Some(t) = tabs.get_mut(tabs.active) {
                t.url = u.clone();
                t.title = url::display_title(&u);
            }
            if let Some(wv) = contents.get(&tabs.active)
                && let Err(e) = wv.load_url(&u)
            {
                log::log(&format!("load_url failed: {e:?}"));
            }
            chrome.refresh(tabs);
        }
        Msg::Back => {
            if let Some(wv) = contents.get(&tabs.active) {
                unsafe { wv.webview().goBack() };
            }
        }
        Msg::Fwd => {
            if let Some(wv) = contents.get(&tabs.active) {
                unsafe { wv.webview().goForward() };
            }
        }
        Msg::Reload => {
            if let Some(wv) = contents.get(&tabs.active) {
                eval(wv, "location.reload()", "reload");
            }
        }
        Msg::FocusAddress => chrome.focus_url(),
        Msg::NextTab | Msg::PreviousTab => {
            let len = tabs.tabs.len();
            if len > 1
                && let Some(index) = tabs.tabs.iter().position(|tab| tab.id == tabs.active)
            {
                let next = if matches!(msg, Msg::NextTab) {
                    (index + 1) % len
                } else {
                    (index + len - 1) % len
                };
                let _ = tx.send(Msg::Switch(tabs.tabs[next].id));
            }
        }
        Msg::SwitchIndex(number) => {
            let index = if number == 9 {
                tabs.tabs.len().checked_sub(1)
            } else {
                Some(number.saturating_sub(1) as usize)
            };
            if let Some(tab) = index.and_then(|index| tabs.tabs.get(index)) {
                let _ = tx.send(Msg::Switch(tab.id));
            }
        }
    }
}

fn sync_tab_url(tabs: &mut TabManager, contents: &HashMap<u32, WebView>, id: u32) -> bool {
    let Some(view) = contents.get(&id) else {
        return false;
    };
    let Ok(current) = view.url() else {
        return false;
    };
    if current.is_empty() {
        return false;
    }
    if let Some(tab) = tabs.get_mut(id)
        && tab.url != current
    {
        tab.url = current;
        return true;
    }
    false
}

fn build_initial_content(
    window: &tao::window::Window,
    ctx: &mut WebContext,
    tx: &MsgSender,
    tabs: &TabManager,
    contents: &mut HashMap<u32, WebView>,
    blocker: &Blocker,
    bounds: wry::Rect,
) {
    let id = tabs.active;
    if id == 0 {
        return;
    }
    if let std::collections::hash_map::Entry::Vacant(entry) = contents.entry(id) {
        let url = tabs.active_tab().expect("initial tab exists").url.clone();
        match build_content(
            window,
            ctx,
            id,
            &url,
            tx.clone(),
            bounds,
            blocker.active_rule(),
        ) {
            Ok(view) => {
                entry.insert(view);
                log::log("first content built");
            }
            Err(error) => log::log(&format!("first content failed: {error:?}")),
        }
    }
}
