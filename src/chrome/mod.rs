//! Native browser chrome: the sidebar, the menu bar, and page overlays.

mod kit;
mod menu;
mod overlay;
mod suggest;
mod tablist;

use std::ptr::NonNull;
use std::time::SystemTime;

use objc2::{AnyThread, DefinedClass, define_class};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly, msg_send, sel};
use objc2_app_kit::{
    NSAutoresizingMaskOptions as Mask, NSBezelStyle, NSBorderType, NSButton, NSButtonType,
    NSCellImagePosition, NSColor, NSControlSize, NSControlStateValueOff, NSControlStateValueOn,
    NSEvent, NSEventMask, NSFocusRingType, NSImageView, NSLineBreakMode, NSProgressIndicator,
    NSProgressIndicatorStyle, NSScrollView, NSSwitch, NSTextAlignment, NSTextField, NSView,
    NSBox, NSMenu, NSMenuItem, NSShadow, NSTrackingArea, NSTrackingAreaOptions,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindow, NSWindowButton,
};
use objc2_foundation::{NSObject, NSPoint, NSSize};
use tao::platform::macos::WindowExtMacOS;

use crate::blocker::Status;
use crate::favicon::Icons;
use crate::log;
use crate::msg::{Msg, MsgSender};
use crate::places::{Bookmarks, History, Suggestion};
use crate::tabs::TabManager;
use kit::{
    WEIGHT_MEDIUM, WEIGHT_SEMIBOLD, fill, font, icon_button, label, ns, rect, symbol, tint,
};
use menu::{ADDRESS_TAG, Actions, BOOKMARKS_FIXED, HISTORY_FIXED, ProtectionMenu};
use tablist::TabList;

pub const SIDEBAR_W: f64 = 256.0;
const W: f64 = SIDEBAR_W;
/// Space for the window's traffic-light buttons, which sit inside the sidebar.
const TITLEBAR_H: f64 = 38.0;
const CARD_H: f64 = 82.0;
/// Pointer distance from the window's left edge that reveals a hidden sidebar.
const EDGE_W: f64 = 6.0;
const RECENT_HISTORY: usize = 15;
const FOOTER_H: f64 = CARD_H + 38.0;

fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn age(time: SystemTime) -> String {
    let secs = time.elapsed().map_or(0, |d| d.as_secs());
    match secs {
        0..=89 => "updated just now".into(),
        90..=3_599 => format!("updated {}m ago", secs / 60),
        3_600..=86_399 => format!("updated {}h ago", secs / 3_600),
        _ => format!("updated {}d ago", secs / 86_400),
    }
}

fn install_mouse_navigation(window: &NSWindow, tx: MsgSender) -> Option<Retained<AnyObject>> {
    let window_ptr = window as *const NSWindow as usize;
    let callback = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        let event_ref = unsafe { event.as_ref() };
        let mtm = MainThreadMarker::new().expect("mouse events run on the main thread");
        let in_browser = event_ref
            .window(mtm)
            .is_some_and(|event_window| &*event_window as *const NSWindow as usize == window_ptr);
        if in_browser {
            let direction = match event_ref.buttonNumber() {
                3 => Some(Msg::Back),
                4 => Some(Msg::Fwd),
                _ => None,
            };
            if let Some(direction) = direction {
                let _ = tx.send(direction);
                return std::ptr::null_mut();
            }
        }
        event.as_ptr()
    });
    unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(
            NSEventMask::OtherMouseDown,
            &callback,
        )
    }
}

struct HoverIvars {
    tx: MsgSender,
    enter: Option<fn() -> Msg>,
    exit: Option<fn() -> Msg>,
}

define_class!(
    /// Owns a tracking area and turns pointer entry and exit into messages.
    #[unsafe(super(NSObject))]
    #[name = "BrowserHover"]
    #[thread_kind = MainThreadOnly]
    #[ivars = HoverIvars]
    struct Hover;

    impl Hover {
        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            if let Some(msg) = self.ivars().enter {
                let _ = self.ivars().tx.send(msg());
            }
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            if let Some(msg) = self.ivars().exit {
                let _ = self.ivars().tx.send(msg());
            }
        }
    }
);

fn watch(
    mtm: MainThreadMarker,
    view: &NSView,
    tx: &MsgSender,
    enter: Option<fn() -> Msg>,
    exit: Option<fn() -> Msg>,
) -> Retained<Hover> {
    let hover: Retained<Hover> = unsafe {
        msg_send![super(Hover::alloc(mtm).set_ivars(HoverIvars {
            tx: tx.clone(),
            enter,
            exit,
        })), init]
    };
    let area = unsafe {
        NSTrackingArea::initWithRect_options_owner_userInfo(
            NSTrackingArea::alloc(),
            objc2_foundation::NSRect::ZERO,
            NSTrackingAreaOptions::MouseEnteredAndExited
                | NSTrackingAreaOptions::ActiveInActiveApp
                | NSTrackingAreaOptions::InVisibleRect,
            Some(&hover),
            None,
        )
    };
    view.addTrackingArea(&area);
    hover
}

/// What the sidebar last showed for navigation, to skip redundant updates.
#[derive(Clone, Copy, PartialEq)]
struct Nav {
    back: bool,
    forward: bool,
    loading: bool,
}

#[allow(dead_code)]
pub struct Chrome {
    actions: Retained<Actions>,
    mouse_navigation_monitor: Option<Retained<AnyObject>>,
    window: Retained<NSWindow>,
    root: Retained<NSVisualEffectView>,
    /// The page area right of the sidebar: web views plus overlays.
    stage: Retained<NSView>,
    /// Web views live here; overlays sit above it.
    pages: Retained<NSView>,
    /// Thin strip at the left edge that reveals a hidden sidebar.
    edge: Retained<NSView>,
    hovers: Vec<Retained<Hover>>,
    sidebar_hidden: bool,
    peeking: bool,
    sidebar_item: Retained<NSMenuItem>,
    bookmarks_menu: Retained<NSMenu>,
    history_menu: Retained<NSMenu>,
    address_bg: Retained<NSBox>,
    star: Retained<NSButton>,
    /// None when the page cannot be bookmarked.
    bookmarked: Option<bool>,
    suggestions: suggest::Suggestions,
    icons: Icons,
    back: Retained<NSButton>,
    forward: Retained<NSButton>,
    reload: Retained<NSButton>,
    field: Retained<NSTextField>,
    address_icon: Retained<NSImageView>,
    zoom_badge: Retained<NSButton>,
    tab_count: Retained<NSTextField>,
    tab_list: TabList,
    shield: Retained<NSImageView>,
    site_switch: Retained<NSSwitch>,
    protection_detail: Retained<NSTextField>,
    protection_caption: Retained<NSTextField>,
    protection_spinner: Retained<NSProgressIndicator>,
    /// The menu-bar copy and the footer pop-up copy.
    menus: Vec<ProtectionMenu>,
    memory_label: Retained<NSTextField>,
    progress: overlay::Progress,
    status: overlay::Status,
    find: overlay::FindBar,
    empty: overlay::Empty,
    nav: Option<Nav>,
    address_icon_name: &'static str,
    last_status: Option<Status>,
}

impl Chrome {
    pub fn build(window: &tao::window::Window, tx: MsgSender) -> Self {
        let mtm = MainThreadMarker::new().unwrap();
        let ns_window: Retained<NSWindow> =
            unsafe { Retained::retain(window.ns_window() as *mut NSWindow) }.unwrap();
        let content_view = ns_window.contentView().unwrap();
        let ch = content_view.bounds().size.height;
        let cw = content_view.bounds().size.width;
        let mouse_navigation_monitor = install_mouse_navigation(&ns_window, tx.clone());

        let actions = Actions::new(mtm, tx.clone());
        let target: &AnyObject = &actions;
        let bar = menu::install(mtm, target);
        // The footer's options button shows a second copy of the menu.
        let popup = menu::protection_menu(mtm, target);
        actions.set_protection_popup(popup.menu.clone());

        // Page area, right of the sidebar.
        let stage = kit::view(mtm, rect(W, 0.0, (cw - W).max(100.0), ch));
        stage.setAutoresizingMask(Mask::ViewWidthSizable | Mask::ViewHeightSizable);
        stage.setWantsLayer(true);
        let stage_size = stage.frame().size;
        let pages = kit::view(mtm, rect(0.0, 0.0, stage_size.width, stage_size.height));
        pages.setAutoresizingMask(Mask::ViewWidthSizable | Mask::ViewHeightSizable);
        stage.addSubview(&pages);
        let empty = overlay::Empty::new(mtm, &stage, &actions);
        let progress = overlay::Progress::new(mtm, &stage);
        let status = overlay::Status::new(mtm, &stage);
        let find = overlay::FindBar::new(mtm, &stage, &actions);
        content_view.addSubview(&stage);
        let edge_strip = kit::view(mtm, rect(0.0, 0.0, EDGE_W, ch));
        edge_strip.setAutoresizingMask(Mask::ViewHeightSizable);
        edge_strip.setHidden(true);
        content_view.addSubview(&edge_strip);

        // Sidebar: native translucent material, follows light and dark mode.
        let root: Retained<NSVisualEffectView> = unsafe {
            msg_send![NSVisualEffectView::alloc(mtm), initWithFrame: rect(0.0, 0.0, W, ch)]
        };
        root.setMaterial(NSVisualEffectMaterial::Sidebar);
        root.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        root.setState(NSVisualEffectState::FollowsWindowActiveState);
        root.setAutoresizingMask(Mask::ViewHeightSizable);
        root.setWantsLayer(true);

        let top_pin = Mask::ViewMinYMargin;
        let bottom_pin = Mask::ViewMaxYMargin;

        // Navigation shares the title bar row with the traffic lights.
        let nav_y = ch - TITLEBAR_H + 7.0;
        let back = icon_button(mtm, target, sel!(goBack:), "chevron.left", "Back (⌘[)", 14.0);
        let forward = icon_button(
            mtm,
            target,
            sel!(goForward:),
            "chevron.right",
            "Forward (⌘])",
            14.0,
        );
        let reload = icon_button(
            mtm,
            target,
            sel!(doReload:),
            "arrow.clockwise",
            "Reload (⌘R)",
            13.0,
        );
        for (i, b) in [&back, &forward, &reload].into_iter().enumerate() {
            b.setFrame(rect(W - 102.0 + i as f64 * 30.0, nav_y, 28.0, 26.0));
            b.setAutoresizingMask(top_pin);
            root.addSubview(b);
        }
        back.setEnabled(false);
        forward.setEnabled(false);
        // Sits right of the traffic lights.
        let sidebar_button = icon_button(
            mtm,
            target,
            sel!(toggleSidebar:),
            "sidebar.left",
            "Hide Sidebar (⌃⌘S)",
            14.0,
        );
        sidebar_button.setFrame(rect(76.0, nav_y, 28.0, 26.0));
        sidebar_button.setAutoresizingMask(top_pin);
        root.addSubview(&sidebar_button);

        let address_y = ch - TITLEBAR_H - 38.0;
        let address_bg = fill(
            mtm,
            rect(12.0, address_y, W - 24.0, 32.0),
            &tint(NSColor::labelColor(), 0.07),
            9.0,
        );
        address_bg.setAutoresizingMask(top_pin);
        root.addSubview(&address_bg);
        let address_icon = NSImageView::new(mtm);
        address_icon.setFrame(rect(22.0, address_y + 8.0, 16.0, 16.0));
        address_icon.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        address_icon.setAutoresizingMask(top_pin);
        if let Some(image) = symbol("magnifyingglass", "Search or enter address", 11.0, WEIGHT_MEDIUM)
        {
            address_icon.setImage(Some(&image));
        }
        root.addSubview(&address_icon);
        let field = NSTextField::textFieldWithString(&ns(""), mtm);
        field.setFrame(rect(42.0, address_y + 7.0, W - 64.0, 18.0));
        field.setFont(Some(&font(13.0, 0.0)));
        field.setBezeled(false);
        field.setBordered(false);
        field.setDrawsBackground(false);
        field.setFocusRingType(NSFocusRingType::None);
        field.setPlaceholderString(Some(&ns("Search or enter address")));
        field.setTag(ADDRESS_TAG);
        unsafe {
            field.setTarget(Some(target));
            field.setAction(Some(sel!(submitURL:)));
        }
        unsafe { field.setDelegate(Some(ProtocolObject::from_ref(&*actions))) };
        if let Some(cell) = field.cell() {
            cell.setSendsActionOnEndEditing(false);
            cell.setScrollable(true);
            cell.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        }
        field.setAutoresizingMask(top_pin);
        root.addSubview(&field);
        let zoom_badge = unsafe {
            NSButton::buttonWithTitle_target_action(
                &ns(""),
                Some(target),
                Some(sel!(zoomReset:)),
                mtm,
            )
        };
        zoom_badge.setFont(Some(&font(10.5, WEIGHT_MEDIUM)));
        zoom_badge.setBezelStyle(NSBezelStyle::Badge);
        zoom_badge.setControlSize(NSControlSize::Small);
        zoom_badge.setToolTip(Some(&ns("Reset zoom (⌘0)")));
        zoom_badge.setFrame(rect(W - 66.0, address_y + 7.0, 44.0, 18.0));
        zoom_badge.setAutoresizingMask(top_pin);
        zoom_badge.setHidden(true);
        root.addSubview(&zoom_badge);
        let star = icon_button(
            mtm,
            target,
            sel!(toggleBookmark:),
            "star",
            "Bookmark This Page (⌘D)",
            12.0,
        );
        star.setFrame(rect(W - 42.0, address_y + 6.0, 20.0, 20.0));
        star.setAutoresizingMask(top_pin);
        star.setHidden(true);
        root.addSubview(&star);

        let section_y = ch - TITLEBAR_H - 66.0;
        let tabs_label = label(
            mtm,
            "Tabs",
            11.0,
            WEIGHT_SEMIBOLD,
            &NSColor::tertiaryLabelColor(),
        );
        tabs_label.setFrame(rect(20.0, section_y, 120.0, 15.0));
        tabs_label.setAutoresizingMask(top_pin);
        root.addSubview(&tabs_label);
        let tab_count = label(mtm, "", 11.0, WEIGHT_MEDIUM, &NSColor::tertiaryLabelColor());
        tab_count.setFrame(rect(W - 60.0, section_y, 40.0, 15.0));
        tab_count.setAlignment(NSTextAlignment::Right);
        tab_count.setAutoresizingMask(top_pin);
        root.addSubview(&tab_count);

        let list_top = section_y - 6.0;
        let scroll: Retained<NSScrollView> = unsafe {
            msg_send![
                NSScrollView::alloc(mtm),
                initWithFrame: rect(0.0, FOOTER_H, W, (list_top - FOOTER_H).max(10.0))
            ]
        };
        scroll.setAutoresizingMask(Mask::ViewHeightSizable);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        scroll.setBorderType(NSBorderType::NoBorder);

        let new_tab_button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &ns("  New Tab"),
                Some(target),
                Some(sel!(newTab:)),
                mtm,
            )
        };
        new_tab_button.setFont(Some(&font(12.5, 0.0)));
        new_tab_button.setAlignment(NSTextAlignment::Left);
        if let Some(image) = symbol("plus", "New tab", 12.0, WEIGHT_MEDIUM) {
            new_tab_button.setImage(Some(&image));
            new_tab_button.setImagePosition(NSCellImagePosition::ImageLeading);
        }
        new_tab_button.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        new_tab_button.setButtonType(NSButtonType::MomentaryChange);
        new_tab_button.setBezelStyle(NSBezelStyle::Automatic);
        new_tab_button.setBordered(false);
        new_tab_button.setToolTip(Some(&ns("New Tab (⌘T)")));
        let tab_list = TabList::new(mtm, W, new_tab_button, actions.clone(), tx);
        scroll.setDocumentView(Some(&tab_list.view));
        root.addSubview(&scroll);

        // Protection card.
        let card_y = 34.0;
        let card = fill(
            mtm,
            rect(10.0, card_y, W - 20.0, CARD_H),
            &tint(NSColor::labelColor(), 0.05),
            11.0,
        );
        card.setAutoresizingMask(bottom_pin);
        root.addSubview(&card);
        let cx = 10.0;
        let shield = NSImageView::new(mtm);
        shield.setFrame(rect(cx + 12.0, card_y + CARD_H - 32.0, 20.0, 20.0));
        shield.setAutoresizingMask(bottom_pin);
        root.addSubview(&shield);
        let title = label(
            mtm,
            "Protection",
            13.0,
            WEIGHT_SEMIBOLD,
            &NSColor::labelColor(),
        );
        title.setFrame(rect(cx + 38.0, card_y + CARD_H - 31.0, 120.0, 18.0));
        title.setAutoresizingMask(bottom_pin);
        root.addSubview(&title);
        let site_switch: Retained<NSSwitch> = unsafe {
            msg_send![NSSwitch::alloc(mtm), initWithFrame: rect(W - 62.0, card_y + CARD_H - 32.0, 40.0, 20.0)]
        };
        site_switch.setControlSize(NSControlSize::Small);
        unsafe {
            site_switch.setTarget(Some(target));
            site_switch.setAction(Some(sel!(toggleSiteProtection:)));
        }
        site_switch.setToolTip(Some(&ns("Block ads and trackers on this site")));
        site_switch.setAutoresizingMask(bottom_pin);
        site_switch.setEnabled(false);
        root.addSubview(&site_switch);
        let protection_detail =
            label(mtm, "Starting…", 12.0, 0.0, &NSColor::secondaryLabelColor());
        protection_detail.setFrame(rect(cx + 12.0, card_y + 26.0, W - 44.0, 16.0));
        protection_detail.setAutoresizingMask(bottom_pin);
        root.addSubview(&protection_detail);
        let protection_spinner = NSProgressIndicator::new(mtm);
        protection_spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        protection_spinner.setControlSize(NSControlSize::Mini);
        protection_spinner.setDisplayedWhenStopped(false);
        protection_spinner.setFrame(rect(cx + 12.0, card_y + 10.0, 12.0, 12.0));
        protection_spinner.setAutoresizingMask(bottom_pin);
        root.addSubview(&protection_spinner);
        let protection_caption = label(mtm, "", 11.0, 0.0, &NSColor::tertiaryLabelColor());
        protection_caption.setFrame(rect(cx + 12.0, card_y + 8.0, W - 72.0, 15.0));
        protection_caption.setAutoresizingMask(bottom_pin);
        root.addSubview(&protection_caption);
        let options = icon_button(
            mtm,
            target,
            sel!(showProtectionMenu:),
            "ellipsis.circle",
            "Protection options",
            13.0,
        );
        options.setFrame(rect(W - 46.0, card_y + 5.0, 24.0, 22.0));
        options.setAutoresizingMask(bottom_pin);
        root.addSubview(&options);

        let memory_caption = label(mtm, "Memory", 11.0, 0.0, &NSColor::tertiaryLabelColor());
        memory_caption.setFrame(rect(22.0, 11.0, 100.0, 15.0));
        memory_caption.setAutoresizingMask(bottom_pin);
        root.addSubview(&memory_caption);
        let memory_label = label(
            mtm,
            "…",
            11.0,
            WEIGHT_MEDIUM,
            &NSColor::secondaryLabelColor(),
        );
        memory_label.setFrame(rect(W - 110.0, 11.0, 88.0, 15.0));
        memory_label.setAlignment(NSTextAlignment::Right);
        memory_label.setAutoresizingMask(bottom_pin);
        root.addSubview(&memory_label);

        let edge = fill(mtm, rect(W - 1.0, 0.0, 1.0, ch), &NSColor::separatorColor(), 0.0);
        edge.setAutoresizingMask(Mask::ViewHeightSizable);
        root.addSubview(&edge);

        content_view.addSubview(&root);
        let suggestions = suggest::Suggestions::new(mtm, &content_view, &actions);
        let hovers = vec![
            watch(mtm, &edge_strip, &actions.tx(), Some(|| Msg::SidebarPeek(true)), None),
            watch(mtm, &root, &actions.tx(), None, Some(|| Msg::SidebarPeek(false))),
        ];
        log::log("native sidebar built");

        Self {
            actions,
            mouse_navigation_monitor,
            window: ns_window,
            root,
            stage,
            pages,
            edge: edge_strip,
            hovers,
            sidebar_hidden: false,
            peeking: false,
            sidebar_item: bar.sidebar,
            bookmarks_menu: bar.bookmarks,
            history_menu: bar.history,
            address_bg,
            star,
            bookmarked: None,
            suggestions,
            icons: Icons::default(),
            back,
            forward,
            reload,
            field,
            address_icon,
            zoom_badge,
            tab_count,
            tab_list,
            shield,
            site_switch,
            protection_detail,
            protection_caption,
            protection_spinner,
            menus: vec![bar.protection, popup],
            memory_label,
            progress,
            status,
            find,
            empty,
            nav: None,
            address_icon_name: "magnifyingglass",
            last_status: None,
        }
    }

    /// The view web views attach to.
    pub fn pages(&self) -> &NSView {
        &self.pages
    }

    pub fn window(&self) -> &NSWindow {
        &self.window
    }

    fn editing_address(&self) -> bool {
        self.field.currentEditor().is_some()
    }

    pub fn refresh(&mut self, tabs: &TabManager) {
        self.empty.view.setHidden(!tabs.tabs.is_empty());
        let count = tabs.tabs.len().to_string();
        if self.tab_count.stringValue().to_string() != count {
            self.tab_count.setStringValue(&ns(&count));
        }
        self.tab_list.sync(tabs, &mut self.icons);

        let active = tabs.active_tab();
        let url = active.map_or("", |tab| tab.url.as_str());
        let address = crate::url::address(url);
        if !self.editing_address() && self.field.stringValue().to_string() != address {
            self.field.setStringValue(&ns(address));
        }
        let (icon, tip) = if url.starts_with("https://") {
            ("lock.fill", "Secure connection")
        } else if url.starts_with("http://") {
            ("exclamationmark.triangle", "Not secure")
        } else {
            ("magnifyingglass", "Search or enter address")
        };
        if icon != self.address_icon_name {
            self.address_icon_name = icon;
            if let Some(image) = symbol(icon, tip, 11.0, WEIGHT_MEDIUM) {
                self.address_icon.setImage(Some(&image));
            }
            self.address_icon.setToolTip(Some(&ns(tip)));
        }
        self.set_zoom(active.map_or(1.0, |tab| tab.zoom));
    }

    /// Puts the page address back after Escape, discarding what was typed.
    pub fn reset_address(&mut self, tabs: &TabManager) {
        let url = tabs.active_tab().map_or("", |tab| tab.url.as_str());
        self.field.abortEditing();
        self.field.setStringValue(&ns(crate::url::address(url)));
    }

    fn set_zoom(&self, zoom: f64) {
        let actual = (zoom - 1.0).abs() < 0.001;
        if !actual {
            let text = format!("{:.0}%", zoom * 100.0);
            if self.zoom_badge.title().to_string() != text {
                self.zoom_badge.setTitle(&ns(&text));
            }
        }
        if self.zoom_badge.isHidden() != actual {
            self.zoom_badge.setHidden(actual);
            self.layout_address();
        }
    }

    /// Fits the address text between the icon and whichever badges are showing.
    fn layout_address(&self) {
        let mut right = W - 18.0;
        if !self.star.isHidden() {
            right -= 26.0;
        }
        if !self.zoom_badge.isHidden() {
            let mut badge = self.zoom_badge.frame();
            badge.origin.x = right - 44.0;
            self.zoom_badge.setFrame(badge);
            right -= 48.0;
        }
        let mut frame = self.field.frame();
        frame.size.width = right - frame.origin.x;
        self.field.setFrame(frame);
    }

    /// Shows the star for pages that can be bookmarked, filled when they are.
    pub fn set_bookmarked(&mut self, state: Option<bool>) {
        if self.bookmarked == state {
            return;
        }
        self.bookmarked = state;
        let hidden = state.is_none();
        let filled = state == Some(true);
        let (icon, tip) = if filled {
            ("star.fill", "Remove Bookmark (⌘D)")
        } else {
            ("star", "Bookmark This Page (⌘D)")
        };
        if let Some(image) = symbol(icon, tip, 12.0, WEIGHT_MEDIUM) {
            self.star.setImage(Some(&image));
        }
        self.star.setToolTip(Some(&ns(tip)));
        let tint_color = if filled {
            NSColor::systemYellowColor()
        } else {
            NSColor::secondaryLabelColor()
        };
        self.star.setContentTintColor(Some(&tint_color));
        if self.star.isHidden() != hidden {
            self.star.setHidden(hidden);
            self.layout_address();
        }
    }

    pub fn set_bookmark_menu(&mut self, bookmarks: &Bookmarks) {
        let entries: Vec<_> = bookmarks
            .items
            .iter()
            .map(|b| (b.title.clone(), b.url.clone(), self.icon_for(&b.url)))
            .collect();
        menu::fill_places(&self.actions, &self.bookmarks_menu, BOOKMARKS_FIXED, true, &entries);
    }

    pub fn set_history_menu(&mut self, history: &History) {
        let entries: Vec<_> = history
            .recent(RECENT_HISTORY)
            .map(|v| (v.title.clone(), v.url.clone(), self.icon_for(&v.url)))
            .collect();
        menu::fill_places(&self.actions, &self.history_menu, HISTORY_FIXED, false, &entries);
    }

    fn icon_for(&mut self, url: &str) -> Option<Retained<objc2_app_kit::NSImage>> {
        crate::url::site(url).and_then(|site| self.icons.get(&site))
    }

    pub fn icon_found(&mut self, site: &str, icon_url: &str, tx: &MsgSender) {
        self.icons.found(site, icon_url, tx);
    }

    /// Returns true when a new icon should be shown.
    pub fn icon_loaded(&mut self, site: &str, bytes: Option<Vec<u8>>) -> bool {
        self.icons.loaded(site, bytes)
    }

    pub fn show_suggestions(&mut self, items: Vec<Suggestion>, typed: &str) {
        if !self.editing_address() {
            self.suggestions.hide();
            return;
        }
        let items: Vec<_> = items
            .into_iter()
            .map(|s| {
                let icon = self.icon_for(&s.url);
                (s, icon)
            })
            .collect();
        let bg = self.address_bg.frame();
        let x = self.root.frame().origin.x + bg.origin.x - 2.0;
        self.suggestions.show(&items, typed, x, bg.origin.y - 4.0);
    }

    pub fn hide_suggestions(&self) {
        self.suggestions.hide();
    }

    pub fn suggestions_open(&self) -> bool {
        self.suggestions.is_open()
    }

    pub fn suggestion(&self, index: usize) -> Option<String> {
        self.suggestions.url(index)
    }

    /// Arrow keys: highlight the next suggestion and show its address.
    pub fn step_suggestion(&self, delta: i8) {
        if let Some(text) = self.suggestions.step(delta) {
            self.set_address_text(&text);
        }
    }

    /// Escape with suggestions open closes them and restores the typed text.
    pub fn dismiss_suggestions(&self) {
        let typed = self.suggestions.typed();
        self.suggestions.hide();
        self.set_address_text(&typed);
    }

    fn set_address_text(&self, text: &str) {
        match self.field.currentEditor() {
            Some(editor) => {
                editor.setString(&ns(text));
                let end = text.encode_utf16().count();
                editor.setSelectedRange(objc2_foundation::NSRange::new(end, 0));
            }
            None => self.field.setStringValue(&ns(text)),
        }
    }

    pub fn address_editing(&self) -> bool {
        self.editing_address()
    }

    pub fn sidebar_hidden(&self) -> bool {
        self.sidebar_hidden
    }

    fn set_traffic_lights(&self, visible: bool) {
        for kind in [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ] {
            if let Some(button) = self.window.standardWindowButton(kind) {
                button.setHidden(!visible);
            }
        }
    }

    /// Collapses the sidebar so the page fills the window, or brings it back.
    pub fn set_sidebar_hidden(&mut self, hidden: bool, animated: bool) {
        let Some(content) = self.window.contentView() else {
            return;
        };
        let size = content.bounds().size;
        self.sidebar_hidden = hidden;
        self.peeking = false;
        self.suggestions.hide();
        let root = rect(if hidden { -W } else { 0.0 }, 0.0, W, size.height);
        let stage = if hidden {
            rect(0.0, 0.0, size.width, size.height)
        } else {
            rect(W, 0.0, (size.width - W).max(100.0), size.height)
        };
        if animated {
            kit::animate_frame(&self.root, root, kit::MOVE);
            kit::animate_frame(&self.stage, stage, kit::MOVE);
        } else {
            self.root.setFrame(root);
            self.stage.setFrame(stage);
        }
        self.root.setShadow(None);
        self.root
            .setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        self.set_traffic_lights(!hidden);
        self.edge.setHidden(!hidden);
        let (title, tip) = if hidden {
            ("Show Sidebar", "Show Sidebar (⌃⌘S)")
        } else {
            ("Hide Sidebar", "Hide Sidebar (⌃⌘S)")
        };
        self.sidebar_item.setTitle(&ns(title));
        let _ = tip;
    }

    /// Slides a hidden sidebar over the page, or back out.
    pub fn peek(&mut self, show: bool) {
        if !self.sidebar_hidden || self.peeking == show {
            return;
        }
        self.peeking = show;
        let height = self.root.frame().size.height;
        if show {
            self.root
                .setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
            let shadow = NSShadow::new();
            shadow.setShadowBlurRadius(16.0);
            shadow.setShadowOffset(NSSize::new(0.0, 0.0));
            shadow.setShadowColor(Some(&tint(NSColor::shadowColor(), 0.3)));
            self.root.setShadow(Some(&shadow));
        } else {
            self.suggestions.hide();
        }
        self.set_traffic_lights(show);
        let x = if show { 0.0 } else { -W };
        kit::animate_frame(&self.root, rect(x, 0.0, W, height), kit::MOVE);
    }

    pub fn peeking(&self) -> bool {
        self.peeking
    }

    pub fn pointer_in_sidebar(&self) -> bool {
        let point = self.window.mouseLocationOutsideOfEventStream();
        let frame = self.root.frame();
        point.x >= frame.origin.x
            && point.x < frame.origin.x + frame.size.width
            && point.y >= frame.origin.y
            && point.y < frame.origin.y + frame.size.height
    }

    pub fn set_navigation(&mut self, back: bool, forward: bool, loading: bool, progress: f64) {
        self.progress.set(loading, progress);
        let next = Nav {
            back,
            forward,
            loading,
        };
        if self.nav == Some(next) {
            return;
        }
        let changed_loading = self.nav.is_none_or(|nav| nav.loading != loading);
        self.nav = Some(next);
        self.back.setEnabled(back);
        self.forward.setEnabled(forward);
        if !changed_loading {
            return;
        }
        let (icon, tip, action) = if loading {
            ("xmark", "Stop (⌘.)", sel!(stopLoading:))
        } else {
            ("arrow.clockwise", "Reload (⌘R)", sel!(doReload:))
        };
        if let Some(image) = symbol(icon, tip, 13.0, WEIGHT_MEDIUM) {
            self.reload.setImage(Some(&image));
        }
        self.reload.setToolTip(Some(&ns(tip)));
        unsafe { self.reload.setAction(Some(action)) };
    }

    /// Switching tabs should not animate the previous page's progress away.
    pub fn reset_progress(&mut self) {
        self.progress.reset();
    }

    pub fn set_hover(&self, link: &str) {
        self.status.set(link);
    }

    pub fn show_find(&self) {
        self.find.show();
    }

    pub fn hide_find(&self) {
        self.find.hide();
    }

    pub fn find_open(&self) -> bool {
        self.find.is_open()
    }

    pub fn find_query(&self) -> String {
        self.find.query()
    }

    pub fn set_find_result(&self, found: Option<bool>) {
        self.find.set_found(found);
    }

    pub fn set_protection(&mut self, status: Status) {
        let caption_changes = status.updated.is_some();
        if self.last_status.as_ref() == Some(&status) && !caption_changes {
            return;
        }
        let site = status.site.as_deref();
        let protected = status.enabled && !status.site_allowed;

        let (icon, color, description) = if !status.enabled {
            (
                "shield.slash",
                NSColor::tertiaryLabelColor(),
                "Protection paused",
            )
        } else if status.site_allowed {
            (
                "shield.slash",
                NSColor::secondaryLabelColor(),
                "Protection off for this site",
            )
        } else {
            (
                "checkmark.shield.fill",
                NSColor::systemGreenColor(),
                "Protection on",
            )
        };
        if let Some(image) = symbol(icon, description, 16.0, WEIGHT_MEDIUM) {
            self.shield.setImage(Some(&image));
        }
        self.shield.setContentTintColor(Some(&color));

        let detail = match (status.enabled, site) {
            (false, _) => "Paused on all sites".to_string(),
            (true, None) => "Blocking ads and trackers".to_string(),
            (true, Some(site)) if status.site_allowed => format!("Off for {site}"),
            (true, Some(site)) => format!("On for {site}"),
        };
        self.protection_detail.setStringValue(&ns(&detail));
        self.protection_detail.setToolTip(Some(&ns(&detail)));

        let (caption, caption_color) = if let Some(error) = &status.error {
            (error.clone(), NSColor::systemOrangeColor())
        } else if status.busy {
            (
                "Updating filters…".to_string(),
                NSColor::tertiaryLabelColor(),
            )
        } else {
            let rules = format!("{} rules", grouped(status.rule_count));
            let text = match status.updated {
                Some(time) => format!("{rules} · {}", age(time)),
                None => format!("{rules} · built-in"),
            };
            (text, NSColor::tertiaryLabelColor())
        };
        let caption_x = if status.busy { 40.0 } else { 22.0 };
        self.protection_caption.setFrameOrigin(NSPoint::new(
            caption_x,
            self.protection_caption.frame().origin.y,
        ));
        self.protection_caption.setStringValue(&ns(&caption));
        self.protection_caption.setTextColor(Some(&caption_color));
        self.protection_caption.setToolTip(Some(&ns(&caption)));
        unsafe {
            if status.busy {
                self.protection_spinner.startAnimation(None);
            } else {
                self.protection_spinner.stopAnimation(None);
            }
        }

        self.site_switch.setEnabled(status.enabled && site.is_some());
        self.site_switch
            .setState(if protected || site.is_none() && status.enabled {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });

        let on = |value: bool| {
            if value {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            }
        };
        let site_title = match site {
            Some(site) => format!("Block on {site}"),
            None => "Block on This Site".to_string(),
        };
        for menu in &self.menus {
            menu.global.setState(on(status.enabled));
            menu.site.setTitle(&ns(&site_title));
            menu.site.setState(on(protected));
            menu.site.setEnabled(status.enabled && site.is_some());
            for (item, enabled) in menu.lists.iter().zip(&status.lists) {
                item.setState(on(*enabled));
            }
            menu.generic_hiding.setState(on(status.generic_hiding));
            menu.lockdown.setState(on(status.lockdown));
        }
        self.last_status = Some(status);
    }

    pub fn focus_url(&mut self) {
        self.peek(true);
        unsafe { self.field.selectText(None) };
    }

    pub fn scroll_to_active_tab(&self, tabs: &TabManager) {
        self.tab_list.scroll_to(tabs);
    }

    pub fn set_memory_usage(&self, usage: Option<crate::memory::Usage>) {
        fn size(bytes: u64) -> String {
            if bytes >= 1_000_000_000 {
                format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
            } else {
                format!("{:.0} MB", bytes as f64 / 1_000_000.0)
            }
        }
        let (text, tip) = match usage {
            Some(u) => (
                size(u.total()),
                format!(
                    "App {} + web pages and WebKit services {}, as Activity Monitor counts it.",
                    size(u.app),
                    size(u.helpers)
                ),
            ),
            None => ("—".to_string(), String::new()),
        };
        self.memory_label.setStringValue(&ns(&text));
        self.memory_label.setToolTip(Some(&ns(&tip)));
    }

    /// Drops in-memory caches that can be rebuilt from disk.
    pub fn trim_caches(&mut self) {
        self.icons.trim();
    }
}

impl Drop for Chrome {
    fn drop(&mut self) {
        if let Some(monitor) = self.mouse_navigation_monitor.take() {
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
    }
}
