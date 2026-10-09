//! The shelf's content: a drop target holding a scrolling column of tiles,
//! with keyboard selection and Quick Look.

use std::cell::{Cell, RefCell};
use std::rc::Weak;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBeep, NSColor, NSDragOperation, NSDraggingInfo, NSEvent, NSEventType, NSLayoutAttribute,
    NSLayoutConstraint, NSMenu, NSMenuItem, NSMenuItemImportFromDeviceIdentifier, NSPasteboard,
    NSPasteboardType, NSResponder, NSScrollView, NSStackView, NSTextAlignment, NSTextField,
    NSUserInterfaceItemIdentification, NSUserInterfaceLayoutOrientation, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindowDelegate,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSEdgeInsets, NSInteger, NSObject, NSObjectProtocol, NSPoint,
    NSRect, NSSize, ns_string,
};
use objc2_quick_look_ui::{
    QLPreviewItem, QLPreviewPanel, QLPreviewPanelDataSource, QLPreviewPanelDelegate,
};
use uuid::Uuid;

use super::item::Entry;
use super::model::ShelfModel;
use super::pasteboard;
use super::tile::TileView;
use crate::ui::menu_item;

const CORNER_RADIUS: f64 = 14.0;

/// Virtual key codes handled by the shelf.
mod key {
    pub const RETURN: u16 = 36;
    pub const SPACE: u16 = 49;
    pub const DELETE: u16 = 51;
    pub const ESCAPE: u16 = 53;
    pub const ENTER: u16 = 76;
    pub const FORWARD_DELETE: u16 = 117;
    pub const DOWN: u16 = 125;
    pub const UP: u16 = 126;
}

pub struct ContentIvars {
    model: Weak<ShelfModel>,
    list: Retained<NSStackView>,
    placeholder: Retained<NSTextField>,
    /// Entry ids in display order, for keyboard navigation.
    order: RefCell<Vec<Uuid>>,
    selected: Cell<Option<Uuid>>,
    /// True while this view controls the Quick Look panel.
    quick_look: Cell<bool>,
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

    // Keyboard: Space toggles Quick Look, arrows move the selection, Return
    // opens, Delete removes, Escape hides the shelf.
    impl ShelfContentView {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if !self.handle_key(event) {
                // SAFETY: Calling the superclass implementation of the same method.
                let _: () = unsafe { msg_send![super(self), keyDown: event] };
            }
        }

        // Clicking the background clears the selection.
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            self.select(None);
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        // Right-clicking the background: add the clipboard, or import a
        // photo or scan from a nearby iPhone or iPad (Continuity Camera).
        #[unsafe(method_id(menuForEvent:))]
        fn menu_for_event(&self, _event: &NSEvent) -> Option<Retained<NSMenu>> {
            // Continuity Camera looks for a receiver starting at the key
            // window's first responder, so make that us.
            if let Some(window) = self.window() {
                window.makeKeyWindow();
                window.makeFirstResponder(Some(self));
            }
            Some(self.background_menu())
        }

        // Continuity Camera asks the responder chain who can receive images.
        #[unsafe(method_id(validRequestorForSendType:returnType:))]
        fn valid_requestor(
            &self,
            send_type: Option<&NSPasteboardType>,
            return_type: Option<&NSPasteboardType>,
        ) -> Option<Retained<AnyObject>> {
            if send_type.is_none() && return_type.is_some() {
                // SAFETY: Every Objective-C object is an `AnyObject`.
                Some(unsafe { Retained::cast_unchecked(self.retain()) })
            } else {
                // SAFETY: Calling the superclass implementation of the same method.
                unsafe {
                    msg_send![super(self), validRequestorForSendType: send_type, returnType: return_type]
                }
            }
        }

        // The photo or scan arrives here.
        #[unsafe(method(readSelectionFromPasteboard:))]
        fn read_selection(&self, pasteboard: &NSPasteboard) -> bool {
            match self.ivars().model.upgrade() {
                Some(model) => model.accept(pasteboard::read(pasteboard)),
                None => false,
            }
        }
    }

    // QLPreviewPanelController (an informal protocol): Quick Look finds us
    // in the key window's responder chain.
    impl ShelfContentView {
        #[unsafe(method(acceptsPreviewPanelControl:))]
        fn accepts_preview_panel_control(&self, _panel: Option<&QLPreviewPanel>) -> bool {
            true
        }

        #[unsafe(method(beginPreviewPanelControl:))]
        fn begin_preview_panel_control(&self, panel: Option<&QLPreviewPanel>) {
            let Some(panel) = panel else { return };
            self.ivars().quick_look.set(true);
            // SAFETY: We implement the data source and delegate protocols.
            unsafe {
                panel.setDataSource(Some(ProtocolObject::from_ref(self)));
                panel.setDelegate(Some(self.as_ref()));
            }
        }

        #[unsafe(method(endPreviewPanelControl:))]
        fn end_preview_panel_control(&self, panel: Option<&QLPreviewPanel>) {
            self.ivars().quick_look.set(false);
            if let Some(panel) = panel {
                // SAFETY: Clearing the data source and delegate is allowed.
                unsafe {
                    panel.setDataSource(None);
                    panel.setDelegate(None);
                }
            }
        }
    }

    // SAFETY: `NSObjectProtocol` has no safety requirements.
    unsafe impl NSObjectProtocol for ShelfContentView {}

    // SAFETY: Required by QLPreviewPanelDelegate; no methods are implemented.
    unsafe impl NSWindowDelegate for ShelfContentView {}

    // SAFETY: The method signatures match the protocol.
    unsafe impl QLPreviewPanelDataSource for ShelfContentView {
        #[unsafe(method(numberOfPreviewItemsInPreviewPanel:))]
        fn number_of_preview_items(&self, _panel: Option<&QLPreviewPanel>) -> NSInteger {
            self.preview_urls().len() as NSInteger
        }

        #[unsafe(method_id(previewPanel:previewItemAtIndex:))]
        fn preview_item(
            &self,
            _panel: Option<&QLPreviewPanel>,
            index: NSInteger,
        ) -> Option<Retained<ProtocolObject<dyn QLPreviewItem>>> {
            usize::try_from(index)
                .ok()
                .and_then(|index| self.preview_urls().into_iter().nth(index))
                .map(ProtocolObject::from_retained)
        }
    }

    // SAFETY: The method signatures match the protocol.
    unsafe impl QLPreviewPanelDelegate for ShelfContentView {
        // Keys pressed while Quick Look is in front: Space closes it, arrows
        // move through the shelf like in Finder.
        #[unsafe(method(previewPanel:handleEvent:))]
        fn preview_panel_handle_event(
            &self,
            panel: Option<&QLPreviewPanel>,
            event: Option<&NSEvent>,
        ) -> bool {
            let key_code = event
                .filter(|event| event.r#type() == NSEventType::KeyDown)
                .map(|event| event.keyCode());
            match key_code {
                Some(key::SPACE) => {
                    if let Some(panel) = panel {
                        panel.orderOut(None);
                    }
                    true
                }
                Some(key::UP) => {
                    self.move_selection(-1);
                    true
                }
                Some(key::DOWN) => {
                    self.move_selection(1);
                    true
                }
                _ => false,
            }
        }

        // Zoom the preview out of (and back into) the tile's icon.
        #[unsafe(method(previewPanel:sourceFrameOnScreenForPreviewItem:))]
        fn preview_source_frame(
            &self,
            _panel: Option<&QLPreviewPanel>,
            _item: Option<&ProtocolObject<dyn QLPreviewItem>>,
        ) -> NSRect {
            self.selected_tile()
                .and_then(|tile| tile.icon_frame_on_screen())
                .unwrap_or(NSRect::ZERO)
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
            order: RefCell::default(),
            selected: Cell::new(None),
            quick_look: Cell::new(false),
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

    /// Rebuilds the tiles from the store, keeping the selection if its
    /// entry still exists.
    pub fn reload(&self, model: &ShelfModel, entries: &[Entry]) {
        let ivars = self.ivars();
        for view in ivars.list.arrangedSubviews().iter() {
            ivars.list.removeArrangedSubview(&view);
            view.removeFromSuperview();
        }
        let mtm = self.mtm();
        for entry in entries {
            let thumbnail = entry
                .items
                .first()
                .and_then(|item| model.thumbnail(mtm, item));
            let tooltip = model.tooltip(entry);
            let tile = TileView::new(
                mtm,
                entry,
                thumbnail.as_deref(),
                &tooltip,
                ivars.model.clone(),
            );
            ivars.list.addArrangedSubview(&tile);
        }
        ivars.placeholder.setHidden(!entries.is_empty());
        *ivars.order.borrow_mut() = entries.iter().map(|entry| entry.id).collect();
        let selected = ivars
            .selected
            .get()
            .filter(|id| ivars.order.borrow().contains(id));
        self.apply_selection(selected);
    }

    fn background_menu(&self) -> Retained<NSMenu> {
        let mtm = self.mtm();
        let menu = NSMenu::new(mtm);
        // No target: the action travels the responder chain to the app
        // delegate.
        menu.addItem(&menu_item(
            mtm,
            "Add Clipboard to Shelf",
            sel!(addClipboard:),
            "",
            None,
        ));
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        // AppKit replaces this item with the nearby devices' "Take Photo",
        // "Scan Documents" and "Add Sketch" commands.
        let import = NSMenuItem::new(mtm);
        import.setTitle(ns_string!("Import from iPhone or iPad"));
        // SAFETY: Reading an immutable framework constant.
        import.setIdentifier(Some(unsafe { NSMenuItemImportFromDeviceIdentifier }));
        menu.addItem(&import);
        menu
    }

    /// Selects an entry (or nothing), focuses the shelf for keyboard use and
    /// keeps Quick Look in sync.
    pub fn select(&self, id: Option<Uuid>) {
        if let Some(window) = self.window() {
            window.makeFirstResponder(Some(self));
        }
        self.apply_selection(id);
    }

    fn apply_selection(&self, id: Option<Uuid>) {
        let changed = self.ivars().selected.replace(id) != id;
        for tile in self.tiles() {
            let selected = Some(tile.entry_id()) == id;
            tile.set_selected(selected);
            if selected {
                tile.scrollRectToVisible(tile.bounds());
            }
        }
        if changed && self.ivars().quick_look.get() {
            self.with_preview_panel(|panel| {
                // SAFETY: We are the panel's data source.
                unsafe { panel.reloadData() }
            });
        }
    }

    /// Opens Quick Look for the selected entry, or closes it if open.
    pub fn toggle_quick_look(&self) {
        let mtm = self.mtm();
        // SAFETY: Called on the main thread.
        let exists = unsafe { QLPreviewPanel::sharedPreviewPanelExists(mtm) };
        if exists && self.with_preview_panel(|panel| panel.isVisible()) == Some(true) {
            self.with_preview_panel(|panel| panel.orderOut(None));
            return;
        }
        if self.preview_urls().is_empty() {
            NSBeep();
            return;
        }
        // Quick Look looks for a controller in the key window's responder
        // chain, so focus ourselves first. Our app is otherwise never active;
        // activate it so the preview comes to the front.
        if let Some(window) = self.window() {
            window.makeKeyWindow();
            window.makeFirstResponder(Some(self));
        }
        crate::ui::activate_app(mtm);
        self.with_preview_panel(|panel| panel.makeKeyAndOrderFront(None));
    }

    /// Removes an entry, moving the selection to its neighbor.
    pub fn remove(&self, id: Uuid) {
        let neighbor = {
            let order = self.ivars().order.borrow();
            let index = order.iter().position(|other| *other == id);
            index.and_then(|i| {
                order
                    .get(i + 1)
                    .or_else(|| i.checked_sub(1).and_then(|j| order.get(j)))
                    .copied()
            })
        };
        let was_selected = self.ivars().selected.get() == Some(id);
        if let Some(model) = self.ivars().model.upgrade() {
            model.remove(id);
        }
        if was_selected {
            self.apply_selection(neighbor);
        }
    }

    fn handle_key(&self, event: &NSEvent) -> bool {
        let selected = self.ivars().selected.get();
        match event.keyCode() {
            key::SPACE => self.toggle_quick_look(),
            key::UP => self.move_selection(-1),
            key::DOWN => self.move_selection(1),
            key::RETURN | key::ENTER => match (selected, self.ivars().model.upgrade()) {
                (Some(id), Some(model)) => model.open(id),
                _ => NSBeep(),
            },
            key::DELETE | key::FORWARD_DELETE => match selected {
                Some(id) => self.remove(id),
                None => NSBeep(),
            },
            key::ESCAPE => {
                if let Some(window) = self.window() {
                    // SAFETY: `cancelOperation:` takes one object argument;
                    // the shelf panel hides itself in response.
                    let _: () = unsafe { msg_send![&*window, cancelOperation: None::<&AnyObject>] };
                }
            }
            _ => return false,
        }
        true
    }

    /// Moves the selection by `delta` tiles; with nothing selected, starts
    /// at the first (down) or last (up) tile.
    fn move_selection(&self, delta: isize) {
        let next = {
            let order = self.ivars().order.borrow();
            if order.is_empty() {
                return;
            }
            let last = order.len() as isize - 1;
            let index = match self
                .ivars()
                .selected
                .get()
                .and_then(|id| order.iter().position(|o| *o == id))
            {
                Some(index) => (index as isize + delta).clamp(0, last),
                None if delta > 0 => 0,
                None => last,
            };
            order[index as usize]
        };
        self.select(Some(next));
    }

    fn tiles(&self) -> Vec<Retained<TileView>> {
        self.ivars()
            .list
            .arrangedSubviews()
            .iter()
            .filter_map(|view| view.downcast::<TileView>().ok())
            .collect()
    }

    fn selected_tile(&self) -> Option<Retained<TileView>> {
        let id = self.ivars().selected.get()?;
        self.tiles().into_iter().find(|tile| tile.entry_id() == id)
    }

    fn preview_urls(&self) -> Vec<Retained<objc2_foundation::NSURL>> {
        match (self.ivars().selected.get(), self.ivars().model.upgrade()) {
            (Some(id), Some(model)) => model.preview_urls(id),
            _ => Vec::new(),
        }
    }

    fn with_preview_panel<R>(&self, f: impl FnOnce(&QLPreviewPanel) -> R) -> Option<R> {
        // SAFETY: Called on the main thread.
        let panel = unsafe { QLPreviewPanel::sharedPreviewPanel(self.mtm()) }?;
        Some(f(&panel))
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
