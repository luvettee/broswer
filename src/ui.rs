use std::collections::HashMap;
use std::sync::OnceLock;

use objc2_foundation::{MainThreadMarker, NSBundle, NSString};
use objc2_web_kit::{WKContentRuleList, WKWebViewConfiguration};
use tao::dpi::{LogicalPosition, LogicalSize};
use wry::{
    BackgroundThrottlingPolicy, NewWindowResponse, PageLoadEvent, PermissionResponse, Rect,
    WebContext, WebView, WebViewBuilder, WebViewBuilderExtMacos, WebViewExtMacOS,
};

use crate::log;
use crate::msg::{Msg, MsgSender};

pub const SIDEBAR_W: f64 = 248.0;

fn browser_user_agent_suffix() -> &'static str {
    static SUFFIX: OnceLock<String> = OnceLock::new();
    SUFFIX.get_or_init(|| {
        let safari = NSBundle::bundleWithPath(&NSString::from_str("/Applications/Safari.app"));
        let version = safari
            .and_then(|bundle| {
                bundle.objectForInfoDictionaryKey(&NSString::from_str("CFBundleShortVersionString"))
            })
            .and_then(|value| value.downcast::<NSString>().ok())
            .map(|value| value.to_string())
            .filter(|value| {
                !value.is_empty() && value.chars().all(|c| c.is_ascii_digit() || c == '.')
            });
        match version {
            Some(version) => format!("Version/{version} Safari/605.1.15"),
            None => "Safari/605.1.15".to_string(),
        }
    })
}

pub fn eval(view: &WebView, js: &str, what: &str) {
    if let Err(e) = view.evaluate_script(js) {
        log::log(&format!("eval {what} failed: {e:?}"));
    }
}

pub fn content_bounds(w: f64, h: f64) -> Rect {
    Rect {
        position: LogicalPosition::new(SIDEBAR_W, 0.0).into(),
        size: LogicalSize::new((w - SIDEBAR_W).max(100.0), h).into(),
    }
}

pub fn build_content(
    window: &tao::window::Window,
    ctx: &mut WebContext,
    id: u32,
    url: &str,
    tx: MsgSender,
    bounds: Rect,
    rule: Option<&WKContentRuleList>,
) -> wry::Result<WebView> {
    let mtm = MainThreadMarker::new().expect("WebKit runs on the main thread");
    let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
    unsafe {
        configuration
            .setApplicationNameForUserAgent(Some(&NSString::from_str(browser_user_agent_suffix())));
    }
    let t1 = tx.clone();
    let t2 = tx.clone();
    let view = WebViewBuilder::new_with_web_context(ctx)
        .with_webview_configuration(configuration)
        .with_bounds(bounds)
        .with_clipboard(true)
        // Use WebKit's default persistent store so cookies, sessions, and cache survive restarts.
        // Keep background pages alive while limiting their work.
        .with_background_throttling(BackgroundThrottlingPolicy::Throttle)
        .with_permission_handler(|_| PermissionResponse::Default)
        .with_download_started_handler(|_, _| true)
        // ask sites not to track; honored voluntarily, costs nothing
        .with_initialization_script(
            "Object.defineProperty(navigator,'doNotTrack',{get:function(){return '1'}});",
        )
        .with_navigation_handler(move |uri: String| {
            if crate::url::guard(&uri).is_some() {
                let _ = t1.send(Msg::Page(id, uri));
                true
            } else {
                log::log(&format!("blocked nav: {uri}"));
                false
            }
        })
        .with_on_page_load_handler(move |ev, uri| {
            if matches!(ev, PageLoadEvent::Finished) {
                let _ = t2.send(Msg::Page(id, uri));
            }
        })
        .with_document_title_changed_handler(move |title: String| {
            let _ = tx.send(Msg::Title(id, title));
        })
        .with_new_window_req_handler(move |uri, _features| {
            if uri.is_empty() || crate::url::guard(&uri).is_some() {
                NewWindowResponse::Allow
            } else {
                NewWindowResponse::Deny
            }
        })
        .build_as_child(window)?;
    unsafe { view.webview().setAllowsBackForwardNavigationGestures(true) };
    if let Some(rule) = rule {
        unsafe { view.manager().addContentRuleList(rule) };
    }
    view.load_url(url)?;
    Ok(view)
}

pub fn show_active(contents: &HashMap<u32, WebView>, previous: Option<u32>, active: u32) {
    if let Some(id) = previous.filter(|id| *id != active)
        && let Some(view) = contents.get(&id)
        && let Err(e) = view.set_visible(false)
    {
        log::log(&format!("hide tab {id} failed: {e:?}"));
    }
    if let Some(view) = contents.get(&active) {
        if let Err(e) = view.set_visible(true) {
            log::log(&format!("show tab {active} failed: {e:?}"));
        }
        let _ = view.ns_window().makeFirstResponder(Some(&view.webview()));
    }
}
