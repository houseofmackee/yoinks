//! Small AppKit helpers shared across modules.

use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2_app_kit::NSMenuItem;
use objc2_foundation::{MainThreadMarker, NSString};

/// A menu item sending `action` to `target`. With no target, the action
/// travels the responder chain.
pub fn menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Sel,
    key: &str,
    target: Option<&AnyObject>,
) -> Retained<NSMenuItem> {
    // SAFETY: Every action used in this app takes a single `sender` argument.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            Some(action),
            &NSString::from_str(key),
        )
    };
    // SAFETY: Targets are long-lived objects owned by the app (or nil).
    unsafe { item.setTarget(target) };
    item
}

/// Brings this (normally background) app to the front, for UI the user
/// explicitly asked for, such as the clipboard history or Quick Look.
pub fn activate_app(mtm: MainThreadMarker) {
    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
    if objc2::available!(macos = 14.0) {
        app.activate();
    } else {
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }
}

/// A main menu with just an Edit menu. Yoinks never shows a menu bar, but
/// text fields get Cut, Copy, Paste, Select All and Undo from the main
/// menu's key equivalents (⌘X, ⌘C, ⌘V, ⌘A, ⌘Z); without it those keys do
/// nothing in the history search, Preferences and the rename prompt.
pub fn install_edit_menu(mtm: MainThreadMarker) {
    use objc2::sel;
    use objc2_app_kit::{NSEventModifierFlags, NSMenu};

    let edit = NSMenu::new(mtm);
    edit.setTitle(&NSString::from_str("Edit"));
    let entries = [
        ("Undo", sel!(undo:), "z", false),
        ("Redo", sel!(redo:), "z", true),
        ("Cut", sel!(cut:), "x", false),
        ("Copy", sel!(copy:), "c", false),
        ("Paste", sel!(paste:), "v", false),
        ("Select All", sel!(selectAll:), "a", false),
    ];
    for (title, action, key, shift) in entries {
        // No target: each goes to the first responder (the focused field).
        let item = menu_item(mtm, title, action, key, None);
        if shift {
            item.setKeyEquivalentModifierMask(
                NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
            );
        }
        edit.addItem(&item);
    }
    let edit_item = NSMenuItem::new(mtm);
    edit_item.setSubmenu(Some(&edit));
    let main = NSMenu::new(mtm);
    // The first item is the (invisible) application menu.
    main.addItem(&NSMenuItem::new(mtm));
    main.addItem(&edit_item);
    objc2_app_kit::NSApplication::sharedApplication(mtm).setMainMenu(Some(&main));
}

/// The system color for a Finder-style color tag.
pub fn tag_color(color: crate::shelf::TagColor) -> Retained<objc2_app_kit::NSColor> {
    use crate::shelf::TagColor;
    use objc2_app_kit::NSColor;
    match color {
        TagColor::Red => NSColor::systemRedColor(),
        TagColor::Orange => NSColor::systemOrangeColor(),
        TagColor::Yellow => NSColor::systemYellowColor(),
        TagColor::Green => NSColor::systemGreenColor(),
        TagColor::Blue => NSColor::systemBlueColor(),
        TagColor::Purple => NSColor::systemPurpleColor(),
        TagColor::Gray => NSColor::systemGrayColor(),
    }
}

/// A filled circle in a tag's color, for menus.
pub fn tag_dot(color: crate::shelf::TagColor) -> Option<Retained<objc2_app_kit::NSImage>> {
    use objc2_app_kit::{NSImage, NSImageSymbolConfiguration};
    let symbol = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        objc2_foundation::ns_string!("circle.fill"),
        Some(&NSString::from_str(color.label())),
    )?;
    let palette = NSImageSymbolConfiguration::configurationWithPaletteColors(
        &objc2_foundation::NSArray::from_retained_slice(&[tag_color(color)]),
    );
    symbol.imageWithSymbolConfiguration(&palette)
}
