//! The drop-down under the address field listing matching bookmarks and history.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{
    NSAutoresizingMaskOptions as Mask, NSBox, NSButton, NSColor, NSImage, NSImageView,
    NSTextField, NSView, NSVisualEffectMaterial, NSVisualEffectView,
};
use objc2_foundation::NSRect;

use super::kit::{FAST, WEIGHT_MEDIUM, fade, fill, label, material, ns, rect, symbol, tint};
use super::menu::Actions;
use crate::places::{SUGGESTION_LIMIT, Suggestion};

const ROW_H: f64 = 40.0;
const PAD: f64 = 5.0;
pub const WIDTH: f64 = 460.0;

struct Row {
    view: Retained<NSView>,
    highlight: Retained<NSBox>,
    icon: Retained<NSImageView>,
    title: Retained<NSTextField>,
    address: Retained<NSTextField>,
}

pub struct Suggestions {
    panel: Retained<NSVisualEffectView>,
    rows: Vec<Row>,
    urls: RefCell<Vec<String>>,
    selected: Cell<Option<usize>>,
    /// What the user typed, restored when the selection moves back off the list.
    typed: RefCell<String>,
    open: Cell<bool>,
}

impl Suggestions {
    pub fn new(mtm: MainThreadMarker, parent: &NSView, actions: &Actions) -> Self {
        let panel = material(mtm, rect(0.0, 0.0, WIDTH, ROW_H), NSVisualEffectMaterial::Menu, 10.0);
        panel.setAutoresizingMask(Mask::ViewMinYMargin);
        panel.setHidden(true);
        let rows = (0..SUGGESTION_LIMIT)
            .map(|index| {
                let view = super::kit::view(mtm, rect(0.0, 0.0, WIDTH, ROW_H));
                let highlight = fill(
                    mtm,
                    rect(PAD, 1.0, WIDTH - PAD * 2.0, ROW_H - 2.0),
                    &NSColor::selectedContentBackgroundColor(),
                    6.0,
                );
                highlight.setHidden(true);
                view.addSubview(&highlight);
                let icon = NSImageView::new(mtm);
                icon.setFrame(rect(16.0, 12.0, 16.0, 16.0));
                view.addSubview(&icon);
                let title = label(mtm, "", 13.0, WEIGHT_MEDIUM, &NSColor::labelColor());
                title.setFrame(rect(42.0, 20.0, WIDTH - 58.0, 17.0));
                view.addSubview(&title);
                let address = label(mtm, "", 11.0, 0.0, &NSColor::secondaryLabelColor());
                address.setFrame(rect(42.0, 5.0, WIDTH - 58.0, 14.0));
                view.addSubview(&address);
                let button = unsafe {
                    NSButton::buttonWithTitle_target_action(
                        &ns(""),
                        Some(actions),
                        Some(sel!(pickSuggestion:)),
                        mtm,
                    )
                };
                button.setFrame(rect(0.0, 0.0, WIDTH, ROW_H));
                button.setTransparent(true);
                button.setBordered(false);
                button.setTag(index as isize);
                view.addSubview(&button);
                panel.addSubview(&view);
                Row {
                    view,
                    highlight,
                    icon,
                    title,
                    address,
                }
            })
            .collect();
        parent.addSubview(&panel);
        Self {
            panel,
            rows,
            urls: RefCell::new(Vec::new()),
            selected: Cell::new(None),
            typed: RefCell::new(String::new()),
            open: Cell::new(false),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.get()
    }

    /// Shows `items` with the panel's top-left corner at (`x`, `top`).
    pub fn show(
        &self,
        items: &[(Suggestion, Option<Retained<NSImage>>)],
        typed: &str,
        x: f64,
        top: f64,
    ) {
        *self.typed.borrow_mut() = typed.to_string();
        self.selected.set(None);
        if items.is_empty() {
            self.hide();
            return;
        }
        let height = items.len() as f64 * ROW_H + PAD * 2.0;
        self.panel.setFrame(rect(x, top - height, WIDTH, height));
        for (index, row) in self.rows.iter().enumerate() {
            let Some((item, icon)) = items.get(index) else {
                row.view.setHidden(true);
                continue;
            };
            row.view.setHidden(false);
            row.view.setFrame(rect(0.0, height - PAD - (index + 1) as f64 * ROW_H, WIDTH, ROW_H));
            let title = if item.title.is_empty() { &item.url } else { &item.title };
            row.title.setStringValue(&ns(title));
            row.address.setStringValue(&ns(crate::url::display_address(&item.url)));
            let image = icon.clone().or_else(|| {
                let name = if item.bookmarked { "star.fill" } else { "clock" };
                symbol(name, "", 12.0, WEIGHT_MEDIUM)
            });
            row.icon.setImage(image.as_deref());
            let tint_color = if item.bookmarked && icon.is_none() {
                NSColor::systemYellowColor()
            } else {
                NSColor::secondaryLabelColor()
            };
            row.icon.setContentTintColor(Some(&tint_color));
        }
        *self.urls.borrow_mut() = items.iter().map(|(s, _)| s.url.clone()).collect();
        self.paint();
        if !self.open.replace(true) {
            fade(&self.panel, true, FAST);
        }
    }

    pub fn hide(&self) {
        if self.open.replace(false) {
            fade(&self.panel, false, FAST);
        }
        self.selected.set(None);
    }

    fn paint(&self) {
        let selected = self.selected.get();
        for (index, row) in self.rows.iter().enumerate() {
            let on = selected == Some(index);
            row.highlight.setHidden(!on);
            let (title, address) = if on {
                (NSColor::alternateSelectedControlTextColor(), tint(NSColor::alternateSelectedControlTextColor(), 0.8))
            } else {
                (NSColor::labelColor(), NSColor::secondaryLabelColor())
            };
            row.title.setTextColor(Some(&title));
            row.address.setTextColor(Some(&address));
        }
    }

    /// Moves the highlight; returns the text the address field should show.
    pub fn step(&self, delta: i8) -> Option<String> {
        if !self.open.get() {
            return None;
        }
        let count = self.urls.borrow().len();
        let next = match (self.selected.get(), delta > 0) {
            (None, true) => Some(0),
            (None, false) => Some(count - 1),
            (Some(i), true) if i + 1 < count => Some(i + 1),
            (Some(i), false) if i > 0 => Some(i - 1),
            _ => None,
        };
        self.selected.set(next);
        self.paint();
        Some(match next {
            Some(i) => self.urls.borrow()[i].clone(),
            None => self.typed.borrow().clone(),
        })
    }

    pub fn url(&self, index: usize) -> Option<String> {
        self.urls.borrow().get(index).cloned()
    }

    /// The typed text, for Escape to put back.
    pub fn typed(&self) -> String {
        self.typed.borrow().clone()
    }

    pub fn frame(&self) -> NSRect {
        self.panel.frame()
    }
}
