//! The Preferences window: General, Shelf, Shortcut, Clipboard and Storage
//! tabs.
//! Every change is applied immediately and saved to `settings.json`.

use std::cell::{OnceCell, RefCell};
use std::path::PathBuf;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSControlStateValueOff, NSControlStateValueOn,
    NSLayoutConstraint, NSPanel, NSPopUpButton, NSTabView, NSTabViewDelegate, NSTabViewItem,
    NSTextField, NSView, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    NSURL, ns_string,
};
use objc2_service_management::{SMAppService, SMAppServiceStatus};

use super::app_list::AppList;
use super::form::{self, column, indented, note, row};
use super::recorder::{Recorded, ShortcutRecorder};
use super::storage::dir_size;
use crate::settings::{AppearMode, DEFAULT_HOTKEY, Settings};
use crate::shelf::ShelfEdge;

const WIDTH: f64 = 560.0;
const HEIGHT: f64 = 470.0;
const NOTE_WIDTH: f64 = 300.0;

const EDGES: [(ShelfEdge, &str); 2] = [(ShelfEdge::Left, "Left"), (ShelfEdge::Right, "Right")];
const APPEAR_MODES: [(AppearMode, &str); 4] = [
    (AppearMode::AtEdge, "At the screen edge"),
    (AppearMode::NearEdge, "When the cursor nears the edge"),
    (AppearMode::AtCursor, "Next to the cursor"),
    (AppearMode::Off, "Never"),
];
const HISTORY_SIZES: [usize; 5] = [25, 50, 100, 200, 500];
const EXPIRY: [(Option<u32>, &str); 5] = [
    (None, "Never"),
    (Some(1), "After 1 hour"),
    (Some(24), "After 1 day"),
    (Some(24 * 7), "After 1 week"),
    (Some(24 * 30), "After 30 days"),
];

/// What the window needs from the rest of the app.
pub struct PreferencesHooks {
    /// The settings in effect right now. Changes start from these, so
    /// changes made elsewhere (like moving the shelf) are never overwritten
    /// by a stale copy.
    pub current: Box<dyn Fn() -> Settings>,
    /// Save and apply settings; an error describes a shortcut problem.
    pub apply: Box<dyn Fn(Settings) -> Result<(), String>>,
    /// Turn the global shortcut off while a new one is being recorded.
    pub suspend_hotkey: Box<dyn Fn(bool)>,
    pub clear_clipboard: Box<dyn Fn()>,
    pub clear_thumbnails: Box<dyn Fn()>,
}

pub struct PrefsIvars {
    settings: RefCell<Settings>,
    data_dir: PathBuf,
    hooks: PreferencesHooks,
    window: OnceCell<Retained<NSPanel>>,
    login: OnceCell<Retained<NSButton>>,
    login_note: OnceCell<Retained<NSTextField>>,
    /// "Check for updates automatically"; Sparkle owns the setting.
    updates: OnceCell<Retained<NSButton>>,
    recorder: OnceCell<Retained<ShortcutRecorder>>,
    shortcut_note: OnceCell<Retained<NSTextField>>,
    sizes: OnceCell<Retained<NSTextField>>,
    /// App list controllers; tables only hold them weakly.
    lists: RefCell<Vec<Retained<AppList>>>,
    /// Content height each tab needs, in tab order, so the window can
    /// resize to the selected tab.
    tab_heights: RefCell<Vec<f64>>,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - `PreferencesWindow` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "YoinksPreferences"]
    #[ivars = PrefsIvars]
    pub struct PreferencesWindow;

    impl PreferencesWindow {
        #[unsafe(method(launchAtLoginChanged:))]
        fn launch_at_login_changed(&self, sender: &NSButton) {
            let enable = sender.state() == NSControlStateValueOn;
            // SAFETY: Plain ServiceManagement calls for this app.
            let result = unsafe {
                let service = SMAppService::mainAppService();
                if enable { service.registerAndReturnError() } else { service.unregisterAndReturnError() }
            };
            let error = result.err().map(|e| e.localizedDescription().to_string());
            self.refresh_login(error);
        }

        #[unsafe(method(openLoginItems:))]
        fn open_login_items(&self, _sender: Option<&AnyObject>) {
            // SAFETY: Opens System Settings; no preconditions.
            unsafe { SMAppService::openSystemSettingsLoginItems() };
        }

        #[unsafe(method(edgeChanged:))]
        fn edge_changed(&self, sender: &NSPopUpButton) {
            if let Some((edge, _)) = usize::try_from(sender.indexOfSelectedItem()).ok().and_then(|i| EDGES.get(i)) {
                // A new edge also brings a dragged shelf back to it.
                self.update(|s| {
                    s.shelf_edge = *edge;
                    s.shelf_position = None;
                });
            }
        }

        #[unsafe(method(appearChanged:))]
        fn appear_changed(&self, sender: &NSPopUpButton) {
            if let Some((mode, _)) = usize::try_from(sender.indexOfSelectedItem()).ok().and_then(|i| APPEAR_MODES.get(i)) {
                self.update(|s| s.appear_on_drag = *mode);
            }
        }

        #[unsafe(method(menuBarIconChanged:))]
        fn menu_bar_icon_changed(&self, sender: &NSButton) {
            let show = sender.state() == NSControlStateValueOn;
            self.update(|s| s.show_menu_bar_icon = show);
        }

        #[unsafe(method(hideAfterUseChanged:))]
        fn hide_after_use_changed(&self, sender: &NSButton) {
            let hide = sender.state() == NSControlStateValueOn;
            self.update(|s| s.hide_after_use = hide);
        }

        #[unsafe(method(keepChanged:))]
        fn keep_changed(&self, sender: &NSButton) {
            let keep = sender.state() == NSControlStateValueOn;
            self.update(|s| s.keep_after_drag_out = keep);
        }

        #[unsafe(method(expiryChanged:))]
        fn expiry_changed(&self, sender: &NSPopUpButton) {
            if let Some((hours, _)) = usize::try_from(sender.indexOfSelectedItem()).ok().and_then(|i| EXPIRY.get(i)) {
                self.update(|s| s.shelf_expiry_hours = *hours);
            }
        }

        #[unsafe(method(screenshotsChanged:))]
        fn screenshots_changed(&self, sender: &NSButton) {
            let on = sender.state() == NSControlStateValueOn;
            self.update(|s| s.add_screenshots = on);
        }

        #[unsafe(method(energyChanged:))]
        fn energy_changed(&self, sender: &NSButton) {
            let on = sender.state() == NSControlStateValueOn;
            self.update(|s| s.save_energy_on_battery = on);
        }

        #[unsafe(method(autoUpdatesChanged:))]
        fn auto_updates_changed(&self, sender: &NSButton) {
            crate::updates::set_checks_automatically(sender.state() == NSControlStateValueOn);
        }

        #[unsafe(method(resetShortcut:))]
        fn reset_shortcut(&self, _sender: Option<&AnyObject>) {
            self.set_hotkey(Some(DEFAULT_HOTKEY.to_owned()));
        }

        #[unsafe(method(clipboardEnabledChanged:))]
        fn clipboard_enabled_changed(&self, sender: &NSButton) {
            let enabled = sender.state() == NSControlStateValueOn;
            self.update(|s| s.clipboard_history = enabled);
        }

        #[unsafe(method(skipSecretsChanged:))]
        fn skip_secrets_changed(&self, sender: &NSButton) {
            let skip = sender.state() == NSControlStateValueOn;
            self.update(|s| s.clipboard_skip_secrets = skip);
        }

        #[unsafe(method(clipboardSizeChanged:))]
        fn clipboard_size_changed(&self, sender: &NSPopUpButton) {
            let size = sender
                .titleOfSelectedItem()
                .and_then(|title| title.to_string().split_whitespace().next()?.parse().ok());
            if let Some(size) = size {
                self.update(|s| s.clipboard_history_size = size);
            }
        }

        #[unsafe(method(clearClipboard:))]
        fn clear_clipboard(&self, _sender: Option<&AnyObject>) {
            (self.ivars().hooks.clear_clipboard)();
            self.refresh_sizes();
        }

        #[unsafe(method(showDataFolder:))]
        fn show_data_folder(&self, _sender: Option<&AnyObject>) {
            if let Some(url) = NSURL::from_directory_path(&self.ivars().data_dir) {
                NSWorkspace::sharedWorkspace()
                    .activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
            }
        }

        #[unsafe(method(clearThumbnails:))]
        fn clear_thumbnails(&self, _sender: Option<&AnyObject>) {
            (self.ivars().hooks.clear_thumbnails)();
            self.refresh_sizes();
        }
    }

    // SAFETY: `NSObjectProtocol` has no safety requirements.
    unsafe impl NSObjectProtocol for PreferencesWindow {}

    // SAFETY: The method signature matches the protocol.
    unsafe impl NSTabViewDelegate for PreferencesWindow {
        // Disk usage changes while the window is open.
        #[unsafe(method(tabView:didSelectTabViewItem:))]
        fn tab_selected(&self, tabs: &NSTabView, item: Option<&NSTabViewItem>) {
            self.refresh_sizes();
            if let Some(item) = item {
                self.fit_window_to_tab(tabs.indexOfTabViewItem(item));
            }
        }
    }
);

impl PreferencesWindow {
    pub fn new(
        mtm: MainThreadMarker,
        settings: Settings,
        data_dir: PathBuf,
        hooks: PreferencesHooks,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(PrefsIvars {
            settings: RefCell::new(settings),
            data_dir,
            hooks,
            window: OnceCell::new(),
            login: OnceCell::new(),
            login_note: OnceCell::new(),
            updates: OnceCell::new(),
            recorder: OnceCell::new(),
            shortcut_note: OnceCell::new(),
            sizes: OnceCell::new(),
            lists: RefCell::default(),
            tab_heights: RefCell::default(),
        });
        // SAFETY: NSObject's `init` has the correct signature.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.build();
        this
    }

    /// Shows the window with fresh login item status and disk usage. It is a
    /// non-activating panel, so it takes clicks and typing without pulling
    /// the app in front of the user's work.
    pub fn show(&self) {
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        self.refresh_login(None);
        self.refresh_sizes();
        if let Some(updates) = self.ivars().updates.get() {
            updates.setState(if crate::updates::checks_automatically() {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
        if !window.isVisible() {
            window.center();
        }
        window.orderFrontRegardless();
        window.makeKeyWindow();
    }

    /// Applies a change that cannot fail (only the shortcut can).
    fn update(&self, change: impl FnOnce(&mut Settings)) {
        let _ = self.apply_change(change);
    }

    /// Applies a change to a copy of the settings and hands it to the app.
    fn apply_change(&self, change: impl FnOnce(&mut Settings)) -> Result<(), String> {
        let mut settings = (self.ivars().hooks.current)();
        change(&mut settings);
        *self.ivars().settings.borrow_mut() = settings.clone();
        let result = (self.ivars().hooks.apply)(settings);
        if let Err(error) = &result {
            eprintln!("yoinks: {error}");
        }
        result
    }

    fn set_hotkey(&self, hotkey: Option<String>) {
        let result = self.apply_change(|s| s.hotkey = hotkey);
        let shortcut = self.ivars().settings.borrow().shortcut();
        if let Some(recorder) = self.ivars().recorder.get() {
            recorder.show(shortcut.as_ref());
        }
        let message = match (result, &shortcut) {
            (Err(error), _) => format!("Could not use this shortcut: {error}."),
            (Ok(()), Some(s)) => match s.known_conflict() {
                Some(what) => format!("Warning: {} is normally used for {what}.", s.symbols()),
                None => String::new(),
            },
            (Ok(()), None) => "No shortcut: use the menu bar icon instead.".to_owned(),
        };
        if let Some(label) = self.ivars().shortcut_note.get() {
            label.setStringValue(&NSString::from_str(&message));
            set_row_hidden(label, message.is_empty());
        }
    }

    fn recorded(&self, result: Recorded) {
        match result {
            Recorded::Shortcut(shortcut) => self.set_hotkey(Some(shortcut.to_string())),
            Recorded::Cleared => self.set_hotkey(None),
            Recorded::Cancelled | Recorded::Invalid(_) => {
                let shortcut = self.ivars().settings.borrow().shortcut();
                if let Some(recorder) = self.ivars().recorder.get() {
                    recorder.show(shortcut.as_ref());
                }
                if let (Recorded::Invalid(message), Some(label)) =
                    (result, self.ivars().shortcut_note.get())
                {
                    label.setStringValue(&NSString::from_str(&format!("{message}.")));
                    set_row_hidden(label, false);
                }
            }
        }
    }

    /// Resizes the window to tab `index`, keeping its top edge in place.
    fn fit_window_to_tab(&self, index: isize) {
        let (Some(window), Ok(index)) = (self.ivars().window.get(), usize::try_from(index)) else {
            return;
        };
        let Some(height) = self.ivars().tab_heights.borrow().get(index).copied() else {
            return;
        };
        let Some(content) = window.contentView() else {
            return;
        };
        let delta = height - content.frame().size.height;
        if delta.abs() < 1.0 {
            return;
        }
        let mut frame = window.frame();
        frame.size.height += delta;
        frame.origin.y -= delta;
        window.setFrame_display_animate(frame, true, window.isVisible());
    }

    fn refresh_login(&self, error: Option<String>) {
        // SAFETY: Reading this app's login item status.
        let status = unsafe { SMAppService::mainAppService().status() };
        if let Some(checkbox) = self.ivars().login.get() {
            let on = status == SMAppServiceStatus::Enabled
                || status == SMAppServiceStatus::RequiresApproval;
            checkbox.setState(if on {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
        let message = match (error, status) {
            (Some(error), _) => format!("Could not change the login item: {error}"),
            (None, SMAppServiceStatus::RequiresApproval) => {
                "Allow Yoinks in System Settings > General > Login Items.".to_owned()
            }
            _ => String::new(),
        };
        if let Some(label) = self.ivars().login_note.get() {
            label.setStringValue(&NSString::from_str(&message));
            // Stack views drop hidden rows, so an empty note takes no space.
            set_row_hidden(label, message.is_empty());
        }
    }

    fn refresh_sizes(&self) {
        let dir = &self.ivars().data_dir;
        let size = |name: &str| crate::shelf::format_bytes(dir_size(&dir.join(name)));
        let text = format!(
            "Shelf copies: {}\nThumbnails: {}\nClipboard images and rich text: {}",
            size("Items"),
            size("Thumbnails"),
            size("Clips"),
        );
        if let Some(label) = self.ivars().sizes.get() {
            label.setStringValue(&NSString::from_str(&text));
        }
    }

    fn build(&self) {
        let mtm = self.mtm();
        let tabs = NSTabView::initWithFrame(
            NSTabView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT)),
        );
        // Each tab's form is pinned to the top of its page; the window is
        // sized below to fit the tallest one.
        let mut heights = Vec::new();
        for (label, form) in [
            ("General", self.general_tab()),
            ("Shelf", self.shelf_tab()),
            ("Shortcut", self.shortcut_tab()),
            ("Clipboard", self.clipboard_tab()),
            ("Storage", self.storage_tab()),
        ] {
            heights.push(form.fittingSize().height);
            let page = NSView::new(mtm);
            form.setTranslatesAutoresizingMaskIntoConstraints(false);
            page.addSubview(&form);
            NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
                form.topAnchor().constraintEqualToAnchor(&page.topAnchor()),
                form.leadingAnchor()
                    .constraintEqualToAnchor(&page.leadingAnchor()),
                form.trailingAnchor()
                    .constraintEqualToAnchor(&page.trailingAnchor()),
            ]));
            let item = NSTabViewItem::new();
            item.setLabel(&NSString::from_str(label));
            item.setView(Some(&page));
            tabs.addTabViewItem(&item);
        }
        // The tab strip and border take the difference between the tab
        // view's frame and its content area.
        let chrome = HEIGHT - tabs.contentRect().size.height;
        // A little extra: fitting sizes run slightly short for wrapped text.
        const PADDING: f64 = 32.0;
        *self.ivars().tab_heights.borrow_mut() = heights
            .iter()
            .map(|h| (h + chrome + PADDING).ceil())
            .collect();
        let height = self
            .ivars()
            .tab_heights
            .borrow()
            .first()
            .copied()
            .unwrap_or(HEIGHT);
        // The tab view holds its delegate weakly; we own the window.
        tabs.setDelegate(Some(ProtocolObject::from_ref(self)));

        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::NonactivatingPanel;
        // SAFETY: NSPanel's designated initializer.
        let window: Retained<NSPanel> = unsafe {
            msg_send![
                NSPanel::alloc(mtm),
                initWithContentRect: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT)),
                styleMask: style,
                backing: NSBackingStoreType::Buffered,
                defer: true,
            ]
        };
        // SAFETY: We hold a strong reference to the window.
        unsafe { window.setReleasedWhenClosed(false) };
        // The version this build was made from (Cargo.toml). In the title
        // rather than a subtitle, which macOS joins with a dash.
        window.setTitle(ns_string!(concat!(
            "Yoinks Preferences (Version ",
            env!("CARGO_PKG_VERSION"),
            ")"
        )));
        window.setHidesOnDeactivate(false);
        window.setContentView(Some(&tabs));
        window.setContentSize(NSSize::new(WIDTH, height));
        let _ = self.ivars().window.set(window);
        for label in [
            self.ivars().login_note.get(),
            self.ivars().shortcut_note.get(),
        ]
        .into_iter()
        .flatten()
        {
            set_row_hidden(label, true);
        }
    }

    fn general_tab(&self) -> Retained<NSView> {
        let mtm = self.mtm();
        let target: &AnyObject = self.as_ref();
        let settings = self.ivars().settings.borrow().clone();

        let login = form::checkbox(
            mtm,
            "Open Yoinks at login",
            target,
            sel!(launchAtLoginChanged:),
        );
        let login_note = note(mtm, "", NOTE_WIDTH);
        let icon = form::checkbox(
            mtm,
            "Show Yoinks in the menu bar",
            target,
            sel!(menuBarIconChanged:),
        );
        icon.setState(if settings.show_menu_bar_icon {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        let icon_note = note(
            mtm,
            "When hidden, open the shelf with the keyboard shortcut or by opening Yoinks \
             again, and use the gear button at the bottom of the shelf for this window \
             and the other menu commands.",
            NOTE_WIDTH,
        );
        let energy = form::checkbox(mtm, "Save energy on battery", target, sel!(energyChanged:));
        energy.setState(if settings.save_energy_on_battery {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        let energy_note = note(
            mtm,
            "On battery or in Low Power Mode, Yoinks checks the clipboard and the \
             screenshot folder less often and makes no new thumbnails (icons stand in \
             until the Mac is plugged in).",
            NOTE_WIDTH,
        );

        let mut rows: Vec<Retained<NSView>> = vec![
            up(&*row(mtm, "Startup:", &login)),
            up(&*indented(mtm, &login_note)),
            up(&*row(mtm, "Menu bar:", &icon)),
            up(&*indented(mtm, &icon_note)),
            up(&*row(mtm, "Battery:", &energy)),
            up(&*indented(mtm, &energy_note)),
        ];
        // Only builds with Sparkle (releases) can update themselves.
        if crate::updates::available() {
            let updates = form::checkbox(
                mtm,
                "Check for updates automatically",
                target,
                sel!(autoUpdatesChanged:),
            );
            rows.insert(2, up(&*row(mtm, "Updates:", &updates)));
            let _ = self.ivars().updates.set(updates);
        }
        let _ = self.ivars().login.set(login);
        let _ = self.ivars().login_note.set(login_note);
        up(&*column(mtm, &refs(&rows)))
    }

    fn shelf_tab(&self) -> Retained<NSView> {
        let mtm = self.mtm();
        let target: &AnyObject = self.as_ref();
        let settings = self.ivars().settings.borrow().clone();

        let edge = form::popup(mtm, &EDGES.map(|(_, t)| t), target, sel!(edgeChanged:));
        edge.selectItemAtIndex(
            EDGES
                .iter()
                .position(|(e, _)| *e == settings.shelf_edge)
                .unwrap_or(1) as isize,
        );
        let appear = form::popup(
            mtm,
            &APPEAR_MODES.map(|(_, t)| t),
            target,
            sel!(appearChanged:),
        );
        appear.selectItemAtIndex(
            APPEAR_MODES
                .iter()
                .position(|(m, _)| *m == settings.appear_on_drag)
                .unwrap_or(0) as isize,
        );
        let hide = form::checkbox(
            mtm,
            "Hide the shelf once it's empty",
            target,
            sel!(hideAfterUseChanged:),
        );
        hide.setState(if settings.hide_after_use {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        let hide_note = note(
            mtm,
            "It slides away about a second after the pointer leaves it. With items on it, \
             it stays until you hide it (Escape, the shortcut or the menu). Drag the shelf \
             by its background to move it; the gear menu puts it back at the edge.",
            NOTE_WIDTH,
        );
        let keep = form::checkbox(
            mtm,
            "Keep items after dragging them out",
            target,
            sel!(keepChanged:),
        );
        keep.setState(if settings.keep_after_drag_out {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });

        let mut expiry_choices = EXPIRY.to_vec();
        let custom;
        if !EXPIRY
            .iter()
            .any(|(h, _)| *h == settings.shelf_expiry_hours)
        {
            // A value set by hand in settings.json.
            custom = format!("After {} hours", settings.shelf_expiry_hours.unwrap_or(0));
            expiry_choices.push((settings.shelf_expiry_hours, custom.as_str()));
        }
        let expiry = form::popup(
            mtm,
            &expiry_choices.iter().map(|(_, t)| *t).collect::<Vec<_>>(),
            target,
            sel!(expiryChanged:),
        );
        expiry.selectItemAtIndex(
            expiry_choices
                .iter()
                .position(|(h, _)| *h == settings.shelf_expiry_hours)
                .unwrap_or(0) as isize,
        );
        let screenshots = form::checkbox(
            mtm,
            "Add new screenshots to the shelf",
            target,
            sel!(screenshotsChanged:),
        );
        screenshots.setState(if settings.add_screenshots {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        let screenshots_note = note(
            mtm,
            "Screenshots and screen recordings saved to the screenshot folder (the Desktop \
             unless changed in the Screenshot app's Options) appear on the shelf.",
            NOTE_WIDTH,
        );

        let weak = Weak::new(self);
        let apps = AppList::new(mtm, settings.ignored_apps.clone(), move |ids| {
            if let Some(this) = weak.load() {
                this.update(|s| s.ignored_apps = ids);
            }
        });
        let apps_view = apps.view(NOTE_WIDTH, 96.0);

        let rows: Vec<Retained<NSView>> = vec![
            up(&*row(mtm, "Shelf edge:", &edge)),
            up(&*row(mtm, "Show the shelf during drags:", &appear)),
            up(&*row(mtm, "Auto-hide:", &hide)),
            up(&*indented(mtm, &hide_note)),
            up(&*row(mtm, "After dragging out:", &keep)),
            up(&*row(mtm, "Remove unpinned items:", &expiry)),
            up(&*row(mtm, "Screenshots:", &screenshots)),
            up(&*indented(mtm, &screenshots_note)),
            up(&*row(mtm, "Don't show it for drags from:", &apps_view)),
            up(&*indented(
                mtm,
                &note(
                    mtm,
                    "The keyboard shortcut and menu bar icon still work in these apps.",
                    NOTE_WIDTH,
                ),
            )),
        ];
        self.ivars().lists.borrow_mut().push(apps);
        up(&*column(mtm, &refs(&rows)))
    }

    fn shortcut_tab(&self) -> Retained<NSView> {
        let mtm = self.mtm();
        let target: &AnyObject = self.as_ref();
        let (suspend, recorded) = (Weak::new(self), Weak::new(self));
        let recorder = ShortcutRecorder::new(
            mtm,
            move |recording| {
                if let Some(this) = suspend.load() {
                    (this.ivars().hooks.suspend_hotkey)(recording);
                }
            },
            move |result| {
                if let Some(this) = recorded.load() {
                    this.recorded(result);
                }
            },
        );
        recorder.show(self.ivars().settings.borrow().shortcut().as_ref());
        recorder.setTranslatesAutoresizingMaskIntoConstraints(false);
        recorder
            .widthAnchor()
            .constraintEqualToConstant(160.0)
            .setActive(true);
        let reset = form::button(mtm, "Use ⌃⌥Y", target, sel!(resetShortcut:));
        let controls = objc2_app_kit::NSStackView::stackViewWithViews(
            &NSArray::from_slice(&[&*recorder as &NSView, &*reset]),
            mtm,
        );
        controls.setSpacing(8.0);
        let shortcut_note = note(mtm, "", NOTE_WIDTH);
        let help = note(
            mtm,
            "Click the box, then press the keys. Delete removes the shortcut; Escape cancels.\n\n\
             Press: show or hide the shelf.\n\
             Hold: bring back the last item you dragged out.\n\
             Press twice: add what's on the clipboard to the shelf.",
            NOTE_WIDTH,
        );
        let rows: Vec<Retained<NSView>> = vec![
            up(&*row(mtm, "Shelf shortcut:", &controls)),
            up(&*indented(mtm, &shortcut_note)),
            up(&*indented(mtm, &help)),
        ];
        let _ = self.ivars().recorder.set(recorder);
        let _ = self.ivars().shortcut_note.set(shortcut_note);
        up(&*column(mtm, &refs(&rows)))
    }

    fn clipboard_tab(&self) -> Retained<NSView> {
        let mtm = self.mtm();
        let target: &AnyObject = self.as_ref();
        let settings = self.ivars().settings.borrow().clone();

        let enabled = form::checkbox(
            mtm,
            "Keep a clipboard history",
            target,
            sel!(clipboardEnabledChanged:),
        );
        enabled.setState(if settings.clipboard_history {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        let skip_secrets = form::checkbox(
            mtm,
            "Skip passwords, API keys and tokens",
            target,
            sel!(skipSecretsChanged:),
        );
        skip_secrets.setState(if settings.clipboard_skip_secrets {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        let skip_note = note(
            mtm,
            "Copies that look like a key or token (GitHub, AWS, Stripe, OpenAI and \
             others), a private key, a KEY=value line with a password or token, or a \
             long random string are not recorded.",
            NOTE_WIDTH,
        );
        let mut sizes: Vec<usize> = HISTORY_SIZES.to_vec();
        if !sizes.contains(&settings.clipboard_history_size) {
            sizes.push(settings.clipboard_history_size);
            sizes.sort_unstable();
        }
        let titles: Vec<String> = sizes.iter().map(|n| format!("{n} items")).collect();
        let size = form::popup(
            mtm,
            &titles.iter().map(String::as_str).collect::<Vec<_>>(),
            target,
            sel!(clipboardSizeChanged:),
        );
        size.selectItemAtIndex(
            sizes
                .iter()
                .position(|n| *n == settings.clipboard_history_size)
                .unwrap_or(1) as isize,
        );

        let weak = Weak::new(self);
        let apps = AppList::new(mtm, settings.clipboard_ignored_apps.clone(), move |ids| {
            if let Some(this) = weak.load() {
                this.update(|s| s.clipboard_ignored_apps = ids);
            }
        });
        let apps_view = apps.view(NOTE_WIDTH, 96.0);
        let clear = form::button(mtm, "Clear Unpinned History", target, sel!(clearClipboard:));

        let rows: Vec<Retained<NSView>> = vec![
            up(&*row(mtm, "Clipboard:", &enabled)),
            up(&*indented(mtm, &skip_secrets)),
            up(&*indented(mtm, &skip_note)),
            up(&*row(mtm, "Remember:", &size)),
            up(&*row(mtm, "Never record copies from:", &apps_view)),
            up(&*indented(
                mtm,
                &note(
                    mtm,
                    "Password managers (1Password, Bitwarden, KeePassXC, Passwords and others) and \
                 anything an app marks as concealed are always skipped. Pinned items are never \
                 removed automatically.",
                    NOTE_WIDTH,
                ),
            )),
            up(&*indented(mtm, &clear)),
        ];
        self.ivars().lists.borrow_mut().push(apps);
        up(&*column(mtm, &refs(&rows)))
    }

    fn storage_tab(&self) -> Retained<NSView> {
        let mtm = self.mtm();
        let target: &AnyObject = self.as_ref();
        let home = std::env::home_dir();
        let path = crate::shelf::tilde(&self.ivars().data_dir, home.as_deref());
        let location = note(mtm, &path, NOTE_WIDTH);
        location.setSelectable(true);
        let show = form::button(mtm, "Show in Finder", target, sel!(showDataFolder:));
        let sizes = note(mtm, "", NOTE_WIDTH);
        let clear = form::button(mtm, "Clear Thumbnails", target, sel!(clearThumbnails:));
        let rows: Vec<Retained<NSView>> = vec![
            up(&*row(mtm, "Data folder:", &location)),
            up(&*indented(mtm, &show)),
            up(&*row(mtm, "Space used:", &sizes)),
            up(&*indented(mtm, &clear)),
            up(&*indented(
                mtm,
                &note(
                    mtm,
                    "Thumbnails are recreated as needed. Shelf copies are the text, images and \
                 attachments you dropped; they are deleted when removed from the shelf.",
                    NOTE_WIDTH,
                ),
            )),
        ];
        let _ = self.ivars().sizes.set(sizes);
        up(&*column(mtm, &refs(&rows)))
    }
}

/// Hides the form row containing `view` (its superview).
fn set_row_hidden(view: &NSView, hidden: bool) {
    // SAFETY: The view is in a live hierarchy owned by the window.
    if let Some(row) = unsafe { view.superview() } {
        row.setHidden(hidden);
    }
}

/// Any view, as an owned `NSView`.
fn up<T: AsRef<NSView> + ?Sized>(view: &T) -> Retained<NSView> {
    view.as_ref().retain()
}

fn refs(views: &[Retained<NSView>]) -> Vec<&NSView> {
    views.iter().map(|view| &**view).collect()
}
