//! One shelf entry: icon or thumbnail, title, stack badge. Tiles are drag
//! sources and handle clicks, selection and their context menu.

use std::cell::{Cell, OnceCell, RefCell};
use std::path::Path;
use std::rc::Weak;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSColor, NSDragOperation, NSDraggingContext, NSDraggingItem, NSDraggingSession,
    NSDraggingSource, NSEvent, NSFont, NSImage, NSImageScaling, NSImageView, NSLayoutAttribute,
    NSLayoutConstraint, NSMenu, NSMenuItem, NSResponder, NSSharingService, NSStackView,
    NSTextAlignment, NSTextField, NSUserInterfaceLayoutOrientation, NSView, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSFileManager, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString, NSURL, ns_string,
};
use uuid::Uuid;

use super::actions::{self, ImageFormat, RESIZE_SIZES};
use super::item::{Entry, ItemKind};
use super::model::ShelfModel;
use super::pasteboard;
use super::prompt::RenamePrompt;
use crate::ui::menu_item;

pub const TILE_WIDTH: f64 = 96.0;
const ICON_SIZE: f64 = 56.0;
const BADGE_SIZE: f64 = 18.0;

pub struct TileIvars {
    entry_id: Uuid,
    is_stack: bool,
    pinned: bool,
    model: Weak<ShelfModel>,
    icon: OnceCell<Retained<NSImageView>>,
    /// Set while the mouse is down and a drag was attempted, so one press
    /// starts at most one drag session.
    dragging: Cell<bool>,
    /// What the Share submenu shares, set when the context menu is built.
    share_items: RefCell<Vec<Retained<AnyObject>>>,
}

define_class!(
    // SAFETY:
    // - NSView has no special subclassing requirements.
    // - `TileView` does not implement `Drop`.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "YoinksShelfTile"]
    #[ivars = TileIvars]
    pub struct TileView;

    impl TileView {
        // The shelf panel never activates the app; react to the first click.
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        // The icon and label are decoration: the tile handles every click on
        // it. Otherwise AppKit asks the image view or label whether it
        // accepts the first click into an inactive panel, they say no, and
        // the click is swallowed.
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            // SAFETY: Calling the superclass implementation of the same method.
            let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
            hit.map(|_| Retained::into_super(self.retain()))
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.ivars().dragging.set(false);
            self.with_model(|model, id| model.view().select(Some(id)));
            if event.clickCount() == 2 {
                self.with_model(|model, id| model.open(id));
            }
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            if !self.ivars().dragging.replace(true) {
                self.begin_drag(event);
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            self.ivars().dragging.set(false);
        }

        #[unsafe(method_id(menuForEvent:))]
        fn menu_for_event(&self, _event: &NSEvent) -> Option<Retained<NSMenu>> {
            self.with_model(|model, id| model.view().select(Some(id)));
            Some(self.context_menu())
        }

        #[unsafe(method(quickLookItem:))]
        fn quick_look_item(&self, _sender: Option<&AnyObject>) {
            self.with_model(|model, _| model.view().toggle_quick_look());
        }

        #[unsafe(method(openItem:))]
        fn open_item(&self, _sender: Option<&AnyObject>) {
            self.with_model(|model, id| model.open(id));
        }

        #[unsafe(method(revealItem:))]
        fn reveal_item(&self, _sender: Option<&AnyObject>) {
            self.with_model(|model, id| model.reveal(id));
        }

        #[unsafe(method(unstackItem:))]
        fn unstack_item(&self, _sender: Option<&AnyObject>) {
            self.with_model(|model, id| model.unstack(id));
        }

        #[unsafe(method(copyItem:))]
        fn copy_item(&self, _sender: Option<&AnyObject>) {
            self.with_model(|model, id| {
                if !model.copy(id) {
                    objc2_app_kit::NSBeep();
                }
            });
        }

        #[unsafe(method(copyItemPath:))]
        fn copy_item_path(&self, _sender: Option<&AnyObject>) {
            self.with_model(|model, id| {
                model.copy_paths(id);
            });
        }

        #[unsafe(method(openItemWith:))]
        fn open_item_with(&self, sender: Option<&NSMenuItem>) {
            let app = sender
                .and_then(|item| item.representedObject())
                .and_then(|object| object.downcast::<NSURL>().ok());
            if let Some(app) = app {
                self.with_model(|model, id| model.open_with(id, &app));
            }
        }

        #[unsafe(method(renameItem:))]
        fn rename_item(&self, _sender: Option<&AnyObject>) {
            let Some(model) = self.ivars().model.upgrade() else { return };
            let id = self.ivars().entry_id;
            let Some(current) = model.renamable_name(id) else { return };
            let weak = self.ivars().model.clone();
            RenamePrompt::show(self.mtm(), self, &current, move |name| match weak.upgrade() {
                Some(model) => model.rename(id, name),
                None => Ok(()),
            });
        }

        #[unsafe(method(compressItem:))]
        fn compress_item(&self, _sender: Option<&AnyObject>) {
            self.with_model(|model, id| {
                let paths = model.item_paths(id);
                model.run_job(self.mtm(), move |out| actions::compress(&paths, out));
            });
        }

        #[unsafe(method(convertImage:))]
        fn convert_image(&self, sender: Option<&NSMenuItem>) {
            let Some(index) = sender.map(|item| item.tag()) else { return };
            let Some(format) = usize::try_from(index).ok().and_then(|i| ImageFormat::ALL.get(i).copied()) else {
                return;
            };
            self.with_model(|model, id| {
                let images = model.image_paths(id);
                model.run_job(self.mtm(), move |out| actions::convert(&images, format, out));
            });
        }

        #[unsafe(method(resizeImage:))]
        fn resize_image(&self, sender: Option<&NSMenuItem>) {
            let Some(max) = sender.and_then(|item| u32::try_from(item.tag()).ok()) else { return };
            self.with_model(|model, id| {
                let images = model.image_paths(id);
                model.run_job(self.mtm(), move |out| actions::resize(&images, max, out));
            });
        }

        #[unsafe(method(shareItem:))]
        fn share_item(&self, sender: Option<&NSMenuItem>) {
            let service = sender
                .and_then(|item| item.representedObject())
                .and_then(|object| object.downcast::<NSSharingService>().ok());
            let items = self.ivars().share_items.borrow().clone();
            if let Some(service) = service
                && !items.is_empty()
            {
                // The service's window (AirDrop, Mail, ...) needs the app in front.
                crate::ui::activate_app(self.mtm());
                // SAFETY: Strings and URLs, which the services accept.
                unsafe { service.performWithItems(&NSArray::from_retained_slice(&items)) };
            }
        }

        #[unsafe(method(copyItemText:))]
        fn copy_item_text(&self, _sender: Option<&AnyObject>) {
            self.with_model(|model, id| model.copy_text(id));
        }

        #[unsafe(method(togglePinItem:))]
        fn toggle_pin_item(&self, _sender: Option<&AnyObject>) {
            self.with_model(|model, id| model.toggle_pin(id));
        }

        #[unsafe(method(removeItem:))]
        fn remove_item(&self, _sender: Option<&AnyObject>) {
            self.with_model(|model, id| model.view().remove(id));
        }
    }

    // SAFETY: `NSObjectProtocol` has no safety requirements.
    unsafe impl NSObjectProtocol for TileView {}

    // SAFETY: The method signatures match the protocol.
    unsafe impl NSDraggingSource for TileView {
        // Offer everything Finder understands and let it apply its usual
        // rules: move on the same volume, copy across volumes, Option to
        // copy, Command to move, Option-Command to make an alias. Within
        // Yoinks a drag is a move: dropped on the shelf, it reorders.
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn source_operation_mask(
            &self,
            _session: &NSDraggingSession,
            context: NSDraggingContext,
        ) -> NSDragOperation {
            if context == NSDraggingContext::WithinApplication {
                NSDragOperation::Move
            } else {
                NSDragOperation::Copy
                    | NSDragOperation::Move
                    | NSDragOperation::Generic
                    | NSDragOperation::Link
            }
        }

        #[unsafe(method(draggingSession:endedAtPoint:operation:))]
        fn dragging_ended(
            &self,
            _session: &NSDraggingSession,
            _point: NSPoint,
            operation: NSDragOperation,
        ) {
            self.ivars().dragging.set(false);
            let reordered = self
                .ivars()
                .model
                .upgrade()
                .is_some_and(|model| model.view().take_reordered(self.ivars().entry_id));
            if operation != NSDragOperation::None && !reordered {
                // Removing the entry takes this tile out of the view
                // hierarchy; keep it alive until this method returns.
                let _alive = self.retain();
                self.with_model(|model, id| {
                    model.dragged_out(id);
                    model.view().notify(super::view::ViewEvent::Used);
                });
            }
        }
    }
);

impl TileView {
    /// A tile for `entry`, showing `thumbnail` (the first item's Quick Look
    /// thumbnail) when there is one.
    pub fn new(
        mtm: MainThreadMarker,
        entry: &Entry,
        thumbnail: Option<&Path>,
        tooltip: &str,
        model: Weak<ShelfModel>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TileIvars {
            entry_id: entry.id,
            is_stack: entry.is_stack(),
            pinned: entry.pinned,
            model,
            icon: OnceCell::new(),
            dragging: Cell::new(false),
            share_items: RefCell::default(),
        });
        // SAFETY: NSView's `init` calls the designated `initWithFrame:`.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.setWantsLayer(true);
        if let Some(layer) = this.layer() {
            layer.setCornerRadius(8.0);
        }

        let image_view = NSImageView::imageViewWithImage(&icon(entry, thumbnail), mtm);
        image_view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        // Image views accept image drops when editable; keep drops going to
        // the shelf.
        image_view.unregisterDraggedTypes();
        let _ = this.ivars().icon.set(image_view.clone());

        let label = NSTextField::wrappingLabelWithString(&NSString::from_str(&entry.title()), mtm);
        label.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        label.setAlignment(NSTextAlignment::Center);
        label.setMaximumNumberOfLines(2);
        label.setPreferredMaxLayoutWidth(TILE_WIDTH - 8.0);
        if let Some(cell) = label.cell() {
            cell.setTruncatesLastVisibleLine(true);
        }
        if entry.has_pending() {
            label.setTextColor(Some(&NSColor::secondaryLabelColor()));
        }

        let column = NSStackView::stackViewWithViews(
            &NSArray::from_slice(&[&*image_view as &NSView, &*label]),
            mtm,
        );
        column.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        column.setAlignment(NSLayoutAttribute::CenterX);
        column.setSpacing(2.0);
        this.addSubview(&column);

        let views: [&NSView; 4] = [&this, &column, &image_view, &label];
        for view in views {
            view.setTranslatesAutoresizingMaskIntoConstraints(false);
        }
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            this.widthAnchor().constraintEqualToConstant(TILE_WIDTH),
            image_view
                .widthAnchor()
                .constraintEqualToConstant(ICON_SIZE),
            image_view
                .heightAnchor()
                .constraintEqualToConstant(ICON_SIZE),
            label
                .widthAnchor()
                .constraintLessThanOrEqualToConstant(TILE_WIDTH - 8.0),
            column
                .topAnchor()
                .constraintEqualToAnchor_constant(&this.topAnchor(), 4.0),
            column
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&this.bottomAnchor(), -4.0),
            column
                .centerXAnchor()
                .constraintEqualToAnchor(&this.centerXAnchor()),
        ]));

        if entry.is_stack() {
            this.add_badge(&image_view, entry.items.len());
        }
        if entry.pinned {
            this.add_pin(&image_view);
        }
        this.setToolTip(Some(&NSString::from_str(tooltip)));
        this
    }

    pub fn entry_id(&self) -> Uuid {
        self.ivars().entry_id
    }

    pub fn set_selected(&self, selected: bool) {
        let Some(layer) = self.layer() else { return };
        let color = selected.then(|| {
            NSColor::controlAccentColor()
                .colorWithAlphaComponent(0.3)
                .CGColor()
        });
        layer.setBackgroundColor(color.as_deref());
    }

    /// The icon's frame in screen coordinates, for Quick Look's zoom.
    pub fn icon_frame_on_screen(&self) -> Option<NSRect> {
        let icon = self.ivars().icon.get()?;
        let window = self.window()?;
        let in_window = icon.convertRect_toView(icon.bounds(), None);
        Some(window.convertRectToScreen(in_window))
    }

    /// A pin, top left of the icon, on pinned entries.
    fn add_pin(&self, icon: &NSImageView) {
        let mtm = self.mtm();
        let Some(symbol) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            ns_string!("pin.circle.fill"),
            Some(ns_string!("Pinned")),
        ) else {
            return;
        };
        let pin = NSImageView::imageViewWithImage(&symbol, mtm);
        pin.setContentTintColor(Some(&NSColor::systemOrangeColor()));
        pin.unregisterDraggedTypes();
        pin.setTranslatesAutoresizingMaskIntoConstraints(false);
        self.addSubview(&pin);
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            pin.widthAnchor().constraintEqualToConstant(BADGE_SIZE),
            pin.heightAnchor().constraintEqualToConstant(BADGE_SIZE),
            pin.centerXAnchor()
                .constraintEqualToAnchor_constant(&icon.leadingAnchor(), 2.0),
            pin.centerYAnchor()
                .constraintEqualToAnchor_constant(&icon.topAnchor(), 4.0),
        ]));
    }

    /// The item count, top right of the icon.
    fn add_badge(&self, icon: &NSImageView, count: usize) {
        let mtm = self.mtm();
        let badge = NSTextField::labelWithString(&NSString::from_str(&count.to_string()), mtm);
        badge.setFont(Some(&NSFont::boldSystemFontOfSize(10.0)));
        badge.setTextColor(Some(&NSColor::whiteColor()));
        badge.setAlignment(NSTextAlignment::Center);
        badge.setWantsLayer(true);
        if let Some(layer) = badge.layer() {
            layer.setCornerRadius(BADGE_SIZE / 2.0);
            layer.setBackgroundColor(Some(&NSColor::systemRedColor().CGColor()));
        }
        badge.setTranslatesAutoresizingMaskIntoConstraints(false);
        self.addSubview(&badge);
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            badge.heightAnchor().constraintEqualToConstant(BADGE_SIZE),
            badge
                .widthAnchor()
                .constraintGreaterThanOrEqualToConstant(BADGE_SIZE),
            badge
                .centerXAnchor()
                .constraintEqualToAnchor_constant(&icon.trailingAnchor(), -2.0),
            badge
                .centerYAnchor()
                .constraintEqualToAnchor_constant(&icon.topAnchor(), 4.0),
        ]));
    }

    /// Starts dragging every item of this entry out of the shelf, each with
    /// the tile's icon, fanned out slightly for stacks.
    fn begin_drag(&self, event: &NSEvent) {
        let Some(model) = self.ivars().model.upgrade() else {
            return;
        };
        let (image, frame) = match self.ivars().icon.get() {
            Some(icon) => (
                icon.image(),
                icon.convertRect_toView(icon.bounds(), Some(self)),
            ),
            None => (None, self.bounds()),
        };
        let items: Vec<Retained<NSDraggingItem>> = model
            .drag_items(self.ivars().entry_id)
            .iter()
            .filter_map(|item| pasteboard::writer(self.mtm(), item))
            .enumerate()
            .map(|(index, writer)| {
                let item =
                    NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), &writer);
                let offset = index.min(4) as f64 * 4.0;
                let frame = NSRect::new(
                    NSPoint::new(frame.origin.x + offset, frame.origin.y - offset),
                    frame.size,
                );
                let contents = image.as_deref().map(|image| image.as_ref());
                // SAFETY: `contents` is an NSImage, as the API expects.
                unsafe { item.setDraggingFrame_contents(frame, contents) };
                item
            })
            .collect();
        if !items.is_empty() {
            self.beginDraggingSessionWithItems_event_source(
                &NSArray::from_retained_slice(&items),
                event,
                ProtocolObject::from_ref(self),
            );
        }
    }

    fn with_model(&self, f: impl FnOnce(&ShelfModel, Uuid)) {
        if let Some(model) = self.ivars().model.upgrade() {
            f(&model, self.ivars().entry_id);
        }
    }

    fn context_menu(&self) -> Retained<NSMenu> {
        let mtm = self.mtm();
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        let target = Some::<&AnyObject>(self);
        let item = |title: &str, action: Sel, key: &str| menu_item(mtm, title, action, key, target);
        let model = self.ivars().model.upgrade();
        let id = self.ivars().entry_id;

        let quick_look = item("Quick Look", sel!(quickLookItem:), " ");
        quick_look.setKeyEquivalentModifierMask(objc2_app_kit::NSEventModifierFlags::empty());
        menu.addItem(&quick_look);
        menu.addItem(&item("Open", sel!(openItem:), ""));
        if let Some(open_with) = model
            .as_ref()
            .and_then(|m| self.open_with_menu(&m.open_urls(id)))
        {
            menu.addItem(&submenu_item(mtm, "Open With", &open_with));
        }
        menu.addItem(&item("Reveal in Finder", sel!(revealItem:), ""));
        if let Some(share) = self.share_menu(model.as_deref()) {
            let parent = NSMenuItem::new(mtm);
            parent.setTitle(ns_string!("Share"));
            parent.setSubmenu(Some(&share));
            menu.addItem(&parent);
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        menu.addItem(&item("Copy", sel!(copyItem:), "c"));
        menu.addItem(&item("Copy Path", sel!(copyItemPath:), ""));
        if model
            .as_ref()
            .is_some_and(|m| !m.image_paths(id).is_empty())
        {
            menu.addItem(&item("Copy Text", sel!(copyItemText:), ""));
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        if model
            .as_ref()
            .is_some_and(|m| m.renamable_name(id).is_some())
        {
            menu.addItem(&item("Rename…", sel!(renameItem:), ""));
        }
        menu.addItem(&item("Compress", sel!(compressItem:), ""));
        if model
            .as_ref()
            .is_some_and(|m| !m.image_paths(id).is_empty())
        {
            let convert = NSMenu::new(mtm);
            for (index, format) in ImageFormat::ALL.iter().enumerate() {
                let choice = item(format.label(), sel!(convertImage:), "");
                choice.setTag(index as isize);
                convert.addItem(&choice);
            }
            menu.addItem(&submenu_item(mtm, "Convert Image", &convert));

            let resize = NSMenu::new(mtm);
            for max in RESIZE_SIZES {
                let choice = item(&format!("{max} px"), sel!(resizeImage:), "");
                choice.setTag(max as isize);
                resize.addItem(&choice);
            }
            menu.addItem(&submenu_item(mtm, "Resize Image", &resize));
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        let pin_title = if self.ivars().pinned { "Unpin" } else { "Pin" };
        menu.addItem(&item(pin_title, sel!(togglePinItem:), ""));
        if self.ivars().is_stack {
            menu.addItem(&item("Unstack", sel!(unstackItem:), ""));
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&item("Remove", sel!(removeItem:), ""));
        menu
    }

    /// Share ▸ the services that can share the entry: AirDrop, Mail,
    /// Messages, Notes and others (like Finder's Share menu). The ready-made
    /// share picker needs an active app to show its popover, which Yoinks
    /// never is, so the services are listed directly.
    #[allow(deprecated)]
    fn share_menu(&self, model: Option<&ShelfModel>) -> Option<Retained<NSMenu>> {
        let mtm = self.mtm();
        let items = model?.share_items(self.ivars().entry_id);
        if items.is_empty() {
            return None;
        }
        let array = NSArray::from_retained_slice(&items);
        // SAFETY: Strings and URLs, which the services accept.
        let services = unsafe { NSSharingService::sharingServicesForItems(&array) };
        if services.is_empty() {
            return None;
        }
        let menu = NSMenu::new(mtm);
        for service in services.iter() {
            let entry = menu_item(
                mtm,
                &service.title().to_string(),
                sel!(shareItem:),
                "",
                Some(self.as_ref()),
            );
            let image = service.image();
            image.setSize(NSSize::new(16.0, 16.0));
            entry.setImage(Some(&image));
            // SAFETY: An NSSharingService is a valid represented object.
            unsafe { entry.setRepresentedObject(Some(&service)) };
            menu.addItem(&entry);
        }
        *self.ivars().share_items.borrow_mut() = items;
        Some(menu)
    }

    /// Apps that can open the first item, the default one first.
    fn open_with_menu(&self, urls: &[Retained<NSURL>]) -> Option<Retained<NSMenu>> {
        let mtm = self.mtm();
        let url = urls.first()?;
        let workspace = NSWorkspace::sharedWorkspace();
        let default = workspace.URLForApplicationToOpenURL(url);
        let mut apps: Vec<Retained<NSURL>> = workspace.URLsForApplicationsToOpenURL(url).to_vec();
        if apps.is_empty() {
            return None;
        }
        let name = |app: &NSURL| {
            app.path()
                .map(|path| {
                    NSFileManager::defaultManager()
                        .displayNameAtPath(&path)
                        .to_string()
                })
                .unwrap_or_default()
        };
        apps.sort_by_key(|app| name(app).to_lowercase());
        if let Some(default) = &default {
            apps.retain(|app| app.path() != default.path());
            apps.insert(0, default.clone());
        }
        let menu = NSMenu::new(mtm);
        for (index, app) in apps.iter().enumerate() {
            let mut title = name(app);
            if index == 0 && default.is_some() {
                title.push_str(" (default)");
            }
            let entry = menu_item(mtm, &title, sel!(openItemWith:), "", Some(self.as_ref()));
            if let Some(path) = app.path() {
                let icon = workspace.iconForFile(&path);
                icon.setSize(NSSize::new(16.0, 16.0));
                entry.setImage(Some(&icon));
            }
            // SAFETY: An NSURL is a valid represented object.
            unsafe { entry.setRepresentedObject(Some(app)) };
            menu.addItem(&entry);
            if index == 0 && default.is_some() && apps.len() > 1 {
                menu.addItem(&NSMenuItem::separatorItem(mtm));
            }
        }
        Some(menu)
    }
}

/// A menu item that opens `submenu`.
fn submenu_item(mtm: MainThreadMarker, title: &str, submenu: &NSMenu) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str(title));
    item.setSubmenu(Some(submenu));
    item
}

/// A Quick Look thumbnail when available; otherwise images show themselves,
/// stacks get Finder's multi-document icon and files their Finder icon.
fn icon(entry: &Entry, thumbnail: Option<&Path>) -> Retained<NSImage> {
    if entry.has_pending()
        && let Some(symbol) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            ns_string!("arrow.down.circle"),
            Some(ns_string!("Receiving")),
        )
    {
        return symbol;
    }
    let load = |path: &Path| {
        NSImage::initWithContentsOfFile(
            NSImage::alloc(),
            &NSString::from_str(&path.to_string_lossy()),
        )
    };
    if let Some(image) = thumbnail.and_then(load) {
        return image;
    }
    let workspace = NSWorkspace::sharedWorkspace();
    if entry.is_stack() {
        let paths: Vec<Retained<NSString>> = entry
            .items
            .iter()
            .map(|item| NSString::from_str(&item.path.to_string_lossy()))
            .collect();
        if let Some(icon) = workspace.iconForFiles(&NSArray::from_retained_slice(&paths)) {
            return icon;
        }
    }
    let item = &entry.items[0];
    if item.kind == ItemKind::Image
        && let Some(image) = load(&item.path)
    {
        return image;
    }
    workspace.iconForFile(&NSString::from_str(&item.path.to_string_lossy()))
}
