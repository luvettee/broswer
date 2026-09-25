//! Views that float over the page: load progress, link status, find bar,
//! and the empty-window placeholder.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly, msg_send, sel};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAutoresizingMaskOptions as Mask, NSBezelStyle, NSBox, NSButton, NSColor, NSControlSize,
    NSSearchField, NSTextAlignment, NSTextField, NSView, NSVisualEffectMaterial,
    NSVisualEffectView,
};
use objc2_foundation::NSSize;

use super::kit::{
    FAST, MOVE, WEIGHT_SEMIBOLD, animate, animate_then, fade, fill, font, icon_button, label,
    material, ns, rect,
};
use super::menu::{Actions, FIND_TAG};

const PROGRESS_H: f64 = 2.5;
const STATUS_H: f64 = 22.0;
const FIND_W: f64 = 320.0;
const FIND_H: f64 = 38.0;

pub struct Progress {
    bar: Retained<NSBox>,
    shown: Cell<bool>,
    width: Cell<f64>,
}

impl Progress {
    /// A thin accent-colored bar along the top edge of the page.
    pub fn new(mtm: MainThreadMarker, parent: &NSView) -> Self {
        let size = parent.frame().size;
        let bar = fill(
            mtm,
            rect(0.0, size.height - PROGRESS_H, 0.0, PROGRESS_H),
            &NSColor::controlAccentColor(),
            0.0,
        );
        bar.setAutoresizingMask(Mask::ViewMinYMargin);
        bar.setHidden(true);
        parent.addSubview(&bar);
        Self {
            bar,
            shown: Cell::new(false),
            width: Cell::new(0.0),
        }
    }

    pub fn set(&self, loading: bool, progress: f64) {
        let Some(parent) = (unsafe { self.bar.superview() }) else {
            return;
        };
        let size = parent.frame().size;
        let y = size.height - PROGRESS_H;
        if loading {
            if !self.shown.replace(true) {
                self.width.set(0.0);
                self.bar.setFrame(rect(0.0, y, 0.0, PROGRESS_H));
                self.bar.setAlphaValue(1.0);
                self.bar.setHidden(false);
            }
            // Never stall at zero: start visibly, then follow WebKit's estimate.
            let width = size.width * progress.clamp(0.08, 1.0);
            if self.width.replace(width) == width {
                return;
            }
            let bar = self.bar.clone();
            animate(MOVE, move || bar.animator().setFrame(rect(0.0, y, width, PROGRESS_H)));
        } else if self.shown.replace(false) {
            self.width.set(0.0);
            let bar = self.bar.clone();
            let done = self.bar.clone();
            let width = size.width;
            animate_then(
                FAST,
                move || bar.animator().setFrame(rect(0.0, y, width, PROGRESS_H)),
                move || {
                    let out = done.clone();
                    let hide = done.clone();
                    animate_then(
                        MOVE,
                        move || out.animator().setAlphaValue(0.0),
                        move || {
                            if hide.alphaValue() == 0.0 {
                                hide.setHidden(true);
                            }
                        },
                    );
                },
            );
        }
    }

    pub fn reset(&self) {
        self.shown.set(false);
        self.width.set(0.0);
        self.bar.setHidden(true);
    }
}

pub struct Status {
    panel: Retained<NSVisualEffectView>,
    text: Retained<NSTextField>,
}

impl Status {
    /// Shows the address of the link under the pointer, bottom left.
    pub fn new(mtm: MainThreadMarker, parent: &NSView) -> Self {
        let panel = material(
            mtm,
            rect(6.0, 6.0, 200.0, STATUS_H),
            NSVisualEffectMaterial::Popover,
            6.0,
        );
        panel.setAutoresizingMask(Mask::ViewMaxYMargin | Mask::ViewMaxXMargin);
        panel.setHidden(true);
        let text = label(mtm, "", 11.5, 0.0, &NSColor::secondaryLabelColor());
        text.setFrame(rect(8.0, 3.0, 184.0, 16.0));
        text.setAutoresizingMask(Mask::ViewWidthSizable);
        panel.addSubview(&text);
        parent.addSubview(&panel);
        Self { panel, text }
    }

    pub fn set(&self, link: &str) {
        if link.is_empty() {
            fade(&self.panel, false, FAST);
            return;
        }
        let shown = link.strip_prefix("https://").unwrap_or(link);
        self.text.setStringValue(&ns(shown));
        let max = unsafe { self.panel.superview() }
            .map_or(400.0, |p| (p.frame().size.width * 0.6).max(160.0));
        let width = (self.text.fittingSize().width + 16.0).clamp(60.0, max);
        self.panel.setFrameSize(NSSize::new(width, STATUS_H));
        fade(&self.panel, true, FAST);
    }
}

pub struct FindBar {
    panel: Retained<NSVisualEffectView>,
    pub field: Retained<NSSearchField>,
    result: Retained<NSTextField>,
    open: Cell<bool>,
}

impl FindBar {
    pub fn new(mtm: MainThreadMarker, parent: &NSView, actions: &Actions) -> Self {
        let size = parent.frame().size;
        let panel = material(
            mtm,
            rect(
                size.width - FIND_W - 12.0,
                size.height - FIND_H - 12.0,
                FIND_W,
                FIND_H,
            ),
            NSVisualEffectMaterial::Popover,
            10.0,
        );
        panel.setAutoresizingMask(Mask::ViewMinXMargin | Mask::ViewMinYMargin);
        panel.setHidden(true);

        let field: Retained<NSSearchField> = unsafe {
            msg_send![NSSearchField::alloc(mtm), initWithFrame: rect(8.0, 8.0, 170.0, 22.0)]
        };
        field.setPlaceholderString(Some(&ns("Find on page")));
        field.setControlSize(NSControlSize::Small);
        field.setFont(Some(&font(12.0, 0.0)));
        field.setTag(FIND_TAG);
        unsafe { field.setDelegate(Some(ProtocolObject::from_ref(actions))) };
        panel.addSubview(&field);

        let result = label(mtm, "", 11.0, 0.0, &NSColor::secondaryLabelColor());
        result.setFrame(rect(182.0, 11.0, 66.0, 15.0));
        result.setAlignment(NSTextAlignment::Right);
        panel.addSubview(&result);

        let target: &AnyObject = actions;
        let previous = icon_button(
            mtm,
            target,
            sel!(findPrevious:),
            "chevron.up",
            "Previous (⇧⌘G)",
            11.0,
        );
        let next = icon_button(mtm, target, sel!(findNext:), "chevron.down", "Next (⌘G)", 11.0);
        let done = icon_button(mtm, target, sel!(hideFind:), "xmark", "Done (Esc)", 10.0);
        for (i, b) in [&previous, &next, &done].into_iter().enumerate() {
            b.setFrame(rect(252.0 + i as f64 * 22.0, 8.0, 22.0, 22.0));
            panel.addSubview(b);
        }
        parent.addSubview(&panel);
        Self {
            panel,
            field,
            result,
            open: Cell::new(false),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.get()
    }

    pub fn query(&self) -> String {
        self.field.stringValue().to_string()
    }

    pub fn show(&self) {
        self.open.set(true);
        fade(&self.panel, true, FAST);
        unsafe { self.field.selectText(None) };
    }

    pub fn hide(&self) {
        self.open.set(false);
        self.result.setStringValue(&ns(""));
        fade(&self.panel, false, FAST);
    }

    pub fn set_found(&self, found: Option<bool>) {
        let (text, color) = match found {
            Some(false) => ("Not found", NSColor::systemRedColor()),
            _ => ("", NSColor::secondaryLabelColor()),
        };
        self.result.setStringValue(&ns(text));
        self.result.setTextColor(Some(&color));
    }
}

pub struct Empty {
    pub view: Retained<NSView>,
}

impl Empty {
    /// Shown when every tab is closed.
    pub fn new(mtm: MainThreadMarker, parent: &NSView, actions: &Actions) -> Self {
        let size = parent.frame().size;
        let view = super::kit::view(mtm, rect(0.0, 0.0, size.width, size.height));
        view.setAutoresizingMask(Mask::ViewWidthSizable | Mask::ViewHeightSizable);
        let background = fill(
            mtm,
            rect(0.0, 0.0, size.width, size.height),
            &NSColor::windowBackgroundColor(),
            0.0,
        );
        background.setAutoresizingMask(Mask::ViewWidthSizable | Mask::ViewHeightSizable);
        view.addSubview(&background);

        let (cx, cy) = (size.width / 2.0, size.height / 2.0);
        let centered = Mask::ViewMinXMargin
            | Mask::ViewMaxXMargin
            | Mask::ViewMinYMargin
            | Mask::ViewMaxYMargin;
        let title = label(mtm, "No tabs open", 20.0, WEIGHT_SEMIBOLD, &NSColor::labelColor());
        title.setAlignment(NSTextAlignment::Center);
        title.setFrame(rect(cx - 150.0, cy + 22.0, 300.0, 28.0));
        let hint = label(
            mtm,
            "Press ⌘T to open a tab, or ⇧⌘T to reopen the last one.",
            13.0,
            0.0,
            &NSColor::secondaryLabelColor(),
        );
        hint.setAlignment(NSTextAlignment::Center);
        hint.setFrame(rect(cx - 200.0, cy - 4.0, 400.0, 20.0));
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &ns("New Tab"),
                Some(actions),
                Some(sel!(newTab:)),
                mtm,
            )
        };
        button.setBezelStyle(NSBezelStyle::Push);
        button.setControlSize(NSControlSize::Large);
        button.setKeyEquivalent(&ns("\r"));
        button.setFrame(rect(cx - 60.0, cy - 52.0, 120.0, 32.0));
        for v in [&*title as &NSView, &*hint, &*button] {
            v.setAutoresizingMask(centered);
            view.addSubview(v);
        }
        view.setHidden(true);
        parent.addSubview(&view);
        Self { view }
    }
}
