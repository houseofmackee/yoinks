//! The application delegate: owns the shelf and status item, and handles
//! menu actions.

use std::cell::OnceCell;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationDelegate, NSBeep, NSControlStateValueOff, NSControlStateValueOn,
    NSMenuItem, NSMenuItemValidation, NSStatusItem,
};
use objc2_foundation::{MainThreadMarker, NSNotification, ns_string};

use super::status_item;
use crate::clipboard::{ClipboardHistory, HistoryList};
use crate::drag::DragMonitor;
use crate::hotkey::{Gesture, HotKey};
use crate::paths;
use crate::settings::Settings;
use crate::shelf::{Shelf, ShelfModel};

#[derive(Default)]
pub struct Ivars {
    shelf: OnceCell<Rc<Shelf>>,
    status_item: OnceCell<Retained<NSStatusItem>>,
    drag_monitor: OnceCell<Option<Rc<DragMonitor>>>,
    hotkey: OnceCell<Rc<HotKey>>,
    clipboard: OnceCell<Rc<ClipboardHistory>>,
    history_list: OnceCell<Retained<HistoryList>>,
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
            let data_dir = paths::data_dir();
            let settings = Settings::load(&data_dir.join("settings.json"));
            let model = ShelfModel::new(mtm, &data_dir);
            model.set_keep_after_drag_out(settings.keep_after_drag_out);
            let shelf = Rc::new(Shelf::new(mtm, settings.shelf_edge, model));
            let drag_monitor = DragMonitor::start(Rc::clone(&shelf), &settings);
            let shortcut = settings.shortcut();
            if let Some(shortcut) = &shortcut {
                let target = Rc::clone(&shelf);
                match HotKey::register(shortcut, move |gesture| on_gesture(&target, gesture)) {
                    Ok(hotkey) => {
                        let _ = self.ivars().hotkey.set(hotkey);
                    }
                    Err(error) => eprintln!("yoinks: {error}"),
                }
            }
            if let Some(clipboard) = ClipboardHistory::start(&data_dir, &settings) {
                let target = Rc::clone(&shelf);
                let list = HistoryList::new(mtm, &clipboard, move |dropped| {
                    if target.model().accept(dropped) {
                        target.show();
                    } else {
                        NSBeep();
                    }
                });
                let _ = self.ivars().clipboard.set(clipboard);
                let _ = self.ivars().history_list.set(list);
            }
            let ivars = self.ivars();
            let _ = ivars.shelf.set(shelf);
            let _ = ivars.drag_monitor.set(drag_monitor);
            let _ = ivars
                .status_item
                .set(status_item::build(mtm, self, shortcut.as_ref()));
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
            let clipboard = self.ivars().clipboard.get();
            if action == Some(sel!(showClipboardHistory:))
                || action == Some(sel!(toggleClipboardPause:))
            {
                item.setHidden(clipboard.is_none());
                if action == Some(sel!(toggleClipboardPause:)) {
                    let paused = clipboard.is_some_and(|c| c.is_paused());
                    item.setState(if paused { NSControlStateValueOn } else { NSControlStateValueOff });
                }
                clipboard.is_some()
            } else if action == Some(sel!(clearShelf:)) {
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
            on_gesture(self.shelf(), Gesture::LongPress);
        }

        #[unsafe(method(addClipboard:))]
        fn add_clipboard(&self, _sender: Option<&AnyObject>) {
            on_gesture(self.shelf(), Gesture::DoublePress);
        }

        #[unsafe(method(showClipboardHistory:))]
        fn show_clipboard_history(&self, _sender: Option<&AnyObject>) {
            let mtm = self.mtm();
            let ivars = self.ivars();
            let button = ivars.status_item.get().and_then(|item| item.button(mtm));
            if let (Some(list), Some(button)) = (ivars.history_list.get(), button) {
                list.toggle(&button);
            }
        }

        #[unsafe(method(toggleClipboardPause:))]
        fn toggle_clipboard_pause(&self, _sender: Option<&AnyObject>) {
            if let Some(clipboard) = self.ivars().clipboard.get() {
                clipboard.set_paused(!clipboard.is_paused());
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

/// The shortcut's gestures, also used by the matching menu items. Beeps when
/// there is nothing to recall or nothing usable on the clipboard.
fn on_gesture(shelf: &Shelf, gesture: Gesture) {
    let done = match gesture {
        Gesture::Press => {
            shelf.toggle();
            true
        }
        Gesture::LongPress => shelf.model().recall(),
        Gesture::DoublePress => shelf.model().accept_clipboard(),
    };
    if !done {
        NSBeep();
    } else if gesture != Gesture::Press {
        shelf.show();
    }
}
