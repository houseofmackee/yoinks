//! The application delegate: owns the shelf and status item, and handles
//! menu actions.

use std::cell::OnceCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationDelegate, NSMenuItem, NSMenuItemValidation, NSStatusItem,
};
use objc2_foundation::{MainThreadMarker, NSNotification, ns_string};

use super::status_item;
use crate::paths;
use crate::shelf::{Shelf, ShelfEdge, ShelfModel};

#[derive(Default)]
pub struct Ivars {
    shelf: OnceCell<Shelf>,
    status_item: OnceCell<Retained<NSStatusItem>>,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - `AppDelegate` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "YoinksAppDelegate"]
    #[ivars = Ivars]
    pub struct AppDelegate;

    // SAFETY: `NSObjectProtocol` has no safety requirements.
    unsafe impl NSObjectProtocol for AppDelegate {}

    // SAFETY: `NSApplicationDelegate` has no safety requirements.
    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            let mtm = self.mtm();
            // TODO(phase 8): read the edge from settings instead of the environment.
            let edge = match std::env::var("YOINKS_SHELF_EDGE").as_deref() {
                Ok("left") => ShelfEdge::Left,
                _ => ShelfEdge::default(),
            };
            let model = ShelfModel::new(mtm, &paths::data_dir());
            // TODO(phase 8): read from settings instead of the environment.
            model.set_keep_after_drag_out(
                std::env::var_os("YOINKS_KEEP_AFTER_DRAG_OUT").is_some_and(|v| v == "1"),
            );
            let _ = self.ivars().shelf.set(Shelf::new(mtm, edge, model));
            let _ = self.ivars().status_item.set(status_item::build(mtm, self));
        }

        // Launching the app again (Finder, Spotlight, `open`) shows the shelf.
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn should_handle_reopen(&self, _app: &NSApplication, _has_visible_windows: bool) -> bool {
            self.shelf().show();
            false
        }
    }

    // SAFETY: `NSMenuItemValidation` has no safety requirements.
    unsafe impl NSMenuItemValidation for AppDelegate {
        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            let action = item.action();
            if action == Some(sel!(toggleShelf:)) {
                item.setTitle(if self.shelf().is_visible() {
                    ns_string!("Hide Shelf")
                } else {
                    ns_string!("Show Shelf")
                });
            }
            let model = self.shelf().model();
            if action == Some(sel!(clearShelf:)) {
                !model.is_empty()
            } else if action == Some(sel!(recallItems:)) {
                model.can_recall()
            } else {
                true
            }
        }
    }

    // Menu and responder-chain actions.
    impl AppDelegate {
        #[unsafe(method(toggleShelf:))]
        fn toggle_shelf(&self, _sender: Option<&AnyObject>) {
            self.shelf().toggle();
        }

        #[unsafe(method(clearShelf:))]
        fn clear_shelf(&self, _sender: Option<&AnyObject>) {
            self.shelf().model().clear();
        }

        #[unsafe(method(recallItems:))]
        fn recall_items(&self, _sender: Option<&AnyObject>) {
            if self.shelf().model().recall() {
                self.shelf().show();
            }
        }

        #[unsafe(method(hideShelf:))]
        fn hide_shelf(&self, _sender: Option<&AnyObject>) {
            self.shelf().hide();
        }
    }
);

impl AppDelegate {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars::default());
        // SAFETY: NSObject's `init` has the correct signature.
        unsafe { msg_send![super(this), init] }
    }

    fn shelf(&self) -> &Shelf {
        self.ivars()
            .shelf
            .get()
            .expect("shelf is created in applicationDidFinishLaunching")
    }
}
