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
