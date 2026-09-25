//! The menu bar, the protection menu, and the object that turns AppKit
//! actions into browser messages.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSButton, NSControl, NSControlTextEditingDelegate, NSEventModifierFlags,
    NSMenu, NSMenuItem, NSSearchFieldDelegate, NSTextField, NSTextFieldDelegate, NSTextView,
};
use objc2_foundation::{NSNotification, NSPoint};

use super::kit::ns;
use crate::blocker::LISTS;
use crate::msg::{Msg, MsgSender};

pub const ADDRESS_TAG: isize = 1;
pub const FIND_TAG: isize = 2;

pub struct ActionIvars {
    tx: MsgSender,
    protection_menu: RefCell<Option<Retained<NSMenu>>>,
    /// Addresses behind the dynamic Bookmarks and History menu items, by tag.
    bookmark_urls: RefCell<Vec<String>>,
    history_urls: RefCell<Vec<String>>,
}

fn shift_held(mtm: MainThreadMarker) -> bool {
    NSApplication::sharedApplication(mtm)
        .currentEvent()
        .is_some_and(|e| e.modifierFlags().contains(NSEventModifierFlags::Shift))
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "BrowserActions"]
    #[thread_kind = MainThreadOnly]
    #[ivars = ActionIvars]
    pub struct Actions;

    impl Actions {
        #[unsafe(method(goBack:))]
        fn go_back(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::Back);
        }

        #[unsafe(method(goForward:))]
        fn go_forward(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::Fwd);
        }

        #[unsafe(method(doReload:))]
        fn do_reload(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::Reload);
        }

        #[unsafe(method(stopLoading:))]
        fn stop_loading(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::Stop);
        }

        #[unsafe(method(newTab:))]
        fn new_tab(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::New(None));
        }

        #[unsafe(method(closeCurrentTab:))]
        fn close_current_tab(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::CloseActive);
        }

        #[unsafe(method(reopenTab:))]
        fn reopen_tab(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::Reopen);
        }

        #[unsafe(method(focusAddress:))]
        fn focus_address(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::FocusAddress);
        }

        #[unsafe(method(nextTab:))]
        fn next_tab(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::NextTab);
        }

        #[unsafe(method(previousTab:))]
        fn previous_tab(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::PreviousTab);
        }

        #[unsafe(method(selectTab:))]
        fn select_tab(&self, sender: Option<&NSMenuItem>) {
            if let Some(item) = sender {
                self.send(Msg::SwitchIndex(item.tag() as u8));
            }
        }

        #[unsafe(method(showFind:))]
        fn show_find(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::ShowFind);
        }

        #[unsafe(method(findNext:))]
        fn find_next(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::FindNext(false));
        }

        #[unsafe(method(findPrevious:))]
        fn find_previous(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::FindNext(true));
        }

        #[unsafe(method(hideFind:))]
        fn hide_find(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::HideFind);
        }

        #[unsafe(method(zoomIn:))]
        fn zoom_in(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::Zoom(1));
        }

        #[unsafe(method(zoomOut:))]
        fn zoom_out(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::Zoom(-1));
        }

        #[unsafe(method(zoomReset:))]
        fn zoom_reset(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::Zoom(0));
        }

        #[unsafe(method(printPage:))]
        fn print_page(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::Print);
        }

        #[unsafe(method(toggleProtection:))]
        fn toggle_protection(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::ToggleProtection);
        }

        #[unsafe(method(toggleSiteProtection:))]
        fn toggle_site_protection(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::ToggleSiteProtection);
        }

        #[unsafe(method(toggleFilterList:))]
        fn toggle_filter_list(&self, sender: Option<&NSMenuItem>) {
            if let Some(item) = sender {
                self.send(Msg::ToggleFilterList(item.tag() as usize));
            }
        }

        #[unsafe(method(toggleGenericHiding:))]
        fn toggle_generic_hiding(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::ToggleGenericHiding);
        }

        #[unsafe(method(toggleLockdown:))]
        fn toggle_lockdown(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::ToggleLockdown);
        }

        #[unsafe(method(updateFilters:))]
        fn update_filters(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::UpdateFilters);
        }

        #[unsafe(method(editCustomFilters:))]
        fn edit_custom_filters(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::EditCustomFilters);
        }

        #[unsafe(method(showProtectionMenu:))]
        fn show_protection_menu(&self, sender: Option<&NSButton>) {
            if let (Some(button), Some(menu)) =
                (sender, self.ivars().protection_menu.borrow().as_ref())
            {
                let bounds = button.bounds();
                menu.popUpMenuPositioningItem_atLocation_inView(
                    None,
                    NSPoint::new(0.0, bounds.size.height + 4.0),
                    Some(button),
                );
            }
        }

        // Tab rows and their context menus carry the tab id in `tag`.
        #[unsafe(method(closeTab:))]
        fn close_tab(&self, sender: Option<&AnyObject>) {
            self.send_for_tag(sender, Msg::Close);
        }

        #[unsafe(method(reloadTab:))]
        fn reload_tab(&self, sender: Option<&AnyObject>) {
            self.send_for_tag(sender, Msg::ReloadTab);
        }

        #[unsafe(method(duplicateTab:))]
        fn duplicate_tab(&self, sender: Option<&AnyObject>) {
            self.send_for_tag(sender, Msg::Duplicate);
        }

        #[unsafe(method(copyTabAddress:))]
        fn copy_tab_address(&self, sender: Option<&AnyObject>) {
            self.send_for_tag(sender, Msg::CopyAddress);
        }

        #[unsafe(method(closeOtherTabs:))]
        fn close_other_tabs(&self, sender: Option<&AnyObject>) {
            self.send_for_tag(sender, Msg::CloseOthers);
        }

        #[unsafe(method(closeTabsBelow:))]
        fn close_tabs_below(&self, sender: Option<&AnyObject>) {
            self.send_for_tag(sender, Msg::CloseBelow);
        }

        #[unsafe(method(toggleBookmark:))]
        fn toggle_bookmark(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::ToggleBookmark);
        }

        #[unsafe(method(editBookmarks:))]
        fn edit_bookmarks(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::EditBookmarks);
        }

        #[unsafe(method(openBookmark:))]
        fn open_bookmark(&self, sender: Option<&NSMenuItem>) {
            let url = sender.and_then(|item| {
                self.ivars().bookmark_urls.borrow().get(item.tag() as usize).cloned()
            });
            if let Some(url) = url {
                self.send(Msg::OpenUrl(url));
            }
        }

        #[unsafe(method(openHistoryItem:))]
        fn open_history_item(&self, sender: Option<&NSMenuItem>) {
            let url = sender.and_then(|item| {
                self.ivars().history_urls.borrow().get(item.tag() as usize).cloned()
            });
            if let Some(url) = url {
                self.send(Msg::OpenUrl(url));
            }
        }

        #[unsafe(method(clearHistory:))]
        fn clear_history(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::ClearHistory);
        }

        #[unsafe(method(toggleSidebar:))]
        fn toggle_sidebar(&self, _sender: Option<&AnyObject>) {
            self.send(Msg::ToggleSidebar);
        }

        #[unsafe(method(pickSuggestion:))]
        fn pick_suggestion(&self, sender: Option<&NSButton>) {
            if let Some(button) = sender {
                self.send(Msg::PickSuggestion(button.tag() as usize));
            }
        }

        #[unsafe(method(submitURL:))]
        fn submit_url(&self, sender: Option<&NSTextField>) {
            if let Some(f) = sender {
                self.send(Msg::Nav(f.stringValue().to_string()));
            }
        }
    }

    unsafe impl NSObjectProtocol for Actions {}

    unsafe impl NSControlTextEditingDelegate for Actions {
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, notification: &NSNotification) {
            let Some(field) = notification
                .object()
                .and_then(|o| o.downcast::<NSTextField>().ok())
            else {
                return;
            };
            let text = field.stringValue().to_string();
            match field.tag() {
                FIND_TAG => self.send(Msg::Find {
                    query: text,
                    backwards: false,
                    typing: true,
                }),
                ADDRESS_TAG => self.send(Msg::AddressTyped(text)),
                _ => {}
            }
        }

        #[unsafe(method(controlTextDidEndEditing:))]
        fn control_text_did_end_editing(&self, notification: &NSNotification) {
            let address = notification
                .object()
                .and_then(|o| o.downcast::<NSTextField>().ok())
                .is_some_and(|f| f.tag() == ADDRESS_TAG);
            if address {
                self.send(Msg::AddressEnded);
            }
        }

        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn do_command(&self, control: &NSControl, _view: &NSTextView, command: Sel) -> bool {
            let mtm = self.mtm();
            match (control.tag(), command) {
                (FIND_TAG, c) if c == sel!(insertNewline:) => {
                    self.send(Msg::FindNext(shift_held(mtm)));
                    true
                }
                (FIND_TAG, c) if c == sel!(cancelOperation:) => {
                    self.send(Msg::HideFind);
                    true
                }
                (ADDRESS_TAG, c) if c == sel!(cancelOperation:) => {
                    self.send(Msg::CancelAddress);
                    true
                }
                (ADDRESS_TAG, c) if c == sel!(moveDown:) => {
                    self.send(Msg::SuggestMove(1));
                    true
                }
                (ADDRESS_TAG, c) if c == sel!(moveUp:) => {
                    self.send(Msg::SuggestMove(-1));
                    true
                }
                _ => false,
            }
        }
    }

    unsafe impl NSTextFieldDelegate for Actions {}

    unsafe impl NSSearchFieldDelegate for Actions {}
);

impl Actions {
    pub fn new(mtm: MainThreadMarker, tx: MsgSender) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ActionIvars {
            tx,
            protection_menu: RefCell::new(None),
            bookmark_urls: RefCell::new(Vec::new()),
            history_urls: RefCell::new(Vec::new()),
        });
        unsafe { msg_send![super(this), init] }
    }

    pub fn tx(&self) -> MsgSender {
        self.ivars().tx.clone()
    }

    fn send(&self, msg: Msg) {
        let _ = self.ivars().tx.send(msg);
    }

    fn send_for_tag(&self, sender: Option<&AnyObject>, msg: fn(u32) -> Msg) {
        let tag = sender.and_then(|s| {
            if let Some(item) = s.downcast_ref::<NSMenuItem>() {
                Some(item.tag())
            } else {
                s.downcast_ref::<NSControl>().map(|c| c.tag())
            }
        });
        if let Some(tag) = tag {
            self.send(msg(tag as u32));
        }
    }

    pub fn set_protection_popup(&self, menu: Retained<NSMenu>) {
        *self.ivars().protection_menu.borrow_mut() = Some(menu);
    }
}

/// Menus whose items follow browser state.
pub struct MenuBar {
    pub protection: ProtectionMenu,
    pub bookmarks: Retained<NSMenu>,
    pub history: Retained<NSMenu>,
    pub sidebar: Retained<NSMenuItem>,
}

/// Items in the Bookmarks and History menus above their dynamic entries.
pub const BOOKMARKS_FIXED: isize = 3;
pub const HISTORY_FIXED: isize = 7;

/// Replaces the dynamic items of `menu` with `entries` (title, address, icon).
pub fn fill_places(
    actions: &Actions,
    menu: &NSMenu,
    fixed: isize,
    bookmarks: bool,
    entries: &[(String, String, Option<Retained<objc2_app_kit::NSImage>>)],
) {
    while menu.numberOfItems() > fixed {
        menu.removeItemAtIndex(fixed);
    }
    let target: &AnyObject = actions;
    let action = if bookmarks {
        sel!(openBookmark:)
    } else {
        sel!(openHistoryItem:)
    };
    let mut urls = Vec::with_capacity(entries.len());
    for (index, (title, url, icon)) in entries.iter().enumerate() {
        let shown = if title.is_empty() { url } else { title };
        let shown: String = if shown.chars().count() > 60 {
            shown.chars().take(59).chain(std::iter::once('…')).collect()
        } else {
            shown.clone()
        };
        let entry = item(menu, Some(target), &shown, action, "");
        entry.setTag(index as isize);
        entry.setToolTip(Some(&ns(url)));
        if let Some(icon) = icon {
            entry.setImage(Some(icon));
        }
        urls.push(url.clone());
    }
    if entries.is_empty() {
        let empty = item(
            menu,
            None,
            if bookmarks { "No Bookmarks" } else { "No History" },
            sel!(openBookmark:),
            "",
        );
        empty.setEnabled(false);
        unsafe { empty.setAction(None) };
    }
    let store = if bookmarks {
        &actions.ivars().bookmark_urls
    } else {
        &actions.ivars().history_urls
    };
    *store.borrow_mut() = urls;
}

fn item(
    menu: &NSMenu,
    target: Option<&AnyObject>,
    title: &str,
    action: Sel,
    key: &str,
) -> Retained<NSMenuItem> {
    let item =
        unsafe { menu.addItemWithTitle_action_keyEquivalent(&ns(title), Some(action), &ns(key)) };
    if let Some(target) = target {
        unsafe { item.setTarget(Some(target)) };
    }
    item
}

fn with_mods(item: Retained<NSMenuItem>, mods: NSEventModifierFlags) -> Retained<NSMenuItem> {
    item.setKeyEquivalentModifierMask(mods);
    item
}

/// A second shortcut for an existing command, invisible in the menu.
fn hidden_alias(menu: &NSMenu, target: &AnyObject, action: Sel, key: &str, mods: NSEventModifierFlags) {
    let alias = with_mods(item(menu, Some(target), "", action, key), mods);
    alias.setHidden(true);
    alias.setAllowsKeyEquivalentWhenHidden(true);
}

fn submenu(mtm: MainThreadMarker, main: &NSMenu, title: &str) -> Retained<NSMenu> {
    let holder = NSMenuItem::new(mtm);
    holder.setTitle(&ns(title));
    let menu = NSMenu::new(mtm);
    menu.setTitle(&ns(title));
    holder.setSubmenu(Some(&menu));
    main.addItem(&holder);
    menu
}

pub struct ProtectionMenu {
    pub menu: Retained<NSMenu>,
    pub global: Retained<NSMenuItem>,
    pub site: Retained<NSMenuItem>,
    pub lists: Vec<Retained<NSMenuItem>>,
    pub generic_hiding: Retained<NSMenuItem>,
    pub lockdown: Retained<NSMenuItem>,
}

pub fn protection_menu(mtm: MainThreadMarker, target: &AnyObject) -> ProtectionMenu {
    let menu = NSMenu::new(mtm);
    menu.setTitle(&ns("Protection"));
    fill_protection_menu(mtm, &menu, target)
}

fn fill_protection_menu(mtm: MainThreadMarker, menu: &NSMenu, target: &AnyObject) -> ProtectionMenu {
    let t = Some(target);
    menu.setAutoenablesItems(false);
    let global = with_mods(
        item(menu, t, "Block Ads and Trackers", sel!(toggleProtection:), "b"),
        NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
    );
    let site = item(menu, t, "Block on This Site", sel!(toggleSiteProtection:), "");
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&NSMenuItem::sectionHeaderWithTitle(&ns("Filter Lists"), mtm));
    let lists = LISTS
        .iter()
        .enumerate()
        .map(|(index, list)| {
            let entry = item(menu, t, list.name, sel!(toggleFilterList:), "");
            entry.setTag(index as isize);
            entry
        })
        .collect();
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&NSMenuItem::sectionHeaderWithTitle(&ns("Memory"), mtm));
    let generic_hiding = item(menu, t, "Hide Ads on Every Site", sel!(toggleGenericHiding:), "");
    generic_hiding.setToolTip(Some(&ns(
        "Also hides ad placeholders using rules for all sites. Costs up to 100 MB more on heavy pages.",
    )));
    let lockdown = item(menu, t, "Lockdown Mode", sel!(toggleLockdown:), "");
    lockdown.setToolTip(Some(&ns(
        "Turns off the JavaScript compiler, WebGL, and other complex web features. Pages use far less memory and are harder to attack, but heavy web apps run slower.",
    )));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    item(menu, t, "Update Filter Lists", sel!(updateFilters:), "");
    item(menu, t, "Edit Custom Filters…", sel!(editCustomFilters:), "");
    ProtectionMenu {
        menu: menu.retain(),
        global,
        site,
        lists,
        generic_hiding,
        lockdown,
    }
}

/// Installs the menu bar; returns the menus whose items follow browser state.
pub fn install(mtm: MainThreadMarker, target: &AnyObject) -> MenuBar {
    let t = Some(target);
    let cmd = NSEventModifierFlags::Command;
    let shift = NSEventModifierFlags::Shift;
    let option = NSEventModifierFlags::Option;
    let control = NSEventModifierFlags::Control;
    let main = NSMenu::new(mtm);
    let app = NSApplication::sharedApplication(mtm);

    let app_menu = submenu(mtm, &main, "Browser");
    item(&app_menu, None, "About Browser", sel!(orderFrontStandardAboutPanel:), "");
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    let services = submenu(mtm, &app_menu, "Services");
    app.setServicesMenu(Some(&services));
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    item(&app_menu, None, "Hide Browser", sel!(hide:), "h");
    with_mods(
        item(&app_menu, None, "Hide Others", sel!(hideOtherApplications:), "h"),
        cmd | option,
    );
    item(&app_menu, None, "Show All", sel!(unhideAllApplications:), "");
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    item(&app_menu, None, "Quit Browser", sel!(terminate:), "q");

    let file = submenu(mtm, &main, "File");
    item(&file, t, "New Tab", sel!(newTab:), "t");
    item(&file, t, "Open Location…", sel!(focusAddress:), "l");
    file.addItem(&NSMenuItem::separatorItem(mtm));
    item(&file, t, "Close Tab", sel!(closeCurrentTab:), "w");
    file.addItem(&NSMenuItem::separatorItem(mtm));
    item(&file, t, "Print…", sel!(printPage:), "p");

    let edit = submenu(mtm, &main, "Edit");
    item(&edit, None, "Undo", sel!(undo:), "z");
    with_mods(item(&edit, None, "Redo", sel!(redo:), "z"), cmd | shift);
    edit.addItem(&NSMenuItem::separatorItem(mtm));
    item(&edit, None, "Cut", sel!(cut:), "x");
    item(&edit, None, "Copy", sel!(copy:), "c");
    item(&edit, None, "Paste", sel!(paste:), "v");
    with_mods(
        item(&edit, None, "Paste and Match Style", sel!(pasteAsPlainText:), "v"),
        cmd | shift | option,
    );
    item(&edit, None, "Select All", sel!(selectAll:), "a");
    edit.addItem(&NSMenuItem::separatorItem(mtm));
    let find = submenu(mtm, &edit, "Find");
    item(&find, t, "Find…", sel!(showFind:), "f");
    item(&find, t, "Find Next", sel!(findNext:), "g");
    with_mods(item(&find, t, "Find Previous", sel!(findPrevious:), "g"), cmd | shift);

    let view = submenu(mtm, &main, "View");
    item(&view, t, "Reload Page", sel!(doReload:), "r");
    item(&view, t, "Stop", sel!(stopLoading:), ".");
    view.addItem(&NSMenuItem::separatorItem(mtm));
    item(&view, t, "Actual Size", sel!(zoomReset:), "0");
    item(&view, t, "Zoom In", sel!(zoomIn:), "+");
    hidden_alias(&view, target, sel!(zoomIn:), "=", cmd);
    item(&view, t, "Zoom Out", sel!(zoomOut:), "-");
    view.addItem(&NSMenuItem::separatorItem(mtm));
    let sidebar = with_mods(item(&view, t, "Hide Sidebar", sel!(toggleSidebar:), "s"), cmd | control);
    with_mods(
        item(&view, None, "Enter Full Screen", sel!(toggleFullScreen:), "f"),
        cmd | control,
    );

    let history = submenu(mtm, &main, "History");
    item(&history, t, "Back", sel!(goBack:), "[");
    item(&history, t, "Forward", sel!(goForward:), "]");
    history.addItem(&NSMenuItem::separatorItem(mtm));
    with_mods(item(&history, t, "Reopen Closed Tab", sel!(reopenTab:), "t"), cmd | shift);
    item(&history, t, "Clear History", sel!(clearHistory:), "");
    history.addItem(&NSMenuItem::separatorItem(mtm));
    history.addItem(&NSMenuItem::sectionHeaderWithTitle(&ns("Recently Visited"), mtm));

    let bookmarks = submenu(mtm, &main, "Bookmarks");
    item(&bookmarks, t, "Bookmark This Page", sel!(toggleBookmark:), "d");
    item(&bookmarks, t, "Edit Bookmarks…", sel!(editBookmarks:), "");
    bookmarks.addItem(&NSMenuItem::separatorItem(mtm));

    let protection = submenu(mtm, &main, "Protection");
    let protection = fill_protection_menu(mtm, &protection, target);

    let tabs = submenu(mtm, &main, "Tabs");
    with_mods(item(&tabs, t, "Show Next Tab", sel!(nextTab:), "\t"), control);
    with_mods(
        item(&tabs, t, "Show Previous Tab", sel!(previousTab:), "\t"),
        control | shift,
    );
    hidden_alias(&tabs, target, sel!(nextTab:), "]", cmd | shift);
    hidden_alias(&tabs, target, sel!(previousTab:), "[", cmd | shift);
    tabs.addItem(&NSMenuItem::separatorItem(mtm));
    for index in 1..=9 {
        let title = if index == 9 {
            "Last Tab".to_string()
        } else {
            format!("Tab {index}")
        };
        item(&tabs, t, &title, sel!(selectTab:), &index.to_string()).setTag(index);
    }

    let window = submenu(mtm, &main, "Window");
    item(&window, None, "Minimize", sel!(performMiniaturize:), "m");
    item(&window, None, "Zoom", sel!(performZoom:), "");
    window.addItem(&NSMenuItem::separatorItem(mtm));
    item(&window, None, "Bring All to Front", sel!(arrangeInFront:), "");
    app.setWindowsMenu(Some(&window));

    app.setMainMenu(Some(&main));
    MenuBar {
        protection,
        bookmarks,
        history,
        sidebar,
    }
}

/// Right-click menu for a tab row.
pub fn tab_menu(mtm: MainThreadMarker, target: &AnyObject, id: u32, can_close_below: bool) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    let t = Some(target);
    let entries: [(&str, Sel); 3] = [
        ("Reload Tab", sel!(reloadTab:)),
        ("Duplicate Tab", sel!(duplicateTab:)),
        ("Copy Address", sel!(copyTabAddress:)),
    ];
    for (title, action) in entries {
        item(&menu, t, title, action, "").setTag(id as isize);
    }
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    item(&menu, t, "Close Tab", sel!(closeTab:), "").setTag(id as isize);
    item(&menu, t, "Close Other Tabs", sel!(closeOtherTabs:), "").setTag(id as isize);
    let below = item(&menu, t, "Close Tabs Below", sel!(closeTabsBelow:), "");
    below.setTag(id as isize);
    below.setEnabled(can_close_below);
    menu
}
