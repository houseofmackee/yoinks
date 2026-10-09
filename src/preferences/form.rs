//! Small layout helpers for the Preferences tabs.

use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2_app_kit::{
    NSButton, NSColor, NSFont, NSLayoutAttribute, NSPopUpButton, NSStackView, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSEdgeInsets, NSPoint, NSRect, NSSize, NSString,
};

pub const LABEL_WIDTH: f64 = 170.0;

/// A tab's content: a column of rows, aligned to the top left.
pub fn column(mtm: MainThreadMarker, rows: &[&NSView]) -> Retained<NSStackView> {
    let column = NSStackView::stackViewWithViews(&NSArray::from_slice(rows), mtm);
    column.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    column.setAlignment(NSLayoutAttribute::Leading);
    column.setSpacing(12.0);
    column.setEdgeInsets(NSEdgeInsets {
        top: 20.0,
        left: 20.0,
        bottom: 20.0,
        right: 20.0,
    });
    column
}

/// A right-aligned label followed by a control, like System Settings forms.
pub fn row(mtm: MainThreadMarker, label: &str, control: &NSView) -> Retained<NSStackView> {
    let label = NSTextField::labelWithString(&NSString::from_str(label), mtm);
    label.setAlignment(objc2_app_kit::NSTextAlignment::Right);
    label.setTranslatesAutoresizingMaskIntoConstraints(false);
    label
        .widthAnchor()
        .constraintEqualToConstant(LABEL_WIDTH)
        .setActive(true);
    let row =
        NSStackView::stackViewWithViews(&NSArray::from_slice(&[&*label as &NSView, control]), mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.setAlignment(NSLayoutAttribute::FirstBaseline);
    row.setSpacing(8.0);
    row
}

/// A control indented to line up with the controls of `row`s.
pub fn indented(mtm: MainThreadMarker, control: &NSView) -> Retained<NSStackView> {
    let spacer = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(LABEL_WIDTH, 1.0)),
    );
    spacer.setTranslatesAutoresizingMaskIntoConstraints(false);
    spacer
        .widthAnchor()
        .constraintEqualToConstant(LABEL_WIDTH)
        .setActive(true);
    let row = NSStackView::stackViewWithViews(&NSArray::from_slice(&[&*spacer, control]), mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.setSpacing(8.0);
    row
}

/// Secondary explanatory text, wrapping at `width`.
pub fn note(mtm: MainThreadMarker, text: &str, width: f64) -> Retained<NSTextField> {
    let label = NSTextField::wrappingLabelWithString(&NSString::from_str(text), mtm);
    label.setFont(Some(&NSFont::systemFontOfSize(11.0)));
    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
    label.setPreferredMaxLayoutWidth(width);
    label
}

pub fn checkbox(
    mtm: MainThreadMarker,
    title: &str,
    target: &AnyObject,
    action: Sel,
) -> Retained<NSButton> {
    // SAFETY: Every action used here takes one sender argument.
    unsafe {
        NSButton::checkboxWithTitle_target_action(
            &NSString::from_str(title),
            Some(target),
            Some(action),
            mtm,
        )
    }
}

pub fn button(
    mtm: MainThreadMarker,
    title: &str,
    target: &AnyObject,
    action: Sel,
) -> Retained<NSButton> {
    // SAFETY: Every action used here takes one sender argument.
    unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target),
            Some(action),
            mtm,
        )
    }
}

pub fn popup(
    mtm: MainThreadMarker,
    titles: &[&str],
    target: &AnyObject,
    action: Sel,
) -> Retained<NSPopUpButton> {
    let popup = NSPopUpButton::initWithFrame_pullsDown(
        NSPopUpButton::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(200.0, 26.0)),
        false,
    );
    for title in titles {
        popup.addItemWithTitle(&NSString::from_str(title));
    }
    // SAFETY: The action takes one sender argument; the target outlives it.
    unsafe {
        popup.setTarget(Some(target));
        popup.setAction(Some(action));
    }
    popup
}
