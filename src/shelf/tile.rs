//! One shelf entry: icon or thumbnail, title, stack badge. Tiles are drag
//! sources and handle clicks, selection and their context menu.

use std::cell::{Cell, OnceCell};
use std::path::Path;
use std::rc::Weak;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSColor, NSDragOperation, NSDraggingContext, NSDraggingItem, NSDraggingSession,
    NSDraggingSource, NSEvent, NSFont, NSImage, NSImageScaling, NSImageView, NSLayoutAttribute,
    NSLayoutConstraint, NSMenu, NSMenuItem, NSResponder, NSStackView, NSTextAlignment, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSObject, NSObjectProtocol, NSPoint, NSRect, NSString, ns_string,
};
use uuid::Uuid;

use super::item::{Entry, ItemKind};
use super::model::ShelfModel;
use super::pasteboard;
use crate::ui::menu_item;

pub const TILE_WIDTH: f64 = 96.0;
const ICON_SIZE: f64 = 56.0;
const BADGE_SIZE: f64 = 18.0;

pub struct TileIvars {
    entry_id: Uuid,
    is_stack: bool,
    model: Weak<ShelfModel>,
    icon: OnceCell<Retained<NSImageView>>,
    /// Set while the mouse is down and a drag was attempted, so one press
    /// starts at most one drag session.
    dragging: Cell<bool>,
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
        // copy, Command to move, Option-Command to make an alias. Dropping
        // back onto the shelf itself is refused.
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn source_operation_mask(
            &self,
            _session: &NSDraggingSession,
            context: NSDraggingContext,
        ) -> NSDragOperation {
            if context == NSDraggingContext::WithinApplication {
                NSDragOperation::None
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
            if operation != NSDragOperation::None {
                // Removing the entry takes this tile out of the view
                // hierarchy; keep it alive until this method returns.
                let _alive = self.retain();
                self.with_model(|model, id| model.dragged_out(id));
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
            model,
            icon: OnceCell::new(),
            dragging: Cell::new(false),
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
        let quick_look = menu_item(mtm, "Quick Look", sel!(quickLookItem:), " ", target);
        quick_look.setKeyEquivalentModifierMask(objc2_app_kit::NSEventModifierFlags::empty());
        menu.addItem(&quick_look);
        menu.addItem(&menu_item(mtm, "Open", sel!(openItem:), "", target));
        menu.addItem(&menu_item(
            mtm,
            "Reveal in Finder",
            sel!(revealItem:),
            "",
            target,
        ));
        if self.ivars().is_stack {
            menu.addItem(&menu_item(mtm, "Unstack", sel!(unstackItem:), "", target));
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&menu_item(mtm, "Remove", sel!(removeItem:), "", target));
        menu
    }
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
