//! An editable list of apps (by bundle id) with + and - buttons. "+" offers
//! the apps that are currently running.

use std::cell::{OnceCell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplicationActivationPolicy, NSBezelStyle, NSButton, NSControlTextEditingDelegate, NSImage,
    NSImageView, NSLayoutAttribute, NSLayoutConstraint, NSMenu, NSMenuItem, NSScrollView,
    NSStackView, NSTableColumn, NSTableView, NSTableViewDataSource, NSTableViewDelegate,
    NSTableViewStyle, NSTextField, NSUserInterfaceLayoutOrientation, NSView, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSFileManager, NSInteger, NSObject, NSObjectProtocol, NSPoint,
    NSSize, NSString, ns_string,
};

const ROW_HEIGHT: f64 = 24.0;

pub struct AppListIvars {
    bundle_ids: RefCell<Vec<String>>,
    on_change: Box<dyn Fn(Vec<String>)>,
    table: OnceCell<Retained<NSTableView>>,
    add: OnceCell<Retained<NSButton>>,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - `AppList` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "YoinksAppList"]
    #[ivars = AppListIvars]
    pub struct AppList;

    impl AppList {
        #[unsafe(method(showAddMenu:))]
        fn show_add_menu(&self, _sender: Option<&AnyObject>) {
            let Some(button) = self.ivars().add.get() else { return };
            let menu = self.running_apps_menu();
            menu.popUpMenuPositioningItem_atLocation_inView(
                None,
                NSPoint::new(0.0, button.bounds().size.height + 4.0),
                Some(button),
            );
        }

        #[unsafe(method(addApp:))]
        fn add_app(&self, sender: Option<&NSMenuItem>) {
            let id = sender
                .and_then(|item| item.representedObject())
                .and_then(|object| object.downcast::<NSString>().ok())
                .map(|id| id.to_string());
            if let Some(id) = id {
                let mut ids = self.ivars().bundle_ids.borrow_mut();
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            self.changed();
        }

        #[unsafe(method(removeApp:))]
        fn remove_app(&self, _sender: Option<&AnyObject>) {
            let Some(table) = self.ivars().table.get() else { return };
            let Ok(row) = usize::try_from(table.selectedRow()) else { return };
            {
                let mut ids = self.ivars().bundle_ids.borrow_mut();
                if row < ids.len() {
                    ids.remove(row);
                }
            }
            self.changed();
        }
    }

    // SAFETY: `NSObjectProtocol` has no safety requirements.
    unsafe impl NSObjectProtocol for AppList {}

    // SAFETY: The method signatures match the protocol.
    unsafe impl NSTableViewDataSource for AppList {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, _table: &NSTableView) -> NSInteger {
            self.ivars().bundle_ids.borrow().len() as NSInteger
        }
    }

    // SAFETY: No required methods; needed by NSTableViewDelegate.
    unsafe impl NSControlTextEditingDelegate for AppList {}

    // SAFETY: The method signatures match the protocol.
    unsafe impl NSTableViewDelegate for AppList {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn view_for_row(
            &self,
            _table: &NSTableView,
            _column: Option<&NSTableColumn>,
            row: NSInteger,
        ) -> Option<Retained<NSView>> {
            let ids = self.ivars().bundle_ids.borrow();
            usize::try_from(row)
                .ok()
                .and_then(|row| ids.get(row))
                .map(|id| app_row(self.mtm(), id))
        }
    }
);

impl AppList {
    pub fn new(
        mtm: MainThreadMarker,
        bundle_ids: Vec<String>,
        on_change: impl Fn(Vec<String>) + 'static,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(AppListIvars {
            bundle_ids: RefCell::new(bundle_ids),
            on_change: Box::new(on_change),
            table: OnceCell::new(),
            add: OnceCell::new(),
        });
        // SAFETY: NSObject's `init` has the correct signature.
        unsafe { msg_send![super(this), init] }
    }

    /// The list and its +/- buttons, `width` by `height` points.
    pub fn view(&self, width: f64, height: f64) -> Retained<NSView> {
        let mtm = self.mtm();
        let table = NSTableView::new(mtm);
        let column =
            NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), ns_string!("app"));
        table.addTableColumn(&column);
        table.setHeaderView(None);
        table.setRowHeight(ROW_HEIGHT);
        table.setStyle(NSTableViewStyle::FullWidth);
        // SAFETY: We implement both protocols and outlive the table.
        unsafe {
            table.setDataSource(Some(ProtocolObject::from_ref(self)));
            table.setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        let scroll = NSScrollView::new(mtm);
        scroll.setDocumentView(Some(&table));
        scroll.setHasVerticalScroller(true);
        scroll.setBorderType(objc2_app_kit::NSBorderType::BezelBorder);

        let target = Some::<&AnyObject>(self.as_ref());
        // SAFETY: The actions take one sender argument; we are the target.
        let (add, remove) = unsafe {
            (
                NSButton::buttonWithTitle_target_action(
                    ns_string!("+"),
                    target,
                    Some(sel!(showAddMenu:)),
                    mtm,
                ),
                NSButton::buttonWithTitle_target_action(
                    ns_string!("−"),
                    target,
                    Some(sel!(removeApp:)),
                    mtm,
                ),
            )
        };
        for button in [&add, &remove] {
            button.setBezelStyle(NSBezelStyle::SmallSquare);
        }
        let buttons = NSStackView::stackViewWithViews(
            &NSArray::from_slice(&[&*add as &NSView, &*remove]),
            mtm,
        );
        buttons.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        buttons.setSpacing(0.0);

        let stack = NSStackView::stackViewWithViews(
            &NSArray::from_slice(&[&*scroll as &NSView, &*buttons]),
            mtm,
        );
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setAlignment(NSLayoutAttribute::Leading);
        stack.setSpacing(0.0);
        scroll.setTranslatesAutoresizingMaskIntoConstraints(false);
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            scroll.widthAnchor().constraintEqualToConstant(width),
            scroll.heightAnchor().constraintEqualToConstant(height),
        ]));
        let _ = self.ivars().table.set(table);
        let _ = self.ivars().add.set(add);
        Retained::into_super(stack)
    }

    fn changed(&self) {
        if let Some(table) = self.ivars().table.get() {
            table.reloadData();
        }
        (self.ivars().on_change)(self.ivars().bundle_ids.borrow().clone());
    }

    /// Regular (Dock) apps that are running and not yet listed.
    fn running_apps_menu(&self) -> Retained<NSMenu> {
        let mtm = self.mtm();
        let menu = NSMenu::new(mtm);
        let listed = self.ivars().bundle_ids.borrow().clone();
        let mut apps: Vec<(String, String, Option<Retained<NSImage>>)> =
            NSWorkspace::sharedWorkspace()
                .runningApplications()
                .iter()
                .filter(|app| app.activationPolicy() == NSApplicationActivationPolicy::Regular)
                .filter_map(|app| {
                    let id = app.bundleIdentifier()?.to_string();
                    let name = app
                        .localizedName()
                        .map_or_else(|| id.clone(), |n| n.to_string());
                    Some((name, id, app.icon()))
                })
                .filter(|(_, id, _)| !listed.contains(id))
                .collect();
        apps.sort_by_key(|(name, _, _)| name.to_lowercase());
        apps.dedup_by(|a, b| a.1 == b.1);
        for (name, id, icon) in apps {
            let item = crate::ui::menu_item(mtm, &name, sel!(addApp:), "", Some(self.as_ref()));
            if let Some(icon) = icon {
                icon.setSize(NSSize::new(16.0, 16.0));
                item.setImage(Some(&icon));
            }
            // SAFETY: An NSString is a valid represented object.
            unsafe { item.setRepresentedObject(Some(&NSString::from_str(&id))) };
            menu.addItem(&item);
        }
        if menu.numberOfItems() == 0 {
            let item = NSMenuItem::new(mtm);
            item.setTitle(ns_string!("No other apps are running"));
            item.setEnabled(false);
            menu.addItem(&item);
        }
        menu
    }
}

/// App icon and name for a bundle id (the id itself if the app is not
/// installed).
fn app_row(mtm: MainThreadMarker, bundle_id: &str) -> Retained<NSView> {
    let workspace = NSWorkspace::sharedWorkspace();
    let path = workspace
        .URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))
        .and_then(|url| url.path());
    let name = path.as_ref().map_or_else(
        || NSString::from_str(bundle_id),
        |path| NSFileManager::defaultManager().displayNameAtPath(path),
    );
    let icon = path.as_ref().map(|path| workspace.iconForFile(path));
    let label = NSTextField::labelWithString(&name, mtm);
    let mut views: Vec<Retained<NSView>> = Vec::new();
    if let Some(icon) = icon {
        let image = NSImageView::imageViewWithImage(&icon, mtm);
        image.setTranslatesAutoresizingMaskIntoConstraints(false);
        image
            .widthAnchor()
            .constraintEqualToConstant(16.0)
            .setActive(true);
        image
            .heightAnchor()
            .constraintEqualToConstant(16.0)
            .setActive(true);
        views.push(Retained::into_super(Retained::into_super(image)));
    }
    views.push(Retained::into_super(Retained::into_super(label)));
    let row = NSStackView::stackViewWithViews(&NSArray::from_retained_slice(&views), mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.setSpacing(6.0);
    row.setEdgeInsets(objc2_foundation::NSEdgeInsets {
        top: 0.0,
        left: 4.0,
        bottom: 0.0,
        right: 4.0,
    });
    Retained::into_super(row)
}
