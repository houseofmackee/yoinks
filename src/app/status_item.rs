//! The menu bar icon and its menu.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::sel;
use objc2_app_kit::{
    NSEventModifierFlags, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::{MainThreadMarker, NSString, ns_string};

use crate::hotkey::Shortcut;
use crate::ui::menu_item;

/// Builds the status item. Menu actions are sent to `target` (the app
/// delegate); items whose action `target` does not implement yet are disabled
/// automatically by AppKit's menu validation.
///
/// `shortcut` is shown next to Show Shelf (it only acts while the menu is
/// open; the global shortcut itself is registered separately).
pub fn build(
    mtm: MainThreadMarker,
    target: &AnyObject,
    shortcut: Option<&Shortcut>,
) -> Retained<NSStatusItem> {
    let status_item =
        NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);

    if let Some(button) = status_item.button(mtm) {
        let icon = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            ns_string!("tray.and.arrow.down"),
            Some(ns_string!("Yoinks")),
        );
        match icon {
            Some(icon) => {
                icon.setTemplate(true);
                button.setImage(Some(&icon));
            }
            None => button.setTitle(ns_string!("Yoinks")),
        }
    }

    status_item.setMenu(Some(&menu(mtm, target, shortcut)));
    status_item
}

/// Replaces the menu, e.g. after the shortcut changed.
pub fn rebuild_menu(
    mtm: MainThreadMarker,
    status_item: &NSStatusItem,
    target: &AnyObject,
    shortcut: Option<&Shortcut>,
) {
    status_item.setMenu(Some(&menu(mtm, target, shortcut)));
}

fn menu(
    mtm: MainThreadMarker,
    target: &AnyObject,
    shortcut: Option<&Shortcut>,
) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    let target = Some(target);
    // The title is updated in `validateMenuItem:` to "Show"/"Hide".
    let toggle = menu_item(mtm, "Show Shelf", sel!(toggleShelf:), "", target);
    if let Some((shortcut, key)) = shortcut.and_then(|s| s.menu_key().map(|key| (s, key))) {
        toggle.setKeyEquivalent(&NSString::from_str(&key));
        toggle.setKeyEquivalentModifierMask(modifier_flags(shortcut));
    }
    menu.addItem(&toggle);
    for (title, action) in [
        ("Add Clipboard to Shelf", sel!(addClipboard:)),
        ("Recall Last Dragged Item", sel!(recallItems:)),
        ("Clear Shelf", sel!(clearShelf:)),
    ] {
        menu.addItem(&menu_item(mtm, title, action, "", target));
    }
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    // Both are hidden by the delegate when clipboard history is off.
    for (title, action) in [
        ("Clipboard History…", sel!(showClipboardHistory:)),
        ("Pause Clipboard History", sel!(toggleClipboardPause:)),
    ] {
        menu.addItem(&menu_item(mtm, title, action, "", target));
    }
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&menu_item(
        mtm,
        "Preferences…",
        sel!(showPreferences:),
        ",",
        target,
    ));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    // No target: `terminate:` travels the responder chain to NSApplication.
    menu.addItem(&menu_item(mtm, "Quit Yoinks", sel!(terminate:), "q", None));
    menu
}

fn modifier_flags(shortcut: &Shortcut) -> NSEventModifierFlags {
    let m = shortcut.modifiers;
    [
        (m.control, NSEventModifierFlags::Control),
        (m.option, NSEventModifierFlags::Option),
        (m.shift, NSEventModifierFlags::Shift),
        (m.command, NSEventModifierFlags::Command),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .fold(NSEventModifierFlags::empty(), |flags, (_, flag)| {
        flags | flag
    })
}
