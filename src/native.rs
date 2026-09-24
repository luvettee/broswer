use std::collections::HashMap;
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSBezelStyle, NSBorderType, NSBox, NSBoxType,
    NSButton, NSButtonType, NSCellImagePosition, NSColor, NSEvent, NSEventMask,
    NSEventModifierFlags, NSFocusRingType, NSFont, NSImage, NSLineBreakMode, NSMenu, NSMenuItem,
    NSScrollView, NSTextAlignment, NSTextField, NSTextFieldBezelStyle, NSTitlePosition, NSView,
    NSWindow,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use tao::platform::macos::WindowExtMacOS;

use crate::log;
use crate::msg::{Msg, MsgSender};
use crate::tabs::TabManager;

const W: f64 = 248.0;
const ROW_H: f64 = 38.0;

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

fn host(url: &str) -> String {
    let s = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let s = s.strip_prefix("www.").unwrap_or(s);
    s.split('/').next().unwrap_or(s).to_string()
}

fn font(size: f64) -> Retained<NSFont> {
    NSFont::systemFontOfSize(size)
}

fn medium_font(size: f64) -> Retained<NSFont> {
    NSFont::systemFontOfSize_weight(size, 0.23)
}

fn color(red: u8, green: u8, blue: u8) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        f64::from(red) / 255.0,
        f64::from(green) / 255.0,
        f64::from(blue) / 255.0,
        1.0,
    )
}

fn symbol(name: &str, description: &str) -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(name), Some(&ns(description)))
}

fn view(mtm: MainThreadMarker, frame: NSRect) -> Retained<NSView> {
    unsafe { msg_send![NSView::alloc(mtm), initWithFrame: frame] }
}

define_class!(
    #[unsafe(super(NSView))]
    #[name = "BrowserTabList"]
    #[thread_kind = MainThreadOnly]
    struct TabListView;

    impl TabListView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

fn tab_list_view(mtm: MainThreadMarker, frame: NSRect) -> Retained<NSView> {
    let list: Retained<TabListView> =
        unsafe { msg_send![TabListView::alloc(mtm), initWithFrame: frame] };
    list.into_super()
}

fn box_(mtm: MainThreadMarker, frame: NSRect) -> Retained<NSBox> {
    unsafe { msg_send![NSBox::alloc(mtm), initWithFrame: frame] }
}

fn shortcut(
    menu: &NSMenu,
    target: &AnyObject,
    title: &str,
    action: Sel,
    key: &str,
) -> Retained<NSMenuItem> {
    let item =
        unsafe { menu.addItemWithTitle_action_keyEquivalent(&ns(title), Some(action), &ns(key)) };
    unsafe { item.setTarget(Some(target)) };
    item
}

fn install_menu(mtm: MainThreadMarker, target: &AnyObject) {
    let main = NSMenu::new(mtm);
    let app_item = NSMenuItem::new(mtm);
    app_item.setTitle(&ns("Browser"));
    let app_menu = NSMenu::new(mtm);
    unsafe {
        app_menu.addItemWithTitle_action_keyEquivalent(
            &ns("Quit Browser"),
            Some(sel!(terminate:)),
            &ns("q"),
        );
    }
    app_item.setSubmenu(Some(&app_menu));
    main.addItem(&app_item);

    let file_item = NSMenuItem::new(mtm);
    file_item.setTitle(&ns("File"));
    let file_menu = NSMenu::new(mtm);
    shortcut(&file_menu, target, "New Tab", sel!(newTab:), "t");
    shortcut(&file_menu, target, "Close Tab", sel!(closeCurrentTab:), "w");
    file_item.setSubmenu(Some(&file_menu));
    main.addItem(&file_item);

    let edit_item = NSMenuItem::new(mtm);
    edit_item.setTitle(&ns("Edit"));
    let edit_menu = NSMenu::new(mtm);
    unsafe {
        edit_menu.addItemWithTitle_action_keyEquivalent(&ns("Undo"), Some(sel!(undo:)), &ns("z"));
        let redo = edit_menu.addItemWithTitle_action_keyEquivalent(
            &ns("Redo"),
            Some(sel!(redo:)),
            &ns("z"),
        );
        redo.setKeyEquivalentModifierMask(
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        );
        edit_menu.addItem(&NSMenuItem::separatorItem(mtm));
        edit_menu.addItemWithTitle_action_keyEquivalent(&ns("Cut"), Some(sel!(cut:)), &ns("x"));
        edit_menu.addItemWithTitle_action_keyEquivalent(&ns("Copy"), Some(sel!(copy:)), &ns("c"));
        edit_menu.addItemWithTitle_action_keyEquivalent(&ns("Paste"), Some(sel!(paste:)), &ns("v"));
        edit_menu.addItemWithTitle_action_keyEquivalent(
            &ns("Select All"),
            Some(sel!(selectAll:)),
            &ns("a"),
        );
    }
    edit_item.setSubmenu(Some(&edit_menu));
    main.addItem(&edit_item);

    let view_item = NSMenuItem::new(mtm);
    view_item.setTitle(&ns("View"));
    let view_menu = NSMenu::new(mtm);
    shortcut(
        &view_menu,
        target,
        "Focus Address",
        sel!(focusAddress:),
        "l",
    );
    shortcut(&view_menu, target, "Reload", sel!(doReload:), "r");
    view_menu.addItem(&NSMenuItem::separatorItem(mtm));
    shortcut(&view_menu, target, "Back", sel!(goBack:), "[");
    shortcut(&view_menu, target, "Forward", sel!(goForward:), "]");
    view_item.setSubmenu(Some(&view_menu));
    main.addItem(&view_item);

    let tabs_item = NSMenuItem::new(mtm);
    tabs_item.setTitle(&ns("Tabs"));
    let tabs_menu = NSMenu::new(mtm);
    let next = shortcut(&tabs_menu, target, "Next Tab", sel!(nextTab:), "\t");
    next.setKeyEquivalentModifierMask(NSEventModifierFlags::Control);
    let previous = shortcut(&tabs_menu, target, "Previous Tab", sel!(previousTab:), "\t");
    previous
        .setKeyEquivalentModifierMask(NSEventModifierFlags::Control | NSEventModifierFlags::Shift);
    tabs_menu.addItem(&NSMenuItem::separatorItem(mtm));
    for index in 1..=9 {
        let title = if index == 9 {
            "Last Tab".to_string()
        } else {
            format!("Tab {index}")
        };
        let item = shortcut(
            &tabs_menu,
            target,
            &title,
            sel!(selectTab:),
            &index.to_string(),
        );
        item.setTag(index);
    }
    tabs_item.setSubmenu(Some(&tabs_menu));
    main.addItem(&tabs_item);
    NSApplication::sharedApplication(mtm).setMainMenu(Some(&main));
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

struct ActionIvars {
    tx: MsgSender,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "MiniActions"]
    #[thread_kind = MainThreadOnly]
    #[ivars = ActionIvars]
    struct Actions;

    impl Actions {
        #[unsafe(method(goBack:))]
        fn go_back(&self, _sender: Option<&AnyObject>) {
            let _ = self.ivars().tx.send(Msg::Back);
        }

        #[unsafe(method(goForward:))]
        fn go_forward(&self, _sender: Option<&AnyObject>) {
            let _ = self.ivars().tx.send(Msg::Fwd);
        }

        #[unsafe(method(doReload:))]
        fn do_reload(&self, _sender: Option<&AnyObject>) {
            let _ = self.ivars().tx.send(Msg::Reload);
        }

        #[unsafe(method(newTab:))]
        fn new_tab(&self, _sender: Option<&AnyObject>) {
            let _ = self.ivars().tx.send(Msg::New(None));
        }

        #[unsafe(method(closeCurrentTab:))]
        fn close_current_tab(&self, _sender: Option<&AnyObject>) {
            let _ = self.ivars().tx.send(Msg::CloseActive);
        }

        #[unsafe(method(focusAddress:))]
        fn focus_address(&self, _sender: Option<&AnyObject>) {
            let _ = self.ivars().tx.send(Msg::FocusAddress);
        }

        #[unsafe(method(nextTab:))]
        fn next_tab(&self, _sender: Option<&AnyObject>) {
            let _ = self.ivars().tx.send(Msg::NextTab);
        }

        #[unsafe(method(previousTab:))]
        fn previous_tab(&self, _sender: Option<&AnyObject>) {
            let _ = self.ivars().tx.send(Msg::PreviousTab);
        }

        #[unsafe(method(selectTab:))]
        fn select_tab(&self, sender: Option<&NSMenuItem>) {
            if let Some(item) = sender {
                let _ = self.ivars().tx.send(Msg::SwitchIndex(item.tag() as u8));
            }
        }

        #[unsafe(method(toggleProtection:))]
        fn toggle_protection(&self, _sender: Option<&AnyObject>) {
            let _ = self.ivars().tx.send(Msg::ToggleProtection);
        }

        #[unsafe(method(openTab:))]
        fn open_tab(&self, sender: Option<&NSButton>) {
            if let Some(b) = sender {
                let _ = self.ivars().tx.send(Msg::Switch(b.tag() as u32));
            }
        }

        #[unsafe(method(closeTab:))]
        fn close_tab(&self, sender: Option<&NSButton>) {
            if let Some(b) = sender {
                let _ = self.ivars().tx.send(Msg::Close(b.tag() as u32));
            }
        }

        #[unsafe(method(submitURL:))]
        fn submit_url(&self, sender: Option<&NSTextField>) {
            if let Some(f) = sender {
                let _ = self.ivars().tx.send(Msg::Nav(f.stringValue().to_string()));
            }
        }
    }
);

#[allow(dead_code)]
pub struct Chrome {
    actions: Retained<Actions>,
    mouse_navigation_monitor: Option<Retained<AnyObject>>,
    root: Retained<NSView>,
    scroll: Retained<NSScrollView>,
    field: Retained<NSTextField>,
    tab_count: Retained<NSTextField>,
    protection: Retained<NSButton>,
    protection_status: Retained<NSTextField>,
    memory_label: Retained<NSTextField>,
    list: Retained<NSView>,
    new_tab_button: Retained<NSButton>,
    empty_state: Retained<NSView>,
    empty_title: Retained<NSTextField>,
    empty_hint: Retained<NSTextField>,
    empty_button: Retained<NSButton>,
    rows: HashMap<u32, (String, Retained<NSView>)>,
    last_key: String,
}

impl Chrome {
    pub fn build(window: &tao::window::Window, tx: MsgSender) -> Self {
        unsafe {
            let mtm = MainThreadMarker::new().unwrap();
            let content = &*(window.ns_window() as *const NSWindow);
            let content_view = content.contentView().unwrap();
            let ch = content_view.bounds().size.height;
            let cw = content_view.bounds().size.width;
            let mouse_navigation_monitor = install_mouse_navigation(content, tx.clone());

            let actions: Retained<Actions> = msg_send![
                super(Actions::alloc(mtm).set_ivars(ActionIvars { tx })),
                init
            ];
            let target: &AnyObject = &actions;
            install_menu(mtm, target);

            let empty_state = view(mtm, rect(W, 0.0, (cw - W).max(100.0), ch));
            empty_state.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            let empty_bg = box_(mtm, rect(0.0, 0.0, (cw - W).max(100.0), ch));
            empty_bg.setBoxType(NSBoxType::Custom);
            empty_bg.setTitlePosition(NSTitlePosition::NoTitle);
            empty_bg.setFillColor(&NSColor::whiteColor());
            empty_bg.setBorderColor(&NSColor::clearColor());
            empty_bg.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            empty_state.addSubview(&empty_bg);

            let empty_title = NSTextField::labelWithString(&ns("No tabs open"), mtm);
            empty_title.setFont(Some(&medium_font(18.0)));
            empty_title.setAlignment(NSTextAlignment::Center);
            empty_title.setTextColor(Some(&color(55, 53, 47)));
            empty_state.addSubview(&empty_title);

            let empty_hint =
                NSTextField::labelWithString(&ns("Open a tab to start browsing."), mtm);
            empty_hint.setFont(Some(&font(12.0)));
            empty_hint.setAlignment(NSTextAlignment::Center);
            empty_hint.setTextColor(Some(&color(130, 128, 124)));
            empty_state.addSubview(&empty_hint);

            let empty_button = NSButton::buttonWithTitle_target_action(
                &ns("New tab"),
                Some(target),
                Some(sel!(newTab:)),
                mtm,
            );
            empty_button.setFont(Some(&medium_font(12.0)));
            empty_button.setBezelStyle(NSBezelStyle::Push);
            empty_state.addSubview(&empty_button);
            empty_state.setHidden(true);
            content_view.addSubview(&empty_state);

            let root = view(mtm, rect(0.0, 0.0, W, ch));
            root.setAutoresizingMask(NSAutoresizingMaskOptions::ViewHeightSizable);
            let bg = box_(mtm, rect(0.0, 0.0, W, ch));
            bg.setBoxType(NSBoxType::Custom);
            bg.setTitlePosition(NSTitlePosition::NoTitle);
            bg.setFillColor(&color(249, 249, 247));
            bg.setBorderColor(&NSColor::clearColor());
            bg.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            root.addSubview(&bg);

            let top_pin = NSAutoresizingMaskOptions::ViewMinYMargin;
            let toolbar_y = ch - 47.0;
            for (i, (glyph, icon, label, action)) in [
                ("‹", "chevron.left", "Back", sel!(goBack:)),
                ("›", "chevron.right", "Forward", sel!(goForward:)),
                ("↻", "arrow.clockwise", "Reload", sel!(doReload:)),
            ]
            .into_iter()
            .enumerate()
            {
                let b = NSButton::buttonWithTitle_target_action(
                    &ns(""),
                    Some(target),
                    Some(action),
                    mtm,
                );
                b.setTitle(&ns(glyph));
                b.setFont(Some(&font(19.0)));
                if let Some(image) = symbol(icon, label) {
                    b.setImage(Some(&image));
                    b.setImagePosition(NSCellImagePosition::ImageOnly);
                }
                b.setContentTintColor(Some(&color(79, 77, 73)));
                b.setFrame(rect(14.0 + i as f64 * 34.0, toolbar_y, 28.0, 28.0));
                b.setButtonType(NSButtonType::MomentaryLight);
                b.setBezelStyle(NSBezelStyle::Automatic);
                b.setBordered(false);
                b.setAutoresizingMask(top_pin);
                b.setToolTip(Some(&ns(label)));
                root.addSubview(&b);
            }

            let address_y = ch - 88.0;
            let field = NSTextField::textFieldWithString(&ns(""), mtm);
            field.setFrame(rect(16.0, address_y + 3.0, W - 32.0, 31.0));
            field.setFont(Some(&font(13.0)));
            field.setBezelStyle(NSTextFieldBezelStyle::RoundedBezel);
            field.setFocusRingType(NSFocusRingType::None);
            field.setPlaceholderString(Some(&ns("Search or enter address")));
            field.setTarget(Some(target));
            field.setAction(Some(sel!(submitURL:)));
            if let Some(cell) = field.cell() {
                cell.setSendsActionOnEndEditing(false);
            }
            field.setAutoresizingMask(top_pin);
            root.addSubview(&field);

            let section_y = ch - 121.0;
            let label = NSTextField::labelWithString(&ns("Tabs"), mtm);
            label.setFrame(rect(18.0, section_y + 5.0, W - 36.0, 15.0));
            label.setFont(Some(&medium_font(11.0)));
            label.setTextColor(Some(&color(105, 103, 99)));
            label.setAutoresizingMask(top_pin);
            root.addSubview(&label);

            let tab_count = NSTextField::labelWithString(&ns(""), mtm);
            tab_count.setFrame(rect(W - 54.0, section_y + 5.0, 34.0, 15.0));
            tab_count.setFont(Some(&font(10.0)));
            tab_count.setAlignment(NSTextAlignment::Right);
            tab_count.setTextColor(Some(&color(150, 148, 144)));
            tab_count.setAutoresizingMask(top_pin);
            root.addSubview(&tab_count);

            let scroll: Retained<NSScrollView> = msg_send![NSScrollView::alloc(mtm), initWithFrame: rect(0.0, 91.0, W, (ch - 222.0).max(10.0))];
            scroll.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            scroll.setHasVerticalScroller(true);
            scroll.setDrawsBackground(false);
            scroll.setBorderType(NSBorderType::NoBorder);
            let list = tab_list_view(mtm, rect(0.0, 0.0, W, ROW_H));
            scroll.setDocumentView(Some(&list));
            root.addSubview(&scroll);

            let new_tab_button = NSButton::buttonWithTitle_target_action(
                &ns("New tab"),
                Some(target),
                Some(sel!(newTab:)),
                mtm,
            );
            new_tab_button.setFrame(rect(18.0, 2.0, W - 36.0, ROW_H - 4.0));
            new_tab_button.setFont(Some(&font(12.0)));
            new_tab_button.setAlignment(NSTextAlignment::Left);
            if let Some(image) = symbol("plus", "New tab") {
                new_tab_button.setImage(Some(&image));
                new_tab_button.setImagePosition(NSCellImagePosition::ImageLeading);
            }
            new_tab_button.setContentTintColor(Some(&color(130, 128, 124)));
            new_tab_button.setButtonType(NSButtonType::MomentaryLight);
            new_tab_button.setBezelStyle(NSBezelStyle::Automatic);
            new_tab_button.setBordered(false);
            new_tab_button.setToolTip(Some(&ns("New tab (⌘T)")));
            list.addSubview(&new_tab_button);

            let footer_line = box_(mtm, rect(14.0, 86.0, W - 28.0, 1.0));
            footer_line.setBoxType(NSBoxType::Custom);
            footer_line.setTitlePosition(NSTitlePosition::NoTitle);
            footer_line.setFillColor(&color(229, 229, 226));
            footer_line.setBorderColor(&NSColor::clearColor());
            footer_line.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
            root.addSubview(&footer_line);

            let protection = NSButton::buttonWithTitle_target_action(
                &ns("Privacy protection"),
                Some(target),
                Some(sel!(toggleProtection:)),
                mtm,
            );
            protection.setFrame(rect(18.0, 49.0, W - 36.0, 28.0));
            protection.setFont(Some(&font(12.0)));
            protection.setAlignment(NSTextAlignment::Left);
            protection.setBordered(false);
            protection.setEnabled(false);
            protection.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
            protection.setToolTip(Some(&ns("Block ads and tracking requests")));
            root.addSubview(&protection);

            let protection_status = NSTextField::labelWithString(&ns("…"), mtm);
            protection_status.setFrame(rect(W - 68.0, 55.0, 46.0, 16.0));
            protection_status.setFont(Some(&medium_font(11.0)));
            protection_status.setAlignment(NSTextAlignment::Right);
            protection_status.setTextColor(Some(&color(130, 128, 124)));
            protection_status.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
            root.addSubview(&protection_status);

            let memory_caption = NSTextField::labelWithString(&ns("App memory"), mtm);
            memory_caption.setFrame(rect(18.0, 20.0, 120.0, 16.0));
            memory_caption.setFont(Some(&font(11.0)));
            memory_caption.setTextColor(Some(&color(130, 128, 124)));
            memory_caption.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
            root.addSubview(&memory_caption);

            let memory_label = NSTextField::labelWithString(&ns("…"), mtm);
            memory_label.setFrame(rect(W - 108.0, 20.0, 88.0, 16.0));
            memory_label.setFont(Some(&medium_font(11.0)));
            memory_label.setAlignment(NSTextAlignment::Right);
            memory_label.setTextColor(Some(&color(95, 94, 91)));
            memory_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxYMargin);
            memory_label.setToolTip(Some(&ns(
                "Resident memory used by this app process. WebKit helper processes are not included.",
            )));
            root.addSubview(&memory_label);

            let edge = box_(mtm, rect(W - 1.0, 0.0, 1.0, ch));
            edge.setBoxType(NSBoxType::Custom);
            edge.setTitlePosition(NSTitlePosition::NoTitle);
            edge.setFillColor(&NSColor::separatorColor());
            edge.setBorderColor(&NSColor::clearColor());
            edge.setAutoresizingMask(NSAutoresizingMaskOptions::ViewHeightSizable);
            root.addSubview(&edge);

            content_view.addSubview(&root);
            log::log("native sidebar built");

            let chrome = Self {
                actions,
                mouse_navigation_monitor,
                root,
                scroll,
                field,
                tab_count,
                protection,
                protection_status,
                memory_label,
                list,
                new_tab_button,
                empty_state,
                empty_title,
                empty_hint,
                empty_button,
                rows: HashMap::new(),
                last_key: String::new(),
            };
            chrome.resize_empty(cw, ch);
            chrome
        }
    }

    pub fn refresh(&mut self, tabs: &TabManager) {
        // pages fire title/page events in bursts; skip rebuilds when nothing changed
        let mut key = String::new();
        for t in &tabs.tabs {
            key.push_str(&format!("{}|{}|{}|{};", t.id, t.title, t.url, t.is_active));
        }
        if key == self.last_key {
            return;
        }
        self.last_key = key;
        unsafe {
            let mtm = MainThreadMarker::new().unwrap();
            self.empty_state.setHidden(!tabs.tabs.is_empty());
            self.tab_count
                .setStringValue(&ns(&tabs.tabs.len().to_string()));
            self.rows.retain(|id, (_, row)| {
                if tabs.has(*id) {
                    true
                } else {
                    row.removeFromSuperview();
                    false
                }
            });
            let n = tabs.tabs.len() as f64;
            let total = (n + 1.0) * ROW_H;
            self.list.setFrame(rect(0.0, 0.0, W, total.max(10.0)));

            let target: &AnyObject = &self.actions;
            for (i, t) in tabs.tabs.iter().enumerate() {
                let y = i as f64 * ROW_H;
                let row_key = format!("{}|{}|{}", t.title, t.url, t.is_active);
                if let Some((key, row)) = self.rows.get(&t.id) {
                    if *key == row_key {
                        row.setFrame(rect(0.0, y, W, ROW_H));
                        continue;
                    }
                    row.removeFromSuperview();
                }
                let row = view(mtm, rect(0.0, y, W, ROW_H));
                if t.is_active {
                    let bg = box_(mtm, rect(9.0, 2.0, W - 18.0, ROW_H - 4.0));
                    bg.setBoxType(NSBoxType::Custom);
                    bg.setTitlePosition(NSTitlePosition::NoTitle);
                    bg.setFillColor(&color(236, 236, 233));
                    bg.setBorderColor(&NSColor::clearColor());
                    bg.setCornerRadius(6.0);
                    row.addSubview(&bg);
                }

                let initial = host(&t.url)
                    .chars()
                    .find(|c| c.is_ascii_alphanumeric())
                    .unwrap_or('•')
                    .to_ascii_uppercase()
                    .to_string();
                let mark = box_(mtm, rect(18.0, 10.0, 17.0, 17.0));
                mark.setBoxType(NSBoxType::Custom);
                mark.setTitlePosition(NSTitlePosition::NoTitle);
                let mark_color = if t.is_active {
                    color(220, 220, 216)
                } else {
                    color(232, 232, 229)
                };
                mark.setFillColor(&mark_color);
                mark.setBorderColor(&NSColor::clearColor());
                mark.setCornerRadius(4.0);
                row.addSubview(&mark);
                let mark_text = NSTextField::labelWithString(&ns(&initial), mtm);
                mark_text.setFrame(rect(18.0, 10.0, 17.0, 16.0));
                mark_text.setAlignment(NSTextAlignment::Center);
                mark_text.setFont(Some(&medium_font(10.0)));
                mark_text.setTextColor(Some(&color(88, 86, 82)));
                row.addSubview(&mark_text);

                let title = NSTextField::labelWithString(&ns(&t.title), mtm);
                title.setFrame(rect(44.0, 9.0, W - 86.0, 18.0));
                let title_font = if t.is_active {
                    medium_font(12.0)
                } else {
                    font(12.0)
                };
                title.setFont(Some(&title_font));
                let title_color = if t.is_active {
                    color(45, 44, 41)
                } else {
                    color(76, 74, 69)
                };
                title.setTextColor(Some(&title_color));
                title.setMaximumNumberOfLines(1);
                if let Some(cell) = title.cell() {
                    cell.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
                }
                row.addSubview(&title);

                let open = NSButton::buttonWithTitle_target_action(
                    &ns(""),
                    Some(target),
                    Some(sel!(openTab:)),
                    mtm,
                );
                open.setFrame(rect(9.0, 0.0, W - 18.0, ROW_H));
                open.setTag(t.id as isize);
                open.setTransparent(true);
                open.setBordered(false);
                row.addSubview(&open);

                let x = NSButton::buttonWithTitle_target_action(
                    &ns("×"),
                    Some(target),
                    Some(sel!(closeTab:)),
                    mtm,
                );
                x.setFrame(rect(W - 39.0, 9.0, 24.0, 20.0));
                x.setFont(Some(&font(15.0)));
                if let Some(image) = symbol("xmark", "Close tab") {
                    x.setImage(Some(&image));
                    x.setImagePosition(NSCellImagePosition::ImageOnly);
                }
                let close_color = if t.is_active {
                    color(124, 122, 118)
                } else {
                    color(173, 171, 167)
                };
                x.setContentTintColor(Some(&close_color));
                x.setButtonType(NSButtonType::MomentaryLight);
                x.setBezelStyle(NSBezelStyle::Automatic);
                x.setBordered(false);
                x.setTag(t.id as isize);
                x.setToolTip(Some(&ns("Close tab (⌘W)")));
                row.addSubview(&x);

                row.setToolTip(Some(&ns(&format!("{}\n{}", t.title, t.url))));
                self.list.addSubview(&row);
                self.rows.insert(t.id, (row_key, row));
            }
            self.new_tab_button
                .setFrame(rect(18.0, n * ROW_H + 2.0, W - 36.0, ROW_H - 4.0));

            let active_url = tabs.active_tab().map_or("", |tab| tab.url.as_str());
            if self.field.currentEditor().is_none()
                && self.field.stringValue().to_string() != active_url
            {
                self.field.setStringValue(&ns(active_url));
            }
        }
    }

    pub fn focus_url(&self) {
        unsafe { self.field.selectText(None) };
    }

    pub fn resize_empty(&self, width: f64, height: f64) {
        let content_width = (width - W).max(100.0);
        let center_x = content_width / 2.0;
        let center_y = height / 2.0;
        self.empty_state
            .setFrame(rect(W, 0.0, content_width, height));
        self.empty_title
            .setFrame(rect(center_x - 150.0, center_y + 20.0, 300.0, 28.0));
        self.empty_hint
            .setFrame(rect(center_x - 170.0, center_y - 9.0, 340.0, 20.0));
        self.empty_button
            .setFrame(rect(center_x - 54.0, center_y - 57.0, 108.0, 30.0));
    }

    pub fn scroll_to_active_tab(&self, tabs: &TabManager) {
        if let Some(index) = tabs.tabs.iter().position(|tab| tab.id == tabs.active) {
            self.list
                .scrollRectToVisible(rect(0.0, index as f64 * ROW_H, W, ROW_H));
        }
    }

    pub fn set_protection_state(&self, enabled: Option<bool>) {
        let status = match enabled {
            Some(true) => "On",
            Some(false) => "Off",
            None => "—",
        };
        self.protection_status.setStringValue(&ns(status));
        self.protection.setEnabled(enabled.is_some());
    }

    pub fn set_memory_usage(&self, bytes: Option<u64>) {
        let text = match bytes {
            Some(bytes) if bytes >= 1_000_000_000 => {
                format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
            }
            Some(bytes) => format!("{:.0} MB", bytes as f64 / 1_000_000.0),
            None => "—".to_string(),
        };
        self.memory_label.setStringValue(&ns(&text));
    }
}

impl Drop for Chrome {
    fn drop(&mut self) {
        if let Some(monitor) = self.mouse_navigation_monitor.take() {
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
    }
}
