//! The sidebar's tab list. Rows are long-lived views updated in place, so
//! title and loading changes never rebuild anything; layout changes animate.

use std::cell::{Cell, OnceCell, RefCell};

use objc2::rc::{Retained, Weak};
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSBox, NSButton, NSColor, NSControlSize, NSEvent, NSImage, NSImageView, NSMenu, NSProgressIndicator,
    NSProgressIndicatorStyle, NSTextAlignment, NSTextField, NSTrackingArea, NSTrackingAreaOptions,
    NSView, NSWindowOrderingMode,
};
use objc2_foundation::{NSPoint, NSRect};

use super::kit::{
    FAST, MOVE, WEIGHT_MEDIUM, animate_frame, animate_then, fade, fill, icon_button, label, ns,
    rect, tint,
};
use super::menu::{Actions, tab_menu};
use crate::favicon::Icons;
use crate::msg::{Msg, MsgSender};
use crate::tabs::{Tab, TabManager};

pub const ROW_H: f64 = 34.0;
const DRAG_THRESHOLD: f64 = 4.0;

/// Muted system colors for the site initials shown in place of favicons.
fn site_color(site: &str) -> Retained<NSColor> {
    let palette = [
        NSColor::systemBlueColor,
        NSColor::systemIndigoColor,
        NSColor::systemPurpleColor,
        NSColor::systemPinkColor,
        NSColor::systemRedColor,
        NSColor::systemOrangeColor,
        NSColor::systemGreenColor,
        NSColor::systemTealColor,
        NSColor::systemCyanColor,
        NSColor::systemBrownColor,
    ];
    let hash = site
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    tint(palette[hash as usize % palette.len()](), 0.9)
}

fn slot(index: usize, width: f64) -> NSRect {
    rect(0.0, index as f64 * ROW_H, width, ROW_H)
}

pub struct ListIvars {
    rows: RefCell<Vec<Retained<TabRowView>>>,
    dragging: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[name = "BrowserTabList"]
    #[thread_kind = MainThreadOnly]
    #[ivars = ListIvars]
    pub struct TabListView;

    impl TabListView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

/// What a row currently shows, to skip AppKit calls when nothing changed.
#[derive(Default, PartialEq)]
struct Shown {
    title: String,
    url: String,
    site: String,
    active: bool,
    loading: bool,
    asleep: bool,
    /// Identity of the icon image shown, 0 for none.
    icon: usize,
    fresh: bool,
}

struct Parts {
    background: Retained<NSBox>,
    favicon: Retained<NSImageView>,
    tile: Retained<NSBox>,
    letter: Retained<NSTextField>,
    spinner: Retained<NSProgressIndicator>,
    title: Retained<NSTextField>,
    close: Retained<NSButton>,
}

struct Press {
    mouse_y: f64,
    row_y: f64,
}

struct RowIvars {
    id: u32,
    tx: MsgSender,
    actions: Retained<Actions>,
    list: Weak<TabListView>,
    parts: OnceCell<Parts>,
    shown: RefCell<Shown>,
    hover: Cell<bool>,
    press: RefCell<Option<Press>>,
    dragging: Cell<bool>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[name = "BrowserTabRow"]
    #[thread_kind = MainThreadOnly]
    #[ivars = RowIvars]
    struct TabRowView;

    impl TabRowView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        /// The row takes every click except those on its close button.
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
            let close = self.ivars().parts.get().map(|p| Retained::as_ptr(&p.close).cast::<NSView>());
            match hit {
                Some(view) if Some(Retained::as_ptr(&view)) == close => Some(view),
                Some(_) => {
                    let this: &NSView = self;
                    Some(this.retain())
                }
                None => None,
            }
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            self.ivars().hover.set(true);
            self.update_hover();
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.ivars().hover.set(false);
            self.update_hover();
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let Some(list) = self.ivars().list.load() else { return };
            let point = list.convertPoint_fromView(event.locationInWindow(), None);
            *self.ivars().press.borrow_mut() = Some(Press {
                mouse_y: point.y,
                row_y: self.frame().origin.y,
            });
            // Select on press, like Safari, so switching feels immediate.
            if !self.ivars().shown.borrow().active {
                let _ = self.ivars().tx.send(Msg::Switch(self.ivars().id));
            }
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.drag(event);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            self.ivars().press.borrow_mut().take();
            if self.ivars().dragging.replace(false) {
                self.drop_row();
            }
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) {
            if event.buttonNumber() == 2 {
                let _ = self.ivars().tx.send(Msg::Close(self.ivars().id));
            }
        }

        #[unsafe(method_id(menuForEvent:))]
        fn menu_for_event(&self, _event: &NSEvent) -> Option<Retained<NSMenu>> {
            let ivars = self.ivars();
            let below = ivars.list.load().is_some_and(|list| {
                let rows = list.ivars().rows.borrow();
                rows.last().is_some_and(|last| last.ivars().id != ivars.id)
            });
            let target: &AnyObject = &ivars.actions;
            Some(tab_menu(self.mtm(), target, ivars.id, below))
        }
    }
);

impl TabRowView {
    fn new(
        mtm: MainThreadMarker,
        frame: NSRect,
        id: u32,
        tx: MsgSender,
        actions: &Retained<Actions>,
        list: &TabListView,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(RowIvars {
            id,
            tx,
            actions: actions.clone(),
            list: Weak::from(list),
            parts: OnceCell::new(),
            shown: RefCell::new(Shown {
                fresh: true,
                ..Shown::default()
            }),
            hover: Cell::new(false),
            press: RefCell::new(None),
            dragging: Cell::new(false),
        });
        let row: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        let w = frame.size.width;
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                NSTrackingAreaOptions::MouseEnteredAndExited
                    | NSTrackingAreaOptions::ActiveInActiveApp
                    | NSTrackingAreaOptions::InVisibleRect,
                Some(&row),
                None,
            )
        };
        row.addTrackingArea(&area);

        let background = fill(
            mtm,
            rect(8.0, 1.0, w - 16.0, ROW_H - 2.0),
            &tint(NSColor::labelColor(), 0.05),
            8.0,
        );
        background.setAlphaValue(0.0);
        row.addSubview(&background);

        let favicon = NSImageView::new(mtm);
        favicon.setFrame(rect(18.0, 9.0, 16.0, 16.0));
        favicon.setHidden(true);
        row.addSubview(&favicon);
        let tile = fill(mtm, rect(18.0, 9.0, 16.0, 16.0), &NSColor::clearColor(), 4.0);
        row.addSubview(&tile);
        let letter = label(mtm, "", 9.5, 0.4, &NSColor::whiteColor());
        letter.setAlignment(NSTextAlignment::Center);
        letter.setFrame(rect(18.0, 10.0, 16.0, 14.0));
        row.addSubview(&letter);

        let spinner = NSProgressIndicator::new(mtm);
        spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        spinner.setControlSize(NSControlSize::Small);
        spinner.setDisplayedWhenStopped(false);
        spinner.setFrame(rect(18.0, 9.0, 16.0, 16.0));
        row.addSubview(&spinner);

        let title = label(mtm, "", 12.5, 0.0, &NSColor::labelColor());
        title.setFrame(rect(42.0, 8.0, w - 82.0, 18.0));
        row.addSubview(&title);

        let close = icon_button(mtm, actions, sel!(closeTab:), "xmark", "Close Tab (⌘W)", 9.5);
        close.setFrame(rect(w - 38.0, 7.0, 20.0, 20.0));
        close.setTag(id as isize);
        close.setHidden(true);
        row.addSubview(&close);

        let _ = row.ivars().parts.set(Parts {
            background,
            favicon,
            tile,
            letter,
            spinner,
            title,
            close,
        });
        row
    }

    fn update(&self, tab: &Tab, icon: Option<Retained<NSImage>>) {
        let Some(parts) = self.ivars().parts.get() else {
            return;
        };
        let next = Shown {
            title: tab.title.clone(),
            url: tab.url.clone(),
            site: crate::url::site(&tab.url).unwrap_or_default(),
            active: tab.is_active,
            loading: tab.loading,
            asleep: tab.asleep,
            icon: icon.as_ref().map_or(0, |i| Retained::as_ptr(i) as usize),
            fresh: false,
        };
        let mut shown = self.ivars().shown.borrow_mut();
        if *shown == next {
            return;
        }
        if shown.title != next.title {
            parts.title.setStringValue(&ns(&next.title));
        }
        if shown.title != next.title || shown.url != next.url {
            let address = crate::url::address(&next.url);
            let tip = if address.is_empty() {
                next.title.clone()
            } else {
                format!("{}\n{address}", next.title)
            };
            self.setToolTip(Some(&ns(&tip)));
        }
        if shown.site != next.site || shown.fresh {
            let initial = next
                .site
                .chars()
                .find(|c| c.is_ascii_alphanumeric())
                .map(|c| c.to_ascii_uppercase().to_string());
            let (color, text) = match initial {
                Some(letter) => (site_color(&next.site), letter),
                None => (tint(NSColor::secondaryLabelColor(), 0.5), "•".to_string()),
            };
            parts.tile.setFillColor(&color);
            parts.letter.setStringValue(&ns(&text));
        }
        if shown.icon != next.icon {
            parts.favicon.setImage(icon.as_deref());
        }
        if shown.loading != next.loading || shown.icon != next.icon || shown.fresh {
            let letter = !next.loading && next.icon == 0;
            parts.favicon.setHidden(next.loading || next.icon == 0);
            parts.tile.setHidden(!letter);
            parts.letter.setHidden(!letter);
            unsafe {
                if next.loading {
                    parts.spinner.startAnimation(None);
                } else {
                    parts.spinner.stopAnimation(None);
                }
            }
        }
        if shown.active != next.active || shown.asleep != next.asleep || shown.fresh {
            // Sleeping tabs (not loaded) read a little quieter.
            let (weight, color) = if next.active {
                (WEIGHT_MEDIUM, NSColor::labelColor())
            } else if next.asleep {
                (0.0, tint(NSColor::labelColor(), 0.55))
            } else {
                (0.0, tint(NSColor::labelColor(), 0.8))
            };
            parts.favicon.setAlphaValue(if next.asleep && !next.active { 0.6 } else { 1.0 });
            parts.title.setFont(Some(&super::kit::font(12.5, weight)));
            parts.title.setTextColor(Some(&color));
            parts.background.setFillColor(&tint(
                NSColor::labelColor(),
                if next.active { 0.1 } else { 0.05 },
            ));
        }
        let active_changed = shown.active != next.active;
        *shown = next;
        drop(shown);
        if active_changed {
            self.update_hover();
        }
    }

    fn update_hover(&self) {
        let Some(parts) = self.ivars().parts.get() else {
            return;
        };
        let active = self.ivars().shown.borrow().active;
        let lit = active || self.ivars().hover.get() || self.ivars().dragging.get();
        let target = if lit { 1.0 } else { 0.0 };
        if parts.background.alphaValue() != target {
            let background = parts.background.clone();
            super::kit::animate(FAST, move || background.animator().setAlphaValue(target));
        }
        parts.close.setHidden(!(active || self.ivars().hover.get()));
    }

    fn drag(&self, event: &NSEvent) {
        let Some(list) = self.ivars().list.load() else {
            return;
        };
        let (mouse_y, row_y) = match self.ivars().press.borrow().as_ref() {
            Some(press) => (press.mouse_y, press.row_y),
            None => return,
        };
        let y = list.convertPoint_fromView(event.locationInWindow(), None).y;
        let dy = y - mouse_y;
        if !self.ivars().dragging.get() {
            if dy.abs() < DRAG_THRESHOLD {
                return;
            }
            self.ivars().dragging.set(true);
            list.ivars().dragging.set(true);
            list.addSubview_positioned_relativeTo(self, NSWindowOrderingMode::Above, None);
            self.update_hover();
        }
        let count = list.ivars().rows.borrow().len();
        let max_y = (count.saturating_sub(1)) as f64 * ROW_H;
        let new_y = (row_y + dy).clamp(0.0, max_y);
        self.setFrameOrigin(NSPoint::new(0.0, new_y));
        list.autoscroll(event);

        // Slide the other rows out of the way.
        let target = (new_y / ROW_H).round() as usize;
        let width = list.frame().size.width;
        let rows = list.ivars().rows.borrow();
        let mut index = 0;
        for row in rows.iter() {
            if std::ptr::eq(&**row, self) {
                continue;
            }
            if index == target {
                index += 1;
            }
            animate_frame(row, slot(index, width), MOVE);
            index += 1;
        }
    }

    fn drop_row(&self) {
        let Some(list) = self.ivars().list.load() else {
            return;
        };
        list.ivars().dragging.set(false);
        let target = ((self.frame().origin.y / ROW_H).round() as usize)
            .min(list.ivars().rows.borrow().len().saturating_sub(1));
        {
            let mut rows = list.ivars().rows.borrow_mut();
            if let Some(from) = rows.iter().position(|r| std::ptr::eq(&**r, self)) {
                let row = rows.remove(from);
                rows.insert(target, row);
            }
        }
        animate_frame(self, slot(target, list.frame().size.width), MOVE);
        self.update_hover();
        let _ = self.ivars().tx.send(Msg::Move(self.ivars().id, target));
    }
}

pub struct TabList {
    pub view: Retained<TabListView>,
    new_tab: Retained<NSButton>,
    actions: Retained<Actions>,
    tx: MsgSender,
    laid_out: Cell<bool>,
}

impl TabList {
    pub fn new(
        mtm: MainThreadMarker,
        width: f64,
        new_tab: Retained<NSButton>,
        actions: Retained<Actions>,
        tx: MsgSender,
    ) -> Self {
        let this = TabListView::alloc(mtm).set_ivars(ListIvars {
            rows: RefCell::new(Vec::new()),
            dragging: Cell::new(false),
        });
        let view: Retained<TabListView> =
            unsafe { msg_send![super(this), initWithFrame: rect(0.0, 0.0, width, ROW_H)] };
        view.addSubview(&new_tab);
        Self {
            view,
            new_tab,
            actions,
            tx,
            laid_out: Cell::new(false),
        }
    }

    /// Brings rows in line with the tabs; rows that move, appear, or leave animate.
    pub fn sync(&self, tabs: &TabManager, icons: &mut Icons) {
        let mtm = self.view.mtm();
        let width = self.view.frame().size.width;
        let animated = self.laid_out.replace(true);
        let dragging = self.view.ivars().dragging.get();
        let old = std::mem::take(&mut *self.view.ivars().rows.borrow_mut());

        let mut rows = Vec::with_capacity(tabs.tabs.len());
        for (index, tab) in tabs.tabs.iter().enumerate() {
            let existing = old.iter().find(|r| r.ivars().id == tab.id).cloned();
            let row = match existing {
                Some(row) => {
                    if !dragging && !row.ivars().dragging.get() {
                        if animated {
                            animate_frame(&row, slot(index, width), MOVE);
                        } else {
                            row.setFrame(slot(index, width));
                        }
                    }
                    row
                }
                None => {
                    let row = TabRowView::new(
                        mtm,
                        slot(index, width),
                        tab.id,
                        self.tx.clone(),
                        &self.actions,
                        &self.view,
                    );
                    self.view.addSubview(&row);
                    if animated {
                        row.setAlphaValue(0.0);
                        fade(&row, true, MOVE);
                    }
                    row
                }
            };
            let icon = crate::url::site(&tab.url).and_then(|site| icons.get(&site));
            row.update(tab, icon);
            rows.push(row);
        }
        for row in old {
            if !tabs.has(row.ivars().id) {
                let gone = row.clone();
                let r = row.clone();
                animate_then(
                    FAST,
                    move || r.animator().setAlphaValue(0.0),
                    move || gone.removeFromSuperview(),
                );
            }
        }
        *self.view.ivars().rows.borrow_mut() = rows;

        let n = tabs.tabs.len();
        let button = rect(12.0, n as f64 * ROW_H + 2.0, width - 24.0, ROW_H - 4.0);
        if animated {
            animate_frame(&self.new_tab, button, MOVE);
        } else {
            self.new_tab.setFrame(button);
        }
        let height = (n as f64 + 1.0) * ROW_H + 8.0;
        let frame = self.view.frame();
        if frame.size.height != height {
            self.view.setFrameSize(objc2_foundation::NSSize::new(width, height));
        }
    }

    pub fn scroll_to(&self, tabs: &TabManager) {
        if let Some(index) = tabs.index_of(tabs.active) {
            let width = self.view.frame().size.width;
            self.view.scrollRectToVisible(slot(index, width));
        }
    }
}
