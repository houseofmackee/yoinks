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

/// Makes `window` show up on the desktop (Space) the user is on, also over
/// full-screen apps, instead of staying on the one where it first opened.
pub fn follow_active_space(window: &objc2_app_kit::NSWindow) {
    use objc2_app_kit::NSWindowCollectionBehavior;
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::MoveToActiveSpace
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
}

/// Brings a window meant to be on every desktop (`behavior` includes
/// `CanJoinAllSpaces`) to the one the user is on, if it is open but not on
/// screen: macOS can leave such a window tied to one desktop (and AppKit's
/// `isOnActiveSpace` then still says yes, so the window server is asked).
/// Re-applies `behavior`, then moves the window here and sets `behavior`
/// again. Returns false if the window is still not on screen.
pub fn ensure_on_active_space(
    window: &objc2_app_kit::NSWindow,
    behavior: objc2_app_kit::NSWindowCollectionBehavior,
) -> bool {
    use objc2_app_kit::NSWindowCollectionBehavior;
    let on_screen = || window_on_screen(window.windowNumber()) != Some(false);
    if !window.isVisible() || on_screen() {
        return true;
    }
    window.setCollectionBehavior(NSWindowCollectionBehavior::empty());
    window.setCollectionBehavior(behavior);
    window.orderFrontRegardless();
    if on_screen() {
        return true;
    }
    window.setCollectionBehavior(
        NSWindowCollectionBehavior::MoveToActiveSpace
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    window.orderFrontRegardless();
    window.setCollectionBehavior(behavior);
    on_screen()
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> *mut objc2::runtime::AnyObject;
}

/// Whether the window server shows window `number` on the current desktop
/// (it may be hidden behind others); `None` if it can't tell.
pub fn window_on_screen(number: isize) -> Option<bool> {
    use objc2_foundation::{NSArray, NSDictionary, NSNumber, ns_string};
    /// `kCGWindowListOptionIncludingWindow`: just that window.
    const INCLUDING_WINDOW: u32 = 1 << 3;
    let id = u32::try_from(number).ok().filter(|id| *id != 0)?;
    // SAFETY: A plain query; the returned CFArray (toll-free bridged to an
    // NSArray of dictionaries) is owned by us ("Copy" rule).
    let info: Retained<NSArray<NSDictionary<objc2_foundation::NSString, AnyObject>>> =
        unsafe { Retained::from_raw(CGWindowListCopyWindowInfo(INCLUDING_WINDOW, id).cast())? };
    let window = info.firstObject()?;
    // The key is only present for windows that are on screen.
    Some(
        window
            .objectForKey(ns_string!("kCGWindowIsOnscreen"))
            .and_then(|value| value.downcast::<NSNumber>().ok())
            .is_some_and(|value| value.boolValue()),
    )
}

/// Calls `f` whenever the user switches to another desktop. Keep the
/// returned observer to keep receiving them.
pub fn on_space_change(
    f: impl Fn() + 'static,
) -> objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_foundation::NSObjectProtocol>> {
    use block2::RcBlock;
    use objc2_app_kit::{NSWorkspace, NSWorkspaceActiveSpaceDidChangeNotification};
    let block = RcBlock::new(move |_note: std::ptr::NonNull<objc2_foundation::NSNotification>| f());
    // SAFETY: Workspace notifications are posted on the main thread; the
    // block only touches main-thread state.
    unsafe {
        NSWorkspace::sharedWorkspace()
            .notificationCenter()
            .addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceActiveSpaceDidChangeNotification),
                None,
                None,
                &block,
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_on_screen_needs_a_real_window() {
        assert_eq!(window_on_screen(0), None, "no window number");
        assert_eq!(window_on_screen(-1), None, "not a window number");
        // A window number that does not exist (the window server hands
        // them out from small numbers up).
        assert_eq!(window_on_screen(u32::MAX as isize - 1), None);
    }
}
