//! The application delegate: owns the shelf and status item, and handles
//! menu actions.

use std::cell::{OnceCell, RefCell};
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationDelegate, NSBeep, NSControlStateValueOff, NSControlStateValueOn,
    NSMenu, NSMenuItem, NSMenuItemValidation, NSPasteboard, NSStatusItem,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSNotification, NSPoint, NSString, NSTimer, NSURL, ns_string,
};

use super::{commands, status_item};
use crate::clipboard::{ClipboardHistory, HistoryList};
use crate::drag::DragMonitor;
use crate::folders::{Folder, FolderWatcher};
use crate::hotkey::{Gesture, HotKey};
use crate::ipc::{self, Request, server};
use crate::paths;
use crate::preferences::{PreferencesHooks, PreferencesWindow};
use crate::settings::Settings;
use crate::settings::ShelfPosition;
use crate::shelf::prompt::RenamePrompt;
use crate::shelf::{DEFAULT_SHELF, HIDE_AFTER_ADD, Shelf, ShelfModel, TagColor, read_pasteboard};

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
    /// Removes expired entries (`shelf_expiry_hours`).
    expiry_timer: OnceCell<Retained<NSTimer>>,
    /// Re-reads the power state for `save_energy_on_battery`.
    power_timer: OnceCell<Retained<NSTimer>>,
    /// Running while `add_screenshots` is on or folders are watched.
    folders: RefCell<Option<Rc<FolderWatcher>>>,
}

/// How often expired entries are looked for, in seconds.
const EXPIRY_CHECK: f64 = 60.0;
/// How often the power source is checked, in seconds.
const POWER_CHECK: f64 = 10.0;

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
            model.set_tint(settings.tint_shelf);
            let shelf = Shelf::new(mtm, settings.shelf_edge, model);
            shelf.set_hide_after_use(settings.hide_after_use);
            shelf.set_custom_origin(settings.shelf_position.map(|p| NSPoint::new(p.x, p.y)));
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

            // One Yoinks per data folder: a second copy would fight over the
            // same shelf, so it hands over to the first and quits.
            if let Err(error) = server::start(server::socket_path(&data_dir)) {
                eprintln!("yoinks: {error}");
                if error.kind() == std::io::ErrorKind::AddrInUse {
                    NSApplication::sharedApplication(mtm).terminate(None);
                    return;
                }
            }
            let target = Rc::clone(&shelf);
            server::set_handler(move |request| commands::handle(&target, request));
            // "Add to Yoinks" in the Services menu and Finder Quick Actions.
            // SAFETY: We implement `addToYoinks:userData:error:`.
            unsafe { NSApplication::sharedApplication(mtm).setServicesProvider(Some(self.as_ref())) };

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
            let (shortcut, show_icon) = {
                let settings = ivars.settings.borrow();
                (settings.shortcut(), settings.show_menu_bar_icon)
            };
            let _ = ivars
                .status_item
                .set(status_item::build(
                    mtm,
                    self,
                    shortcut.as_ref(),
                    &self.shelf().model().shelves(),
                    show_icon,
                ));
            // Remember where the user drags the shelf.
            let mover = objc2::rc::Weak::new(self);
            self.shelf().set_on_moved(move |origin| {
                if let Some(delegate) = mover.load() {
                    let mut settings = delegate.ivars().settings.borrow().clone();
                    settings.shelf_position = origin.map(|o| ShelfPosition { x: o.x, y: o.y });
                    let _ = delegate.apply_settings(settings);
                }
            });
            // The same menu from the gear button at the bottom of the shelf.
            let weak = objc2::rc::Weak::new(self);
            self.shelf().set_menu(move || match weak.load() {
                Some(delegate) => delegate.app_menu(),
                None => NSMenu::new(mtm),
            });
            crate::updates::start(mtm);
            self.start_expiry_timer();
            self.start_power_timer();
            self.expire_entries();
            self.update_folder_watcher();
        }

        // Files opened with Yoinks go on the shelf (one stack); `yoinks://`
        // links run their command.
        #[unsafe(method(application:openURLs:))]
        fn open_urls(&self, _app: &NSApplication, urls: &NSArray<NSURL>) {
            let mut paths = Vec::new();
            for url in urls.iter() {
                if let Some(path) = url.isFileURL().then(|| url.to_file_path()).flatten() {
                    paths.push(path);
                    continue;
                }
                let link = url.absoluteString().map(|s| s.to_string()).unwrap_or_default();
                match ipc::url::parse(&link) {
                    Ok(request) => self.run(request),
                    Err(error) => {
                        eprintln!("yoinks: {error}");
                        NSBeep();
                    }
                }
            }
            if !paths.is_empty() {
                self.run(Request::add_paths(paths));
            }
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
            } else if action == Some(sel!(checkForUpdates:)) {
                let available = crate::updates::available();
                item.setHidden(!available);
                let title = match crate::updates::found() {
                    Some(version) => format!("Update to Yoinks {version}…"),
                    None => "Check for Updates…".to_owned(),
                };
                item.setTitle(&NSString::from_str(&title));
                available
            } else if action == Some(sel!(switchShelf:)) {
                let current = model.current_shelf().id;
                let id = item
                    .representedObject()
                    .and_then(|object| object.downcast::<NSString>().ok())
                    .map(|id| id.to_string());
                let on = id.as_deref() == Some(current.as_str());
                item.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
                true
            } else if action == Some(sel!(setShelfColor:)) {
                let tag = model
                    .current_shelf()
                    .color
                    .and_then(|color| TagColor::ALL.iter().position(|c| *c == color))
                    .map_or(0, |index| index as isize + 1);
                item.setState(if item.tag() == tag { NSControlStateValueOn } else { NSControlStateValueOff });
                true
            } else if action == Some(sel!(deleteShelf:)) {
                model.current_shelf().id != DEFAULT_SHELF
            } else if action == Some(sel!(searchShelf:)) {
                !model.is_empty()
            } else if action == Some(sel!(clearShelf:)) {
                // Clear Shelf leaves pinned items alone.
                model.has_unpinned()
            } else if action == Some(sel!(resetShelfPosition:)) {
                self.shelf().has_custom_position()
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

        #[unsafe(method(switchShelf:))]
        fn switch_shelf(&self, sender: Option<&NSMenuItem>) {
            let id = sender
                .and_then(|item| item.representedObject())
                .and_then(|object| object.downcast::<NSString>().ok())
                .map(|id| id.to_string());
            if let Some(id) = id {
                self.shelf().model().switch_shelf(&id);
                self.shelf().show();
            }
        }

        #[unsafe(method(newShelf:))]
        fn new_shelf(&self, _sender: Option<&AnyObject>) {
            self.ask_shelf_name("New Shelf", "Create", "", |model, name| model.add_shelf(name));
        }

        #[unsafe(method(renameShelf:))]
        fn rename_shelf(&self, _sender: Option<&AnyObject>) {
            let current = self.shelf().model().current_shelf();
            self.ask_shelf_name("Rename Shelf", "Rename", &current.name, move |model, name| {
                model.rename_shelf(&current.id, name)
            });
        }

        #[unsafe(method(setShelfColor:))]
        fn set_shelf_color(&self, sender: Option<&NSMenuItem>) {
            let tag = sender.map_or(0, |item| item.tag());
            let color = usize::try_from(tag - 1).ok().and_then(|i| TagColor::ALL.get(i).copied());
            let model = self.shelf().model();
            model.set_shelf_color(&model.current_shelf().id, color);
            self.refresh_menu();
            self.shelf().show();
        }

        #[unsafe(method(deleteShelf:))]
        fn delete_shelf(&self, _sender: Option<&AnyObject>) {
            let model = Rc::clone(self.shelf().model());
            let shelf = model.current_shelf();
            let count = model.shelf_len(&shelf.id);
            if count > 0 && !confirm_delete(self.mtm(), &shelf.name, count) {
                return;
            }
            if let Err(error) = model.delete_shelf(&shelf.id) {
                eprintln!("yoinks: {error}");
                NSBeep();
            }
            self.refresh_menu();
        }

        #[unsafe(method(searchShelf:))]
        fn search_shelf(&self, _sender: Option<&AnyObject>) {
            let shelf = self.shelf();
            shelf.show();
            shelf.model().view().begin_search("");
        }

        #[unsafe(method(addClipboard:))]
        fn add_clipboard(&self, _sender: Option<&AnyObject>) {
            on_gesture(self.shelf(), Gesture::DoublePress);
        }

        #[unsafe(method(showClipboardHistory:))]
        fn show_clipboard_history(&self, _sender: Option<&AnyObject>) {
            let mtm = self.mtm();
            let ivars = self.ivars();
            let Some(list) = ivars.history_list.get() else { return };
            // Under the menu bar icon, or next to the shelf when the icon is
            // hidden (the command then comes from the shelf's gear menu).
            let status_button = ivars
                .status_item
                .get()
                .filter(|item| item.isVisible())
                .and_then(|item| item.button(mtm));
            match status_button {
                Some(button) => list.toggle(&button),
                None => {
                    self.shelf().show();
                    list.toggle(self.shelf().menu_button());
                }
            }
        }

        #[unsafe(method(toggleClipboardPause:))]
        fn toggle_clipboard_pause(&self, _sender: Option<&AnyObject>) {
            if let Some(clipboard) = self.ivars().clipboard.get() {
                clipboard.set_paused(!clipboard.is_paused());
            }
        }

        #[unsafe(method(checkForUpdates:))]
        fn check_for_updates(&self, _sender: Option<&AnyObject>) {
            crate::updates::check(self.mtm());
        }

        #[unsafe(method(showPreferences:))]
        fn show_preferences(&self, _sender: Option<&AnyObject>) {
            let window = self.ivars().preferences.get_or_init(|| self.make_preferences());
            window.show();
        }

        // The "Add to Yoinks" service (see NSServices in Info.plist).
        #[unsafe(method(addToYoinks:userData:error:))]
        fn add_to_yoinks(
            &self,
            pasteboard: &NSPasteboard,
            _user_data: Option<&NSString>,
            _error: *mut *mut NSString,
        ) {
            let shelf = self.shelf();
            if shelf.model().accept(read_pasteboard(pasteboard)) {
                shelf.appear_at_edge();
                shelf.arm_auto_hide(HIDE_AFTER_ADD);
            } else {
                NSBeep();
            }
        }

        #[unsafe(method(resetShelfPosition:))]
        fn reset_shelf_position(&self, _sender: Option<&AnyObject>) {
            self.shelf().reset_position();
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

    fn run(&self, request: Request) {
        let response = server::handle(request);
        if !response.ok {
            eprintln!("yoinks: {}", response.error.unwrap_or_default());
            NSBeep();
        }
    }

    /// Rebuilds the menu bar icon's menu after the shelves changed.
    fn refresh_menu(&self) {
        if let Some(item) = self.ivars().status_item.get() {
            let shortcut = self.ivars().settings.borrow().shortcut();
            status_item::rebuild_menu(
                self.mtm(),
                item,
                self,
                shortcut.as_ref(),
                &self.shelf().model().shelves(),
            );
        }
    }

    /// Shows the shelf and asks for a shelf name beside it; `apply` uses
    /// it or says why it can't.
    fn ask_shelf_name(
        &self,
        title: &str,
        button: &str,
        current: &str,
        apply: impl Fn(&ShelfModel, &str) -> Result<(), String> + 'static,
    ) {
        let shelf = self.shelf();
        shelf.show();
        let model = Rc::downgrade(shelf.model());
        let weak = objc2::rc::Weak::new(self);
        RenamePrompt::ask(
            self.mtm(),
            shelf.menu_button(),
            title,
            button,
            current,
            false,
            move |name| {
                let Some(model) = model.upgrade() else {
                    return Ok(());
                };
                apply(&model, name)?;
                if let Some(delegate) = weak.load() {
                    delegate.refresh_menu();
                }
                Ok(())
            },
        );
    }

    /// The app menu with the current shortcut, for the shelf's gear button.
    fn app_menu(&self) -> Retained<NSMenu> {
        let shortcut = self.ivars().settings.borrow().shortcut();
        status_item::menu(
            self.mtm(),
            self,
            shortcut.as_ref(),
            &self.shelf().model().shelves(),
        )
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
        shelf.set_hide_after_use(settings.hide_after_use);
        shelf.set_custom_origin(settings.shelf_position.map(|p| NSPoint::new(p.x, p.y)));
        shelf
            .model()
            .set_keep_after_drag_out(settings.keep_after_drag_out);
        shelf.model().set_tint(settings.tint_shelf);
        if let Some(monitor) = ivars.drag_monitor.get() {
            monitor.update(&settings);
        }
        if let Some(item) = ivars.status_item.get() {
            item.setVisible(settings.show_menu_bar_icon);
        }
        if let Some(clipboard) = ivars.clipboard.get() {
            clipboard.apply(&settings);
        }
        if old.shelf_expiry_hours != settings.shelf_expiry_hours {
            self.expire_entries();
        }
        if old.save_energy_on_battery != settings.save_energy_on_battery {
            self.update_power();
        }
        if old.add_screenshots != settings.add_screenshots
            || old.watched_folders != settings.watched_folders
        {
            self.update_folder_watcher();
        }
        if old.hotkey == settings.hotkey {
            return Ok(());
        }
        let result = self.register_hotkey();
        if let Some(item) = ivars.status_item.get() {
            status_item::rebuild_menu(
                self.mtm(),
                item,
                self,
                settings.shortcut().as_ref(),
                &self.shelf().model().shelves(),
            );
        }
        result
    }

    fn start_expiry_timer(&self) {
        let weak = objc2::rc::Weak::new(self);
        let block = RcBlock::new(move |_timer: NonNull<NSTimer>| {
            if let Some(delegate) = weak.load() {
                delegate.expire_entries();
            }
        });
        // SAFETY: Scheduled on the main run loop; the block only touches
        // main-thread state.
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_repeats_block(EXPIRY_CHECK, true, &block)
        };
        let _ = self.ivars().expiry_timer.set(timer);
    }

    fn start_power_timer(&self) {
        self.update_power();
        let weak = objc2::rc::Weak::new(self);
        let block = RcBlock::new(move |_timer: NonNull<NSTimer>| {
            if let Some(delegate) = weak.load() {
                delegate.update_power();
            }
        });
        // SAFETY: Scheduled on the main run loop; the block only touches
        // main-thread state.
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_repeats_block(POWER_CHECK, true, &block)
        };
        let _ = self.ivars().power_timer.set(timer);
    }

    /// Starts or stops saving energy as the power source changes. Once it
    /// stops, the shelf asks for the thumbnails it skipped.
    fn update_power(&self) {
        let enabled = self.ivars().settings.borrow().save_energy_on_battery;
        if crate::power::update(enabled) && !crate::power::saving() {
            self.shelf().model().refresh();
        }
    }

    /// Removes unpinned entries older than `shelf_expiry_hours`, if set.
    fn expire_entries(&self) {
        let max_age = self.ivars().settings.borrow().shelf_max_age();
        if let Some(max_age) = max_age {
            self.shelf().model().expire(max_age);
        }
    }

    /// (Re)starts watching folders to match `add_screenshots` and
    /// `watched_folders`; stops when there is nothing to watch.
    fn update_folder_watcher(&self) {
        let ivars = self.ivars();
        let folders: Vec<Folder> = {
            let settings = ivars.settings.borrow();
            settings
                .add_screenshots
                .then_some(Folder::Screenshots)
                .into_iter()
                .chain(settings.watched_folders.iter().cloned().map(Folder::Path))
                .collect()
        };
        let mut watcher = ivars.folders.borrow_mut();
        watcher.take();
        if !folders.is_empty() {
            let shelf = Rc::clone(self.shelf());
            *watcher = Some(FolderWatcher::start(folders, move |paths| {
                let response = commands::handle(&shelf, Request::add_paths(paths));
                if !response.ok {
                    eprintln!("yoinks: {}", response.error.unwrap_or_default());
                }
            }));
        }
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
        let current = {
            let weak = weak.clone();
            move || {
                weak.load()
                    .map(|delegate| delegate.ivars().settings.borrow().clone())
                    .unwrap_or_default()
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
                current: Box::new(current),
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
        // Something was added or recalled: show it, then slide away again
        // if the user leaves it alone.
        shelf.appear_at_edge();
        shelf.arm_auto_hide(HIDE_AFTER_ADD);
    }
}

/// Asks before deleting a shelf that has items on it.
fn confirm_delete(mtm: MainThreadMarker, name: &str, count: usize) -> bool {
    let alert = objc2_app_kit::NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(&format!(
        "Delete the shelf \"{name}\"?"
    )));
    let items = if count == 1 {
        "1 item".to_owned()
    } else {
        format!("{count} items")
    };
    alert.setInformativeText(&NSString::from_str(&format!(
        "Its {items} will be removed. Files you added from Finder stay where they are."
    )));
    alert.addButtonWithTitle(ns_string!("Delete"));
    alert.addButtonWithTitle(ns_string!("Cancel"));
    crate::ui::activate_app(mtm);
    alert.runModal() == objc2_app_kit::NSAlertFirstButtonReturn
}
