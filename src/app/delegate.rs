//! The application delegate: owns the shelf and status item, and handles
//! menu actions.

use std::cell::{OnceCell, RefCell};
use std::path::PathBuf;
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
use crate::preferences::{PreferencesHooks, PreferencesWindow};
use crate::settings::Settings;
use crate::shelf::{Shelf, ShelfModel};

#[derive(Default)]
pub struct Ivars {
    data_dir: OnceCell<PathBuf>,
    /// The settings in effect; Preferences changes go through
    /// `apply_settings`.
    settings: RefCell<Settings>,
    shelf: OnceCell<Rc<Shelf>>,
    status_item: OnceCell<Retained<NSStatusItem>>,
    drag_monitor: OnceCell<Rc<DragMonitor>>,
    hotkey: RefCell<Option<Rc<HotKey>>>,
    clipboard: OnceCell<Rc<ClipboardHistory>>,
    history_list: OnceCell<Retained<HistoryList>>,
    preferences: OnceCell<Retained<PreferencesWindow>>,
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
            let clipboard = ClipboardHistory::start(&data_dir, &settings);
            let target = Rc::clone(&shelf);
            let list = HistoryList::new(mtm, &clipboard, move |dropped| {
                if target.model().accept(dropped) {
                    target.show();
                } else {
                    NSBeep();
                }
            });

            let ivars = self.ivars();
            let _ = ivars.data_dir.set(data_dir);
            *ivars.settings.borrow_mut() = settings;
            let _ = ivars.shelf.set(shelf);
            let _ = ivars.drag_monitor.set(drag_monitor);
            let _ = ivars.clipboard.set(clipboard);
            let _ = ivars.history_list.set(list);
            if let Err(error) = self.register_hotkey() {
                eprintln!("yoinks: {error}");
            }
            let shortcut = ivars.settings.borrow().shortcut();
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
            let clipboard = self.ivars().clipboard.get().filter(|c| c.is_enabled());
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

        #[unsafe(method(showPreferences:))]
        fn show_preferences(&self, _sender: Option<&AnyObject>) {
            let window = self.ivars().preferences.get_or_init(|| self.make_preferences());
            window.show();
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

    fn shelf(&self) -> &Rc<Shelf> {
        self.ivars()
            .shelf
            .get()
            .expect("shelf is created in applicationDidFinishLaunching")
    }

    /// (Re)registers the shortcut from the current settings, replacing any
    /// previous one. A `null` hotkey just unregisters.
    fn register_hotkey(&self) -> Result<(), String> {
        let ivars = self.ivars();
        // Drop the old registration first so the same keys can be reused.
        ivars.hotkey.borrow_mut().take();
        let Some(shortcut) = ivars.settings.borrow().shortcut() else {
            return Ok(());
        };
        let shelf = Rc::clone(self.shelf());
        let hotkey = HotKey::register(&shortcut, move |gesture| on_gesture(&shelf, gesture))?;
        *ivars.hotkey.borrow_mut() = Some(hotkey);
        Ok(())
    }

    /// Saves `settings` and applies them to everything that is running.
    /// Returns an error message if the shortcut could not be registered.
    fn apply_settings(&self, settings: Settings) -> Result<(), String> {
        let ivars = self.ivars();
        let old = ivars.settings.replace(settings.clone());
        if let Some(dir) = ivars.data_dir.get()
            && let Err(error) = settings.save(&dir.join("settings.json"))
        {
            eprintln!("yoinks: could not save settings: {error}");
        }
        let shelf = self.shelf();
        shelf.set_edge(settings.shelf_edge);
        shelf
            .model()
            .set_keep_after_drag_out(settings.keep_after_drag_out);
        if let Some(monitor) = ivars.drag_monitor.get() {
            monitor.update(&settings);
        }
        if let Some(clipboard) = ivars.clipboard.get() {
            clipboard.apply(&settings);
        }
        if old.hotkey == settings.hotkey {
            return Ok(());
        }
        let result = self.register_hotkey();
        if let Some(item) = ivars.status_item.get() {
            status_item::rebuild_menu(self.mtm(), item, self, settings.shortcut().as_ref());
        }
        result
    }

    fn make_preferences(&self) -> Retained<PreferencesWindow> {
        let mtm = self.mtm();
        let ivars = self.ivars();
        let weak = objc2::rc::Weak::new(self);
        let apply = {
            let weak = weak.clone();
            move |settings: Settings| match weak.load() {
                Some(delegate) => delegate.apply_settings(settings),
                None => Ok(()),
            }
        };
        let suspend = move |suspended: bool| {
            if let Some(delegate) = weak.load() {
                if suspended {
                    delegate.ivars().hotkey.borrow_mut().take();
                } else if let Err(error) = delegate.register_hotkey() {
                    eprintln!("yoinks: {error}");
                }
            }
        };
        let clipboard = ivars.clipboard.get().cloned();
        let model = Rc::clone(self.shelf().model());
        PreferencesWindow::new(
            mtm,
            ivars.settings.borrow().clone(),
            ivars.data_dir.get().cloned().unwrap_or_default(),
            PreferencesHooks {
                apply: Box::new(apply),
                suspend_hotkey: Box::new(suspend),
                clear_clipboard: Box::new(move || {
                    if let Some(clipboard) = &clipboard {
                        clipboard.clear();
                    }
                }),
                clear_thumbnails: Box::new(move || model.clear_thumbnails()),
            },
        )
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
