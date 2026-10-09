//! The menu bar icon and its menu.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::sel;
use objc2_app_kit::{
    NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{MainThreadMarker, ns_string};

use crate::ui::menu_item;

/// Builds the status item. Menu actions are sent to `target` (the app
/// delegate); items whose action `target` does not implement yet are disabled
/// automatically by AppKit's menu validation.
pub fn build(mtm: MainThreadMarker, target: &AnyObject) -> Retained<NSStatusItem> {
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

    let menu = NSMenu::new(mtm);
    // The title is updated in `validateMenuItem:` to "Show"/"Hide".
    menu.addItem(&menu_item(
        mtm,
        "Show Shelf",
        sel!(toggleShelf:),
        "",
        Some(target),
    ));
    menu.addItem(&menu_item(
        mtm,
        "Clear Shelf",
        sel!(clearShelf:),
        "",
        Some(target),
    ));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&menu_item(
        mtm,
        "Preferences…",
        sel!(showPreferences:),
        ",",
        Some(target),
    ));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    // No target: `terminate:` travels the responder chain to NSApplication.
    menu.addItem(&menu_item(mtm, "Quit Yoinks", sel!(terminate:), "q", None));
    status_item.setMenu(Some(&menu));

    status_item
}
