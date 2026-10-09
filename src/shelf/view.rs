//! The shelf's content: a drop target holding a scrolling column of tiles.

use std::cell::{Cell, OnceCell};
use std::rc::Weak;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSColor, NSDragOperation, NSDraggingContext, NSDraggingInfo, NSDraggingItem, NSDraggingSession,
    NSDraggingSource, NSEvent, NSFont, NSImage, NSImageScaling, NSImageView, NSLayoutAttribute,
    NSLayoutConstraint, NSMenu, NSResponder, NSScrollView, NSStackView, NSTextAlignment,
    NSTextField, NSUserInterfaceLayoutOrientation, NSView, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSEdgeInsets, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString, ns_string,
};
use uuid::Uuid;

use super::item::{Entry, ItemKind};
use super::model::ShelfModel;
use super::pasteboard;
use crate::ui::menu_item;

const CORNER_RADIUS: f64 = 14.0;
const TILE_WIDTH: f64 = 96.0;
const ICON_SIZE: f64 = 56.0;

pub struct ContentIvars {
    model: Weak<ShelfModel>,
    list: Retained<NSStackView>,
    placeholder: Retained<NSTextField>,
}

define_class!(
    // SAFETY:
    // - NSVisualEffectView has no special subclassing requirements.
    // - `ShelfContentView` does not implement `Drop`.
    #[unsafe(super(NSVisualEffectView, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "YoinksShelfContentView"]
    #[ivars = ContentIvars]
    pub struct ShelfContentView;

    // NSDraggingDestination (informal on NSView).
    impl ShelfContentView {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            let operation = drop_operation(info);
            self.set_highlighted(operation != NSDragOperation::None);
            operation
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            drop_operation(info)
        }

        #[unsafe(method(draggingExited:))]
        fn dragging_exited(&self, _info: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            self.set_highlighted(false);
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag_operation(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            self.set_highlighted(false);
            let dropped = pasteboard::read(&info.draggingPasteboard());
            match self.ivars().model.upgrade() {
                Some(model) => model.accept(dropped),
                None => false,
            }
        }
    }
);

/// Files stay where they are, so the drop is a reference rather than a copy
/// or move. Prefer an operation that shows no badge on the cursor.
fn drop_operation(info: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
    let mask = info.draggingSourceOperationMask();
    [
        NSDragOperation::Generic,
        NSDragOperation::Copy,
        NSDragOperation::Link,
    ]
    .into_iter()
    .find(|operation| mask.contains(*operation))
    .unwrap_or(NSDragOperation::None)
}

impl ShelfContentView {
    pub fn new(mtm: MainThreadMarker, model: Weak<ShelfModel>) -> Retained<Self> {
        let list = NSStackView::new(mtm);
        list.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        list.setAlignment(NSLayoutAttribute::CenterX);
        list.setSpacing(6.0);
        list.setEdgeInsets(NSEdgeInsets {
            top: 10.0,
            left: 0.0,
            bottom: 10.0,
            right: 0.0,
        });

        let placeholder = NSTextField::wrappingLabelWithString(ns_string!("Drop items here"), mtm);
        placeholder.setAlignment(NSTextAlignment::Center);
        placeholder.setTextColor(Some(&NSColor::secondaryLabelColor()));

        let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(112.0, 200.0));
        let this = Self::alloc(mtm).set_ivars(ContentIvars {
            model,
            list,
            placeholder,
        });
        // SAFETY: `initWithFrame:` is NSView's designated initializer.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: rect] };
        this.setup();
        this
    }

    fn setup(&self) {
        let mtm = self.mtm();
        self.setMaterial(NSVisualEffectMaterial::Popover);
        self.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        self.setState(NSVisualEffectState::Active);
        self.setWantsLayer(true);
        if let Some(layer) = self.layer() {
            layer.setCornerRadius(CORNER_RADIUS);
            layer.setMasksToBounds(true);
        }
        self.registerForDraggedTypes(&pasteboard::accepted_types());

        // Scrolling column of tiles. The document view is flipped so tiles
        // stack from the top.
        let ivars = self.ivars();
        let document = FlippedView::new(mtm);
        document.addSubview(&ivars.list);
        let scroll = NSScrollView::new(mtm);
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDocumentView(Some(&document));
        self.addSubview(&scroll);
        self.addSubview(&ivars.placeholder);

        let clip = scroll.contentView();
        let views: [&NSView; 4] = [&scroll, &document, &ivars.list, &ivars.placeholder];
        for view in views {
            view.setTranslatesAutoresizingMaskIntoConstraints(false);
        }
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            scroll
                .topAnchor()
                .constraintEqualToAnchor(&self.topAnchor()),
            scroll
                .bottomAnchor()
                .constraintEqualToAnchor(&self.bottomAnchor()),
            scroll
                .leadingAnchor()
                .constraintEqualToAnchor(&self.leadingAnchor()),
            scroll
                .trailingAnchor()
                .constraintEqualToAnchor(&self.trailingAnchor()),
            document
                .topAnchor()
                .constraintEqualToAnchor(&clip.topAnchor()),
            document
                .leadingAnchor()
                .constraintEqualToAnchor(&clip.leadingAnchor()),
            document
                .trailingAnchor()
                .constraintEqualToAnchor(&clip.trailingAnchor()),
            ivars
                .list
                .topAnchor()
                .constraintEqualToAnchor(&document.topAnchor()),
            ivars
                .list
                .bottomAnchor()
                .constraintEqualToAnchor(&document.bottomAnchor()),
            ivars
                .list
                .leadingAnchor()
                .constraintEqualToAnchor(&document.leadingAnchor()),
            ivars
                .list
                .trailingAnchor()
                .constraintEqualToAnchor(&document.trailingAnchor()),
            ivars
                .placeholder
                .centerXAnchor()
                .constraintEqualToAnchor(&self.centerXAnchor()),
            ivars
                .placeholder
                .centerYAnchor()
                .constraintEqualToAnchor(&self.centerYAnchor()),
            ivars
                .placeholder
                .widthAnchor()
                .constraintEqualToAnchor_constant(&self.widthAnchor(), -16.0),
        ]));
    }

    /// Rebuilds the tiles from the store.
    pub fn reload(&self, entries: &[Entry]) {
        let ivars = self.ivars();
        for view in ivars.list.arrangedSubviews().iter() {
            ivars.list.removeArrangedSubview(&view);
            view.removeFromSuperview();
        }
        for entry in entries {
            let tile = TileView::new(self.mtm(), entry, ivars.model.clone());
            ivars.list.addArrangedSubview(&tile);
        }
        ivars.placeholder.setHidden(!entries.is_empty());
    }

    fn set_highlighted(&self, highlighted: bool) {
        let Some(layer) = self.layer() else { return };
        if highlighted {
            layer.setBorderWidth(2.0);
            layer.setBorderColor(Some(&NSColor::controlAccentColor().CGColor()));
        } else {
            layer.setBorderWidth(0.0);
        }
    }
}

define_class!(
    // SAFETY: NSView has no special subclassing requirements.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "YoinksFlippedView"]
    struct FlippedView;

    impl FlippedView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl FlippedView {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // SAFETY: `FlippedView` has no ivars; NSView's `init` is fine.
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

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
    struct TileView;

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
            Some(self.context_menu())
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
            self.with_model(|model, id| model.remove(id));
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
    fn new(mtm: MainThreadMarker, entry: &Entry, model: Weak<ShelfModel>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TileIvars {
            entry_id: entry.id,
            is_stack: entry.is_stack(),
            model,
            icon: OnceCell::new(),
            dragging: Cell::new(false),
        });
        // SAFETY: NSView's `init` calls the designated `initWithFrame:`.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };

        let image_view = NSImageView::imageViewWithImage(&icon(entry), mtm);
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
                .constraintEqualToAnchor(&this.topAnchor()),
            column
                .bottomAnchor()
                .constraintEqualToAnchor(&this.bottomAnchor()),
            column
                .centerXAnchor()
                .constraintEqualToAnchor(&this.centerXAnchor()),
        ]));

        let tooltip: Vec<&str> = entry.items.iter().map(|item| item.title.as_str()).collect();
        this.setToolTip(Some(&NSString::from_str(&tooltip.join("\n"))));
        this
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
        menu.addItem(&objc2_app_kit::NSMenuItem::separatorItem(mtm));
        menu.addItem(&menu_item(mtm, "Remove", sel!(removeItem:), "", target));
        menu
    }
}

/// Stacks get Finder's multi-document icon; images show themselves; files
/// show their Finder icon. Real thumbnails arrive in Phase 6.
fn icon(entry: &Entry) -> Retained<NSImage> {
    let workspace = NSWorkspace::sharedWorkspace();
    if entry.has_pending()
        && let Some(symbol) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            ns_string!("arrow.down.circle"),
            Some(ns_string!("Receiving")),
        )
    {
        return symbol;
    }
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
    let path = NSString::from_str(&item.path.to_string_lossy());
    if item.kind == ItemKind::Image
        && let Some(image) = NSImage::initWithContentsOfFile(NSImage::alloc(), &path)
    {
        return image;
    }
    workspace.iconForFile(&path)
}
