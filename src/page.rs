//! One tab's web view, plus the native hooks that keep the sidebar in step with it.

use std::cell::{Cell, OnceCell};
use std::ffi::{CStr, c_void};
use std::ptr::NonNull;
use std::sync::OnceLock;

use block2::RcBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{
    AnyClass, AnyObject, Bool, MessageReceiver, NSObject, NSObjectProtocol, ProtocolObject, Sel,
};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSAutoresizingMaskOptions, NSPrintInfo, NSView, NSWindow};
use objc2_foundation::{
    MainThreadMarker, NSBundle, NSKeyValueObservingOptions, NSObjectNSKeyValueObserverRegistration,
    NSString,
};
use objc2_web_kit::{
    WKAudiovisualMediaTypes, WKContentRuleList, WKContentWorld, WKFindConfiguration, WKFindResult,
    WKMediaCaptureState, WKMediaPlaybackState, WKScriptMessage, WKScriptMessageHandler,
    WKUserContentController, WKUserScript, WKUserScriptInjectionTime, WKWebViewConfiguration,
};
use wry::raw_window_handle::{
    AppKitWindowHandle, HandleError, HasWindowHandle, RawWindowHandle, WindowHandle,
};
use wry::{
    BackgroundThrottlingPolicy, NewWindowResponse, PermissionResponse, Rect, WebContext, WebView,
    WebViewBuilder, WebViewBuilderExtMacos, WebViewExtMacOS,
};

use crate::log;
use crate::msg::{Msg, MsgSender};
use crate::url::{self, NEW_TAB};

/// Web view properties the sidebar mirrors; each change posts `Msg::PageChanged`.
const OBSERVED: [&str; 6] = [
    "URL",
    "title",
    "loading",
    "estimatedProgress",
    "canGoBack",
    "canGoForward",
];

const HANDLER: &str = "browser";

/// Runs in WebKit's isolated client world: pages cannot see it, call it, or
/// forge its messages, and it only reacts to real (trusted) clicks.
const LINK_SCRIPT: &str = r#"(()=>{
const post=m=>{try{webkit.messageHandlers.browser.postMessage(m)}catch(_){}};
let hover='';
const href=e=>{const t=e.target,a=t&&t.closest?t.closest('a[href]'):null;
return a&&typeof a.href==='string'&&/^https?:/i.test(a.href)?a.href:''};
const setHover=u=>{if(u!==hover){hover=u;post('h'+u)}};
addEventListener('mouseover',e=>setHover(href(e)),true);
addEventListener('mouseout',e=>{if(!e.relatedTarget)setHover('')},true);
addEventListener('pagehide',()=>setHover(''));
const open=(e,front)=>{const u=href(e);if(!u)return;
e.preventDefault();e.stopImmediatePropagation();post((front?'O':'o')+u)};
addEventListener('click',e=>{if(e.isTrusted&&e.button===0&&e.metaKey)open(e,e.shiftKey)},true);
addEventListener('auxclick',e=>{if(e.isTrusted&&e.button===1)open(e,e.shiftKey)},true);
if(window===top)addEventListener('load',()=>{let best='',rank=1e9;
for(const l of document.querySelectorAll('link[rel][href]')){
const rel=l.rel.toLowerCase(),h=l.href;
if(!/(^|\s)(icon|apple-touch-icon)(\s|$)/.test(rel)||!/^https?:/i.test(h)||/svg/i.test(l.type)||/\.svg([?#]|$)/i.test(h))continue;
const m=/(\d+)x\d+/.exec(l.sizes&&l.sizes.value||''),n=m?+m[1]:(rel.includes('apple')?180:32);
const r=Math.abs(n-32)+(n<32?50:0);if(r<rank){rank=r;best=h}}
if(!best&&/^https?:$/.test(location.protocol))best=location.origin+'/favicon.ico';
if(best)post('i'+best)});
})();"#;

pub const ZOOM_STEPS: [f64; 13] = [
    0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0,
];

fn responds(object: &AnyObject, selector: Sel) -> bool {
    unsafe { msg_send![object, respondsToSelector: selector] }
}

/// One process pool for every tab, tuned for memory over speed: no spare
/// process started ahead of time, no cache of old page processes for quick
/// back navigation, no in-memory back/forward page cache, and web processes
/// suspended aggressively when macOS runs short of memory. These are private
/// WebKit settings; any this version lacks are skipped.
fn lean_process_pool() -> Option<Retained<AnyObject>> {
    thread_local! {
        static POOL: OnceCell<Option<Retained<AnyObject>>> = const { OnceCell::new() };
    }
    POOL.with(|pool| pool.get_or_init(build_lean_pool).clone())
}

fn build_lean_pool() -> Option<Retained<AnyObject>> {
    const FLAGS: [(&CStr, bool); 5] = [
        (c"setPrewarmsProcessesAutomatically:", false),
        (c"setUsesWebProcessCache:", false),
        (c"setPageCacheEnabled:", false),
        (c"setAlwaysKeepAndReuseSwappedProcesses:", false),
        (
            c"setSuspendsWebProcessesAggressivelyOnMemoryPressure:",
            true,
        ),
    ];
    let config_class = AnyClass::get(c"_WKProcessPoolConfiguration")?;
    let pool_class = AnyClass::get(c"WKProcessPool")?;
    let init = Sel::register(c"_initWithConfiguration:");
    let can_init: bool = unsafe { msg_send![pool_class, instancesRespondToSelector: init] };
    if !can_init {
        return None;
    }
    let config: Retained<AnyObject> = unsafe { msg_send![config_class, new] };
    for (name, value) in FLAGS {
        let selector = Sel::register(name);
        if responds(&config, selector) {
            unsafe {
                let _: () = MessageReceiver::send_message(&*config, selector, (Bool::new(value),));
            }
        }
    }
    let allocated: Allocated<AnyObject> = unsafe { msg_send![pool_class, alloc] };
    let pool: Option<Retained<AnyObject>> =
        unsafe { msg_send![allocated, _initWithConfiguration: &*config] };
    log::log(&format!("lean process pool: {}", pool.is_some()));
    pool
}

/// Turns off the back/forward page cache, which keeps whole previous pages
/// in the web process's memory.
fn disable_page_cache(configuration: &WKWebViewConfiguration) {
    let preferences = unsafe { configuration.preferences() };
    let selector = Sel::register(c"_setUsesPageCache:");
    if responds(&preferences, selector) {
        unsafe {
            let _: () = MessageReceiver::send_message(&*preferences, selector, (Bool::NO,));
        }
    }
}

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

fn browser_user_agent_suffix() -> &'static str {
    static SUFFIX: OnceLock<String> = OnceLock::new();
    SUFFIX.get_or_init(|| {
        let safari = NSBundle::bundleWithPath(&ns("/Applications/Safari.app"));
        let version = safari
            .and_then(|bundle| bundle.objectForInfoDictionaryKey(&ns("CFBundleShortVersionString")))
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

struct HookIvars {
    tx: MsgSender,
    id: u32,
    /// Coalesces bursts of property changes into one message.
    queued: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "BrowserPageHooks"]
    #[thread_kind = MainThreadOnly]
    #[ivars = HookIvars]
    struct PageHooks;

    impl PageHooks {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe(
            &self,
            _key: Option<&NSString>,
            _object: Option<&AnyObject>,
            _change: Option<&AnyObject>,
            _context: *mut c_void,
        ) {
            let ivars = self.ivars();
            if !ivars.queued.replace(true) {
                let _ = ivars.tx.send(Msg::PageChanged(ivars.id));
            }
        }
    }

    unsafe impl NSObjectProtocol for PageHooks {}

    unsafe impl WKScriptMessageHandler for PageHooks {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive(&self, _controller: &WKUserContentController, message: &WKScriptMessage) {
            let Ok(body) = unsafe { message.body() }.downcast::<NSString>() else {
                return;
            };
            let body = body.to_string();
            let ivars = self.ivars();
            let msg = match body.split_at_checked(1) {
                Some(("h", link)) => Msg::Hover(ivars.id, link.to_string()),
                Some(("i", icon)) if url::guard(icon).is_some() => {
                    Msg::IconFound(ivars.id, icon.to_string())
                }
                Some((kind @ ("o" | "O"), link)) if url::guard(link).is_some() => Msg::OpenLink {
                    url: link.to_string(),
                    select: kind == "O",
                },
                _ => return,
            };
            let _ = ivars.tx.send(msg);
        }
    }
);

/// Lets wry attach web views to our content view instead of the window root.
struct Host<'a>(&'a NSView);

impl HasWindowHandle for Host<'_> {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = AppKitWindowHandle::new(NonNull::from(self.0).cast());
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::AppKit(raw)) })
    }
}

/// What the browser reads back from a web view after it changes.
pub struct PageState {
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub progress: f64,
    pub can_back: bool,
    pub can_forward: bool,
}

pub struct Page {
    view: WebView,
    hooks: Retained<PageHooks>,
}

impl Page {
    pub fn new(
        host: &NSView,
        ctx: &mut WebContext,
        id: u32,
        address: &str,
        start_page: &str,
        tx: MsgSender,
        rules: &[Retained<WKContentRuleList>],
        lockdown: bool,
    ) -> wry::Result<Self> {
        let mtm = MainThreadMarker::new().expect("WebKit runs on the main thread");
        let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
        unsafe {
            configuration.setApplicationNameForUserAgent(Some(&ns(browser_user_agent_suffix())));
        }
        if let Some(pool) = lean_process_pool() {
            let _: () = unsafe { msg_send![&*configuration, setProcessPool: &*pool] };
        }
        disable_page_cache(&configuration);
        unsafe {
            // Video and audio wait for a click; autoplaying video alone can
            // cost hundreds of megabytes in WebKit's GPU process.
            configuration.setMediaTypesRequiringUserActionForPlayback(WKAudiovisualMediaTypes::All);
            if lockdown {
                configuration
                    .defaultWebpagePreferences()
                    .setLockdownModeEnabled(true);
            }
        }
        let size = host.frame().size;
        let windows = tx.clone();
        let view = WebViewBuilder::new_with_web_context(ctx)
            .with_webview_configuration(configuration)
            .with_bounds(Rect {
                position: wry::dpi::LogicalPosition::new(0.0, 0.0).into(),
                size: wry::dpi::LogicalSize::new(size.width, size.height).into(),
            })
            .with_visible(false)
            .with_clipboard(true)
            .with_autoplay(false)
            // WebKit's default persistent store keeps cookies, sessions, and cache.
            .with_background_throttling(BackgroundThrottlingPolicy::Throttle)
            .with_back_forward_navigation_gestures(true)
            .with_permission_handler(|_| PermissionResponse::Default)
            .with_download_started_handler(|_, _| true)
            // ask sites not to track; honored voluntarily, costs nothing
            .with_initialization_script(
                "Object.defineProperty(navigator,'doNotTrack',{get:function(){return '1'}});",
            )
            .with_navigation_handler(|uri: String| {
                let allowed = url::guard(&uri).is_some();
                if !allowed {
                    log::log(&format!("blocked nav: {uri}"));
                }
                allowed
            })
            .with_new_window_req_handler(move |uri, features| {
                if url::guard(&uri).is_none() && !uri.is_empty() {
                    return NewWindowResponse::Deny;
                }
                // Plain target=_blank links and window.open(url) become tabs;
                // sized pop-ups (sign-in, payment) keep their own window.
                if features.size.is_none() && !uri.is_empty() && uri != NEW_TAB {
                    let _ = windows.send(Msg::OpenLink {
                        url: uri,
                        select: true,
                    });
                    return NewWindowResponse::Deny;
                }
                NewWindowResponse::Allow
            })
            .build_as_child(&Host(host))?;

        let webview = view.webview();
        webview.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );

        let hooks: Retained<PageHooks> = unsafe {
            msg_send![
                super(PageHooks::alloc(mtm).set_ivars(HookIvars {
                    tx,
                    id,
                    queued: Cell::new(false),
                })),
                init
            ]
        };
        for key in OBSERVED {
            unsafe {
                webview.addObserver_forKeyPath_options_context(
                    &hooks,
                    &ns(key),
                    NSKeyValueObservingOptions::empty(),
                    std::ptr::null_mut(),
                );
            }
        }
        let controller = view.manager();
        let world = unsafe { WKContentWorld::defaultClientWorld(mtm) };
        unsafe {
            controller.addScriptMessageHandler_contentWorld_name(
                ProtocolObject::from_ref(&*hooks),
                &world,
                &ns(HANDLER),
            );
            let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly_inContentWorld(
                WKUserScript::alloc(mtm),
                &ns(LINK_SCRIPT),
                WKUserScriptInjectionTime::AtDocumentStart,
                false,
                &world,
            );
            controller.addUserScript(&script);
            for rules in rules {
                controller.addContentRuleList(rules);
            }
        }

        let page = Self { view, hooks };
        page.load(address, start_page);
        Ok(page)
    }

    pub fn view(&self) -> &WebView {
        &self.view
    }

    pub fn load(&self, address: &str, start_page: &str) {
        let result = if address == NEW_TAB {
            self.view.load_html(start_page)
        } else {
            self.view.load_url(address)
        };
        if let Err(e) = result {
            log::log(&format!("load failed: {e:?}"));
        }
    }

    /// Reads the observed properties and re-arms change notifications.
    pub fn state(&self) -> PageState {
        self.hooks.ivars().queued.set(false);
        let view = self.view.webview();
        unsafe {
            PageState {
                url: view
                    .URL()
                    .and_then(|u| u.absoluteString())
                    .map_or_else(String::new, |u| u.to_string()),
                title: view.title().map_or_else(String::new, |t| t.to_string()),
                loading: view.isLoading(),
                progress: view.estimatedProgress(),
                can_back: view.canGoBack(),
                can_forward: view.canGoForward(),
            }
        }
    }

    pub fn show(&self, visible: bool) {
        if let Err(e) = self.view.set_visible(visible) {
            log::log(&format!("set_visible failed: {e:?}"));
        }
    }

    pub fn focus(&self) {
        let view = self.view.webview();
        if let Some(window) = view.window() {
            window.makeFirstResponder(Some(&view));
        }
    }

    pub fn back(&self) {
        unsafe { self.view.webview().goBack() };
    }

    pub fn forward(&self) {
        unsafe { self.view.webview().goForward() };
    }

    pub fn reload(&self) {
        unsafe { self.view.webview().reload() };
    }

    pub fn stop(&self) {
        unsafe { self.view.webview().stopLoading() };
    }

    pub fn set_zoom(&self, zoom: f64) {
        unsafe { self.view.webview().setPageZoom(zoom) };
    }

    pub fn find(&self, query: &str, backwards: bool, typing: bool, tx: &MsgSender) {
        if query.is_empty() {
            return;
        }
        let mtm = MainThreadMarker::new().expect("WebKit runs on the main thread");
        if typing {
            // Refine the current match instead of jumping past it.
            let _ = self
                .view
                .evaluate_script("try{getSelection().collapseToStart()}catch(_){}");
        }
        let tx = tx.clone();
        let done = RcBlock::new(move |result: NonNull<WKFindResult>| {
            let found = unsafe { result.as_ref().matchFound() };
            let _ = tx.send(Msg::FindResult(found));
        });
        unsafe {
            let config = WKFindConfiguration::new(mtm);
            config.setBackwards(backwards);
            config.setCaseSensitive(false);
            config.setWraps(true);
            self.view
                .webview()
                .findString_withConfiguration_completionHandler(&ns(query), Some(&config), &done);
        }
    }

    /// Asks whether this page is playing media; answers with `Msg::UnloadCheck`.
    /// Pages using the camera or microphone are never unloaded.
    pub fn check_idle(&self, tx: &MsgSender) {
        let view = self.view.webview();
        let capturing = unsafe {
            view.cameraCaptureState() != WKMediaCaptureState::None
                || view.microphoneCaptureState() != WKMediaCaptureState::None
        };
        let id = self.hooks.ivars().id;
        if capturing {
            let _ = tx.send(Msg::UnloadCheck { id, playing: true });
            return;
        }
        let tx = tx.clone();
        let done = RcBlock::new(move |state: WKMediaPlaybackState| {
            let _ = tx.send(Msg::UnloadCheck {
                id,
                playing: state == WKMediaPlaybackState::Playing,
            });
        });
        unsafe { view.requestMediaPlaybackStateWithCompletionHandler(&done) };
    }

    pub fn print(&self, window: &NSWindow) {
        let view = self.view.webview();
        unsafe {
            let operation = view.printOperationWithPrintInfo(&NSPrintInfo::sharedPrintInfo());
            operation.setShowsPrintPanel(true);
            operation.setShowsProgressPanel(true);
            if let Some(print_view) = operation.view() {
                print_view.setFrame(view.bounds());
            }
            operation.runOperationModalForWindow_delegate_didRunSelector_contextInfo(
                window,
                None,
                None,
                std::ptr::null_mut(),
            );
        }
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        let view = self.view.webview();
        for key in OBSERVED {
            unsafe { view.removeObserver_forKeyPath(&self.hooks, &ns(key)) };
        }
        let mtm = MainThreadMarker::new().expect("WebKit runs on the main thread");
        unsafe {
            self.view
                .manager()
                .removeScriptMessageHandlerForName_contentWorld(
                    &ns(HANDLER),
                    &WKContentWorld::defaultClientWorld(mtm),
                );
        }
    }
}

/// The next zoom level in `direction` from `current`.
pub fn zoom_step(current: f64, direction: i8) -> f64 {
    match direction {
        0 => 1.0,
        d if d > 0 => ZOOM_STEPS
            .iter()
            .copied()
            .find(|z| *z > current + 0.001)
            .unwrap_or(current),
        _ => ZOOM_STEPS
            .iter()
            .rev()
            .copied()
            .find(|z| *z < current - 0.001)
            .unwrap_or(current),
    }
}

#[cfg(test)]
mod tests {
    use super::zoom_step;

    #[test]
    fn zoom_steps() {
        assert_eq!(zoom_step(1.0, 1), 1.1);
        assert_eq!(zoom_step(1.0, -1), 0.9);
        assert_eq!(zoom_step(3.0, 1), 3.0);
        assert_eq!(zoom_step(0.5, -1), 0.5);
        assert_eq!(zoom_step(1.75, 0), 1.0);
    }
}
