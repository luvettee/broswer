//! Small AppKit helpers shared by the sidebar and the content overlays.

use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{MainThreadMarker, MainThreadOnly, Message, msg_send};
use objc2_app_kit::{
    NSAnimationContext, NSAnimatablePropertyContainer, NSBezelStyle, NSBox, NSBoxType, NSButton,
    NSButtonType, NSCellImagePosition, NSColor, NSFont, NSImage, NSImageSymbolConfiguration,
    NSLineBreakMode, NSTextField, NSTitlePosition, NSView, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

pub const WEIGHT_MEDIUM: f64 = 0.23;
pub const WEIGHT_SEMIBOLD: f64 = 0.3;

/// Standard durations, kept short so the interface never waits on itself.
pub const FAST: f64 = 0.12;
pub const MOVE: f64 = 0.2;

pub fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

pub fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

pub fn font(size: f64, weight: f64) -> Retained<NSFont> {
    NSFont::systemFontOfSize_weight(size, weight)
}

/// A system color at reduced opacity; stays dynamic across light and dark mode.
pub fn tint(base: Retained<NSColor>, alpha: f64) -> Retained<NSColor> {
    base.colorWithAlphaComponent(alpha)
}

pub fn symbol(name: &str, description: &str, size: f64, weight: f64) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &ns(name),
        Some(&ns(description)),
    )?;
    let config = NSImageSymbolConfiguration::configurationWithPointSize_weight(size, weight);
    image.imageWithSymbolConfiguration(&config)
}

pub fn view(mtm: MainThreadMarker, frame: NSRect) -> Retained<NSView> {
    unsafe { msg_send![NSView::alloc(mtm), initWithFrame: frame] }
}

pub fn fill(mtm: MainThreadMarker, frame: NSRect, color: &NSColor, radius: f64) -> Retained<NSBox> {
    let b: Retained<NSBox> = unsafe { msg_send![NSBox::alloc(mtm), initWithFrame: frame] };
    b.setBoxType(NSBoxType::Custom);
    b.setTitlePosition(NSTitlePosition::NoTitle);
    b.setFillColor(color);
    b.setBorderColor(&NSColor::clearColor());
    b.setBorderWidth(0.0);
    b.setCornerRadius(radius);
    b.setContentViewMargins(NSSize::new(0.0, 0.0));
    b
}

/// A floating translucent panel, like the ones Safari uses for its status bar.
pub fn material(
    mtm: MainThreadMarker,
    frame: NSRect,
    material: NSVisualEffectMaterial,
    radius: f64,
) -> Retained<NSVisualEffectView> {
    let v: Retained<NSVisualEffectView> =
        unsafe { msg_send![NSVisualEffectView::alloc(mtm), initWithFrame: frame] };
    v.setMaterial(material);
    v.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
    v.setState(NSVisualEffectState::Active);
    v.setWantsLayer(true);
    if radius > 0.0
        && let Some(layer) = v.layer()
    {
        layer.setCornerRadius(radius);
        layer.setMasksToBounds(true);
        layer.setBorderWidth(0.5);
        let border = tint(NSColor::labelColor(), 0.12);
        layer.setBorderColor(Some(&border.CGColor()));
    }
    v
}

pub fn label(
    mtm: MainThreadMarker,
    text: &str,
    size: f64,
    weight: f64,
    color: &NSColor,
) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&ns(text), mtm);
    l.setFont(Some(&font(size, weight)));
    l.setTextColor(Some(color));
    l.setMaximumNumberOfLines(1);
    if let Some(cell) = l.cell() {
        cell.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    }
    l
}

pub fn icon_button(
    mtm: MainThreadMarker,
    target: &AnyObject,
    action: Sel,
    icon: &str,
    tip: &str,
    size: f64,
) -> Retained<NSButton> {
    let b = unsafe {
        NSButton::buttonWithTitle_target_action(&ns(""), Some(target), Some(action), mtm)
    };
    if let Some(image) = symbol(icon, tip, size, WEIGHT_MEDIUM) {
        b.setImage(Some(&image));
    }
    b.setImagePosition(NSCellImagePosition::ImageOnly);
    b.setButtonType(NSButtonType::MomentaryChange);
    b.setBezelStyle(NSBezelStyle::Automatic);
    b.setBordered(false);
    b.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    b.setToolTip(Some(&ns(tip)));
    b
}

/// Runs `changes` inside an animation group; use `view.animator()` inside it.
pub fn animate(duration: f64, changes: impl Fn() + 'static) {
    let block = RcBlock::new(move |context: NonNull<NSAnimationContext>| {
        let context = unsafe { context.as_ref() };
        context.setDuration(duration);
        context.setAllowsImplicitAnimation(true);
        changes();
    });
    NSAnimationContext::runAnimationGroup(&block);
}

/// Like [`animate`], then runs `done` once the animation finishes.
pub fn animate_then(duration: f64, changes: impl Fn() + 'static, done: impl Fn() + 'static) {
    let block = RcBlock::new(move |context: NonNull<NSAnimationContext>| {
        let context = unsafe { context.as_ref() };
        context.setDuration(duration);
        context.setAllowsImplicitAnimation(true);
        changes();
    });
    let done = RcBlock::new(done);
    NSAnimationContext::runAnimationGroup_completionHandler(&block, Some(&done));
}

/// Fades a view in or out; hidden views stop taking clicks and hit tests.
pub fn fade(view: &NSView, visible: bool, duration: f64) {
    let showing = !view.isHidden() && view.alphaValue() > 0.0;
    if visible {
        if view.isHidden() {
            view.setAlphaValue(0.0);
            view.setHidden(false);
        }
        let v = view.retain();
        animate(duration, move || v.animator().setAlphaValue(1.0));
    } else if showing {
        let v = view.retain();
        let hide = view.retain();
        animate_then(
            duration,
            move || v.animator().setAlphaValue(0.0),
            move || {
                // A later fade-in may have started; only hide if still transparent.
                if hide.alphaValue() == 0.0 {
                    hide.setHidden(true);
                }
            },
        );
    }
}

pub fn animate_frame(view: &NSView, frame: NSRect, duration: f64) {
    let current = view.frame();
    if current == frame {
        return;
    }
    let v = view.retain();
    animate(duration, move || v.animator().setFrame(frame));
}
