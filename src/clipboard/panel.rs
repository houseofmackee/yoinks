//! The clipboard history panel, shown under the menu bar icon: a search
//! field over a list of clips. Return or double-click copies a clip back to
//! the clipboard; the context menu adds it to the shelf, pins or deletes it.
//!
//! It is a non-activating panel (like Spotlight) rather than an NSPopover:
//! it takes keyboard input without bringing Yoinks to the front, so the app
//! the user was in stays active and Command-V works right after copying.

use std::cell::{OnceCell, RefCell};
use std::rc::{Rc, Weak};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBackingStoreType, NSBeep, NSButton, NSColor,
    NSControlTextEditingDelegate, NSFont, NSImage, NSImageView, NSLayoutConstraint,
    NSLineBreakMode, NSMenu, NSMenuItem, NSMenuItemValidation, NSPanel, NSResponder, NSScrollView,
    NSSearchField, NSSearchFieldDelegate, NSStatusWindowLevel, NSTableColumn,
    NSTableColumnResizingOptions, NSTableView, NSTableViewColumnAutoresizingStyle,
    NSTableViewDataSource, NSTableViewDelegate, NSTableViewStyle, NSTextField, NSTextFieldDelegate,
    NSTextView, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSVisualEffectView, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSIndexSet, NSInteger, NSNotification, NSObject, NSObjectProtocol,
    NSPoint, NSRect, NSSize, NSString, ns_string,
};
use uuid::Uuid;

use super::history::{Clip, ClipContent, age};
use super::watcher::ClipboardHistory;
use crate::shelf::{Dropped, unix_now};
use crate::ui::menu_item;

const WIDTH: f64 = 360.0;
const HEIGHT: f64 = 440.0;
const ROW_HEIGHT: f64 = 44.0;
const ICON: f64 = 28.0;

pub struct ListIvars {
    history: Weak<ClipboardHistory>,
    add_to_shelf: Box<dyn Fn(Dropped)>,
    /// Clips currently listed (after filtering).
    rows: RefCell<Vec<Clip>>,
    panel: OnceCell<Retained<HistoryPanel>>,
    table: OnceCell<Retained<NSTableView>>,
    search: OnceCell<Retained<NSSearchField>>,
    empty: OnceCell<Retained<NSTextField>>,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - `HistoryList` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "YoinksHistoryList"]
    #[ivars = ListIvars]
    pub struct HistoryList;

    // Actions from the table, its context menu and the footer button.
    impl HistoryList {
        #[unsafe(method(copyClip:))]
        fn copy_clip(&self, _sender: Option<&AnyObject>) {
            self.copy_target();
        }

        #[unsafe(method(addClipToShelf:))]
        fn add_clip_to_shelf(&self, _sender: Option<&AnyObject>) {
            let dropped = self
                .target_clip()
                .and_then(|id| self.ivars().history.upgrade()?.to_dropped(id));
            match dropped {
                Some(dropped) => {
                    self.close();
                    (self.ivars().add_to_shelf)(dropped);
                }
                None => NSBeep(),
            }
        }

        #[unsafe(method(togglePinClip:))]
        fn toggle_pin_clip(&self, _sender: Option<&AnyObject>) {
            if let (Some(id), Some(history)) = (self.target_clip(), self.ivars().history.upgrade()) {
                history.toggle_pin(id);
            }
        }

        #[unsafe(method(deleteClip:))]
        fn delete_clip(&self, _sender: Option<&AnyObject>) {
            if let (Some(id), Some(history)) = (self.target_clip(), self.ivars().history.upgrade()) {
                history.remove(id);
            }
        }

        #[unsafe(method(clearHistory:))]
        fn clear_history(&self, _sender: Option<&AnyObject>) {
            if let Some(history) = self.ivars().history.upgrade() {
                history.clear();
            }
        }
    }

    // SAFETY: `NSObjectProtocol` has no safety requirements.
    unsafe impl NSObjectProtocol for HistoryList {}

    // SAFETY: The method signatures match the protocol.
    unsafe impl NSTableViewDataSource for HistoryList {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, _table: &NSTableView) -> NSInteger {
            self.ivars().rows.borrow().len() as NSInteger
        }
    }

    // SAFETY: The method signatures match the protocol.
    unsafe impl NSTableViewDelegate for HistoryList {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn view_for_row(
            &self,
            _table: &NSTableView,
            _column: Option<&NSTableColumn>,
            row: NSInteger,
        ) -> Option<Retained<NSView>> {
            let rows = self.ivars().rows.borrow();
            usize::try_from(row)
                .ok()
                .and_then(|row| rows.get(row))
                .map(|clip| row_view(self.mtm(), clip))
        }
    }

    // SAFETY: The method signatures match the protocol.
    unsafe impl NSControlTextEditingDelegate for HistoryList {
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, _notification: &NSNotification) {
            self.refresh();
            self.select_row(0);
        }

        // Keys typed in the search field drive the list.
        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn do_command(&self, _control: &objc2_app_kit::NSControl, _text_view: &NSTextView, command: Sel) -> bool {
            self.handle_command(command)
        }
    }

    // SAFETY: No required methods.
    unsafe impl NSTextFieldDelegate for HistoryList {}

    // SAFETY: No required methods.
    unsafe impl NSSearchFieldDelegate for HistoryList {}

    // SAFETY: The method signature matches the protocol.
    unsafe impl NSMenuItemValidation for HistoryList {
        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            let clip = self.target_clip().and_then(|id| {
                self.ivars().rows.borrow().iter().find(|c| c.id == id).cloned()
            });
            if item.action() == Some(sel!(togglePinClip:)) {
                let pinned = clip.as_ref().is_some_and(|c| c.pinned);
                item.setTitle(if pinned { ns_string!("Unpin") } else { ns_string!("Pin") });
            }
            item.action() == Some(sel!(clearHistory:)) || clip.is_some()
        }
    }
);

impl HistoryList {
    /// Builds the panel. `add_to_shelf` receives clips sent to the shelf.
    pub fn new(
        mtm: MainThreadMarker,
        history: &Rc<ClipboardHistory>,
        add_to_shelf: impl Fn(Dropped) + 'static,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ListIvars {
            history: Rc::downgrade(history),
            add_to_shelf: Box::new(add_to_shelf),
            rows: RefCell::default(),
            panel: OnceCell::new(),
            table: OnceCell::new(),
            search: OnceCell::new(),
            empty: OnceCell::new(),
        });
        // SAFETY: NSObject's `init` has the correct signature.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.build();

        let weak = objc2::rc::Weak::from_retained(&this);
        history.set_on_change(move || {
            if let Some(list) = weak.load() {
                list.refresh();
            }
        });
        this
    }

    pub fn is_shown(&self) -> bool {
        self.ivars().panel.get().is_some_and(|p| p.isVisible())
    }

    /// Shows the panel under `anchor` (the menu bar button), or closes it.
    pub fn toggle(&self, anchor: &NSView) {
        if self.is_shown() {
            self.close();
            return;
        }
        let Some(panel) = self.ivars().panel.get() else {
            return;
        };
        if let Some(search) = self.ivars().search.get() {
            search.setStringValue(ns_string!(""));
        }
        self.refresh();
        self.select_row(0);
        if let Some(origin) = panel_origin(anchor) {
            panel.setFrameOrigin(origin);
        }
        panel.orderFrontRegardless();
        panel.makeKeyWindow();
        if let Some(search) = self.ivars().search.get() {
            panel.makeFirstResponder(Some(search));
        }
    }

    fn close(&self) {
        if let Some(panel) = self.ivars().panel.get() {
            panel.orderOut(None);
        }
    }

    /// Reloads the list from the history, applying the search text.
    fn refresh(&self) {
        let ivars = self.ivars();
        let query = ivars
            .search
            .get()
            .map(|s| s.stringValue().to_string())
            .unwrap_or_default();
        let rows = ivars
            .history
            .upgrade()
            .map(|h| h.search(&query))
            .unwrap_or_default();
        let selected = ivars.table.get().map_or(-1, |t| t.selectedRow());
        let empty = rows.is_empty();
        *ivars.rows.borrow_mut() = rows;
        if let Some(table) = ivars.table.get() {
            table.reloadData();
            if selected >= 0 {
                self.select_row(selected);
            }
        }
        if let Some(label) = ivars.empty.get() {
            label.setHidden(!empty);
            label.setStringValue(if query.trim().is_empty() {
                ns_string!("Nothing copied yet")
            } else {
                ns_string!("No matches")
            });
        }
    }

    /// Keys typed in the search field: arrows move through the list, Return
    /// copies, Escape clears the search or closes the panel.
    fn handle_command(&self, command: Sel) -> bool {
        if command == sel!(moveDown:) {
            self.move_selection(1);
        } else if command == sel!(moveUp:) {
            self.move_selection(-1);
        } else if command == sel!(insertNewline:) {
            self.copy_target();
        } else if command == sel!(cancelOperation:) {
            let search = self.ivars().search.get();
            match search.filter(|s| s.stringValue().length() > 0) {
                Some(search) => {
                    search.setStringValue(ns_string!(""));
                    self.refresh();
                }
                None => self.close(),
            }
        } else {
            return false;
        }
        true
    }

    fn select_row(&self, row: NSInteger) {
        let Some(table) = self.ivars().table.get() else {
            return;
        };
        let count = self.ivars().rows.borrow().len() as NSInteger;
        if count == 0 {
            return;
        }
        let row = row.clamp(0, count - 1);
        table.selectRowIndexes_byExtendingSelection(
            &NSIndexSet::indexSetWithIndex(row as usize),
            false,
        );
        table.scrollRowToVisible(row);
    }

    fn move_selection(&self, delta: NSInteger) {
        let current = self.ivars().table.get().map_or(-1, |t| t.selectedRow());
        self.select_row(if current < 0 { 0 } else { current + delta });
    }

    /// The clip a menu command applies to: the right-clicked row, else the
    /// selected one.
    fn target_clip(&self) -> Option<Uuid> {
        let table = self.ivars().table.get()?;
        let row = match table.clickedRow() {
            -1 => table.selectedRow(),
            clicked => clicked,
        };
        let rows = self.ivars().rows.borrow();
        rows.get(usize::try_from(row).ok()?).map(|clip| clip.id)
    }

    /// Copies the target clip and closes the panel; keyboard focus returns
    /// to the app the user was in.
    fn copy_target(&self) {
        let copied = self
            .target_clip()
            .zip(self.ivars().history.upgrade())
            .is_some_and(|(id, history)| history.copy(id));
        if copied {
            self.close();
        } else {
            NSBeep();
        }
    }

    fn build(&self) {
        let mtm = self.mtm();
        let target = Some::<&AnyObject>(self.as_ref());

        let search = NSSearchField::new(mtm);
        search.setPlaceholderString(Some(ns_string!("Search clipboard history")));
        // SAFETY: We implement the delegate protocol and outlive the field.
        unsafe { search.setDelegate(Some(ProtocolObject::from_ref(self))) };

        let table = NSTableView::new(mtm);
        let column =
            NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), ns_string!("clip"));
        column.setResizingMask(NSTableColumnResizingOptions::AutoresizingMask);
        table.addTableColumn(&column);
        table.setHeaderView(None);
        table.setRowHeight(ROW_HEIGHT);
        table.setStyle(NSTableViewStyle::Inset);
        table.setColumnAutoresizingStyle(
            NSTableViewColumnAutoresizingStyle::FirstColumnOnlyAutoresizingStyle,
        );
        // SAFETY: We implement both protocols and outlive the table.
        unsafe {
            table.setDataSource(Some(ProtocolObject::from_ref(self)));
            table.setDelegate(Some(ProtocolObject::from_ref(self)));
            table.setTarget(target);
            table.setDoubleAction(Some(sel!(copyClip:)));
        }
        let menu = NSMenu::new(mtm);
        for (title, action) in [
            ("Copy", sel!(copyClip:)),
            ("Add to Shelf", sel!(addClipToShelf:)),
            ("Pin", sel!(togglePinClip:)),
        ] {
            menu.addItem(&menu_item(mtm, title, action, "", target));
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&menu_item(mtm, "Delete", sel!(deleteClip:), "", target));
        // SAFETY: Setting a context menu on a view is always allowed.
        unsafe { table.setMenu(Some(&menu)) };

        let scroll = NSScrollView::new(mtm);
        scroll.setDocumentView(Some(&table));
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);

        let empty = NSTextField::labelWithString(ns_string!("Nothing copied yet"), mtm);
        empty.setTextColor(Some(&NSColor::secondaryLabelColor()));

        // SAFETY: The action takes one sender argument; we are the target.
        let clear = unsafe {
            NSButton::buttonWithTitle_target_action(
                ns_string!("Clear Unpinned"),
                target,
                Some(sel!(clearHistory:)),
                mtm,
            )
        };
        let hint =
            NSTextField::labelWithString(ns_string!("Return copies · right-click for more"), mtm);
        hint.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        hint.setTextColor(Some(&NSColor::secondaryLabelColor()));

        let container = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT)),
        );
        let views: [&NSView; 5] = [&search, &scroll, &empty, &clear, &hint];
        for view in views {
            view.setTranslatesAutoresizingMaskIntoConstraints(false);
            container.addSubview(view);
        }
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            search
                .topAnchor()
                .constraintEqualToAnchor_constant(&container.topAnchor(), 12.0),
            search
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&container.leadingAnchor(), 12.0),
            search
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&container.trailingAnchor(), -12.0),
            scroll
                .topAnchor()
                .constraintEqualToAnchor_constant(&search.bottomAnchor(), 8.0),
            scroll
                .leadingAnchor()
                .constraintEqualToAnchor(&container.leadingAnchor()),
            scroll
                .trailingAnchor()
                .constraintEqualToAnchor(&container.trailingAnchor()),
            scroll
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&clear.topAnchor(), -8.0),
            empty
                .centerXAnchor()
                .constraintEqualToAnchor(&scroll.centerXAnchor()),
            empty
                .centerYAnchor()
                .constraintEqualToAnchor(&scroll.centerYAnchor()),
            clear
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&container.leadingAnchor(), 12.0),
            clear
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&container.bottomAnchor(), -10.0),
            hint.trailingAnchor()
                .constraintEqualToAnchor_constant(&container.trailingAnchor(), -12.0),
            hint.centerYAnchor()
                .constraintEqualToAnchor(&clear.centerYAnchor()),
        ]));

        let panel = HistoryPanel::new(mtm);
        let background = panel_background(mtm);
        background.addSubview(&container);
        container.setFrame(background.bounds());
        container.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        panel.setContentView(Some(&background));

        let ivars = self.ivars();
        let _ = ivars.panel.set(panel);
        let _ = ivars.table.set(table);
        let _ = ivars.search.set(search);
        let _ = ivars.empty.set(empty);
    }
}

define_class!(
    // SAFETY:
    // - NSPanel has no special subclassing requirements.
    // - `HistoryPanel` does not implement `Drop`.
    #[unsafe(super(NSPanel, NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "YoinksHistoryPanel"]
    pub struct HistoryPanel;

    impl HistoryPanel {
        // Borderless windows refuse key status by default; the search field
        // needs it.
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }

        // Clicking anywhere else closes the panel, like a menu.
        #[unsafe(method(resignKeyWindow))]
        fn resign_key_window(&self) {
            // SAFETY: Calling the superclass implementation of the same method.
            let _: () = unsafe { msg_send![super(self), resignKeyWindow] };
            self.orderOut(None);
        }

        #[unsafe(method(cancelOperation:))]
        fn cancel_operation(&self, _sender: Option<&AnyObject>) {
            self.orderOut(None);
        }
    }
);

impl HistoryPanel {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT));
        let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
        // SAFETY: Inherited designated initializer; `HistoryPanel` has no ivars.
        let panel: Retained<Self> = unsafe {
            msg_send![
                Self::alloc(mtm),
                initWithContentRect: rect,
                styleMask: style,
                backing: NSBackingStoreType::Buffered,
                defer: true,
            ]
        };
        // SAFETY: The list holds a strong reference to the panel.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setFloatingPanel(true);
        panel.setLevel(NSStatusWindowLevel);
        panel.setHidesOnDeactivate(false);
        crate::ui::follow_active_space(&panel);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(true);
        panel
    }
}

/// Rounded, translucent background like a menu.
fn panel_background(mtm: MainThreadMarker) -> Retained<NSVisualEffectView> {
    let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT));
    let view = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), rect);
    view.setMaterial(NSVisualEffectMaterial::Menu);
    view.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    view.setState(NSVisualEffectState::Active);
    view.setWantsLayer(true);
    if let Some(layer) = view.layer() {
        layer.setCornerRadius(12.0);
        layer.setMasksToBounds(true);
    }
    view
}

/// Where the panel goes: centered under `anchor` when it is the menu bar
/// icon; beside the anchor's window (the shelf) otherwise. Always kept on the
/// anchor's screen.
fn panel_origin(anchor: &NSView) -> Option<NSPoint> {
    let window = anchor.window()?;
    let button = window.convertRectToScreen(anchor.convertRect_toView(anchor.bounds(), None));
    let visible = window.screen()?.visibleFrame();
    let min_x = visible.origin.x + 8.0;
    let max_x = (visible.origin.x + visible.size.width - WIDTH - 8.0).max(min_x);
    let min_y = visible.origin.y + 8.0;
    let max_y = (visible.origin.y + visible.size.height - HEIGHT - 8.0).max(min_y);
    let in_menu_bar = window.level() >= NSStatusWindowLevel;
    let (x, y) = if in_menu_bar {
        (
            button.origin.x + button.size.width / 2.0 - WIDTH / 2.0,
            button.origin.y - HEIGHT - 6.0,
        )
    } else {
        let frame = window.frame();
        let left = frame.origin.x - WIDTH - 8.0;
        let x = if left >= min_x {
            left
        } else {
            frame.origin.x + frame.size.width + 8.0
        };
        (x, button.origin.y)
    };
    Some(NSPoint::new(x.clamp(min_x, max_x), y.clamp(min_y, max_y)))
}

/// One clip: icon, one-line title, and "source · age" below.
fn row_view(mtm: MainThreadMarker, clip: &Clip) -> Retained<NSView> {
    let row = NSView::new(mtm);
    let icon = NSImageView::imageViewWithImage(&clip_icon(clip), mtm);
    let title = NSTextField::labelWithString(&NSString::from_str(&clip.content.title()), mtm);
    title.setFont(Some(&NSFont::systemFontOfSize(13.0)));
    title.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    let mut details = Vec::new();
    if clip.pinned {
        details.push("Pinned".to_owned());
    }
    if let Some(name) = clip.source.as_ref().and_then(|s| s.name.clone()) {
        details.push(name);
    }
    details.push(age(unix_now(), clip.copied_at));
    let subtitle = NSTextField::labelWithString(&NSString::from_str(&details.join(" · ")), mtm);
    subtitle.setFont(Some(&NSFont::systemFontOfSize(11.0)));
    subtitle.setTextColor(Some(&NSColor::secondaryLabelColor()));
    subtitle.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);

    let views: [&NSView; 3] = [&icon, &title, &subtitle];
    for view in views {
        view.setTranslatesAutoresizingMaskIntoConstraints(false);
        row.addSubview(view);
    }
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        icon.widthAnchor().constraintEqualToConstant(ICON),
        icon.heightAnchor().constraintEqualToConstant(ICON),
        icon.leadingAnchor()
            .constraintEqualToAnchor_constant(&row.leadingAnchor(), 4.0),
        icon.centerYAnchor()
            .constraintEqualToAnchor(&row.centerYAnchor()),
        title
            .leadingAnchor()
            .constraintEqualToAnchor_constant(&icon.trailingAnchor(), 8.0),
        title
            .trailingAnchor()
            .constraintLessThanOrEqualToAnchor_constant(&row.trailingAnchor(), -4.0),
        title
            .bottomAnchor()
            .constraintEqualToAnchor_constant(&row.centerYAnchor(), 1.0),
        subtitle
            .leadingAnchor()
            .constraintEqualToAnchor(&title.leadingAnchor()),
        subtitle
            .trailingAnchor()
            .constraintLessThanOrEqualToAnchor_constant(&row.trailingAnchor(), -4.0),
        subtitle
            .topAnchor()
            .constraintEqualToAnchor_constant(&row.centerYAnchor(), 2.0),
    ]));
    row
}

fn clip_icon(clip: &Clip) -> Retained<NSImage> {
    let workspace = NSWorkspace::sharedWorkspace();
    let symbol = |name: &str| {
        NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)
    };
    let image = match &clip.content {
        ClipContent::Image { file, .. } => NSImage::initWithContentsOfFile(
            NSImage::alloc(),
            &NSString::from_str(&file.to_string_lossy()),
        ),
        ClipContent::Files { paths } => paths
            .first()
            .map(|path| workspace.iconForFile(&NSString::from_str(&path.to_string_lossy()))),
        ClipContent::Url { .. } => symbol("link"),
        ClipContent::Text { .. } | ClipContent::RichText { .. } => clip
            .source
            .as_ref()
            .and_then(|s| s.bundle_id.as_deref())
            .and_then(|id| workspace.URLForApplicationWithBundleIdentifier(&NSString::from_str(id)))
            .and_then(|url| url.path())
            .map(|path| workspace.iconForFile(&path))
            .or_else(|| symbol("doc.text")),
    };
    image.unwrap_or_else(|| workspace.iconForFile(ns_string!("/")))
}
