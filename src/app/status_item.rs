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
use crate::shelf::{ShelfInfo, TagColor};
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
    shelves: &[ShelfInfo],
    visible: bool,
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

    status_item.setMenu(Some(&menu(mtm, target, shortcut, shelves)));
    status_item.setVisible(visible);
    status_item
}

/// Replaces the menu, e.g. after the shortcut or the shelves changed.
pub fn rebuild_menu(
    mtm: MainThreadMarker,
    status_item: &NSStatusItem,
    target: &AnyObject,
    shortcut: Option<&Shortcut>,
    shelves: &[ShelfInfo],
) {
    status_item.setMenu(Some(&menu(mtm, target, shortcut, shelves)));
}

/// The app menu, shown from the menu bar icon and from the shelf's gear
/// button.
pub fn menu(
    mtm: MainThreadMarker,
    target: &AnyObject,
    shortcut: Option<&Shortcut>,
    shelves: &[ShelfInfo],
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
        ("Search Shelf", sel!(searchShelf:)),
        ("Recall Last Dragged Item", sel!(recallItems:)),
        ("Clear Shelf", sel!(clearShelf:)),
        ("Move Shelf Back to Edge", sel!(resetShelfPosition:)),
    ] {
        menu.addItem(&menu_item(mtm, title, action, "", target));
    }
    let shelves_item = NSMenuItem::new(mtm);
    shelves_item.setTitle(ns_string!("Shelves"));
    shelves_item.setSubmenu(Some(&shelves_menu(mtm, target, shelves)));
    menu.addItem(&shelves_item);
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
    // Hidden by the delegate in builds without Sparkle; retitled when a
    // scheduled check found an update.
    menu.addItem(&menu_item(
        mtm,
        "Check for Updates…",
        sel!(checkForUpdates:),
        "",
        target,
    ));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    // No target: `terminate:` travels the responder chain to NSApplication.
    menu.addItem(&menu_item(mtm, "Quit Yoinks", sel!(terminate:), "q", None));
    menu
}

/// Shelves ▸ one item per shelf (checked by the delegate for the one that
/// is showing), then New, Rename, Color and Delete for the current shelf.
fn shelves_menu(
    mtm: MainThreadMarker,
    target: Option<&AnyObject>,
    shelves: &[ShelfInfo],
) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    for (index, shelf) in shelves.iter().enumerate() {
        let key = if index < 9 {
            (index + 1).to_string()
        } else {
            String::new()
        };
        let item = menu_item(mtm, &shelf.name, sel!(switchShelf:), &key, target);
        // SAFETY: An NSString is a valid represented object.
        unsafe { item.setRepresentedObject(Some(&NSString::from_str(&shelf.id))) };
        if let Some(dot) = shelf.color.and_then(crate::ui::tag_dot) {
            item.setImage(Some(&dot));
        }
        menu.addItem(&item);
    }
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&menu_item(mtm, "New Shelf…", sel!(newShelf:), "", target));
    menu.addItem(&menu_item(
        mtm,
        "Rename Shelf…",
        sel!(renameShelf:),
        "",
        target,
    ));
    let colors = NSMenu::new(mtm);
    let none = menu_item(mtm, "None", sel!(setShelfColor:), "", target);
    none.setTag(0);
    colors.addItem(&none);
    colors.addItem(&NSMenuItem::separatorItem(mtm));
    for (index, color) in TagColor::ALL.iter().enumerate() {
        let item = menu_item(mtm, color.label(), sel!(setShelfColor:), "", target);
        item.setTag(index as isize + 1);
        if let Some(dot) = crate::ui::tag_dot(*color) {
            item.setImage(Some(&dot));
        }
        colors.addItem(&item);
    }
    let color_item = NSMenuItem::new(mtm);
    color_item.setTitle(ns_string!("Color"));
    color_item.setSubmenu(Some(&colors));
    menu.addItem(&color_item);
    menu.addItem(&menu_item(
        mtm,
        "Delete Shelf…",
        sel!(deleteShelf:),
        "",
        target,
    ));
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
