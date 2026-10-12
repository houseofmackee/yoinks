//! The shelf's content: a drop target holding a scrolling column of tiles,
//! with keyboard selection and Quick Look.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Weak;

use objc2::ClassType;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBeep, NSColor, NSControl, NSControlTextEditingDelegate, NSDragOperation, NSDraggingInfo,
    NSEvent, NSEventModifierFlags, NSEventType, NSLayoutAttribute, NSLayoutConstraint, NSMenu,
    NSMenuItem, NSMenuItemImportFromDeviceIdentifier, NSPasteboard, NSPasteboardType, NSResponder,
    NSScrollView, NSSearchField, NSSearchFieldDelegate, NSStackView, NSTextAlignment, NSTextField,
    NSTextFieldDelegate, NSTextView, NSUserInterfaceItemIdentification,
    NSUserInterfaceLayoutOrientation, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindowDelegate,
};
use objc2_app_kit::{NSScroller, NSTrackingArea, NSTrackingAreaOptions};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSEdgeInsets, NSInteger, NSObject, NSObjectProtocol, NSPoint,
    NSRange, NSRect, NSSize, NSString, ns_string,
};
use objc2_quick_look_ui::{
    QLPreviewItem, QLPreviewPanel, QLPreviewPanelDataSource, QLPreviewPanelDelegate,
};
use uuid::Uuid;

use super::item::Entry;
use super::menu_button::MenuButton;
use super::model::ShelfModel;
use super::pasteboard;
use super::search::Query;
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

/// What the content view tells the shelf about.
#[derive(Clone, Copy, Debug)]
pub enum ViewEvent {
    /// The pointer left the shelf.
    Exited,
    /// Something was dropped on or dragged out of the shelf.
    Used,
    /// The user dragged the shelf to a new place (bottom-left corner).
    Moved(NSPoint),
    /// The shelf's contents changed (items added, removed or cleared).
    Changed,
}

type EventHandler = Box<dyn Fn(ViewEvent)>;

/// A background press that may become a move of the whole shelf.
#[derive(Clone, Copy)]
struct Press {
    mouse: NSPoint,
    origin: NSPoint,
    moved: bool,
}

/// Pointer travel before a background press counts as moving the shelf.
const MOVE_THRESHOLD: f64 = 3.0;

pub struct ContentIvars {
    model: Weak<ShelfModel>,
    list: Retained<NSStackView>,
    placeholder: Retained<NSTextField>,
    /// Entry ids in display order, for keyboard navigation.
    order: RefCell<Vec<Uuid>>,
    selected: Cell<Option<Uuid>>,
    /// True while this view controls the Quick Look panel.
    quick_look: Cell<bool>,
    menu_button: Retained<MenuButton>,
    on_event: RefCell<Option<EventHandler>>,
    press: Cell<Option<Press>>,
    scroll: OnceCell<Retained<NSScrollView>>,
    /// Shown at the top while searching (⌘F or typing on the shelf).
    search: Retained<NSSearchField>,
    query: RefCell<Option<Query>>,
    /// The shelf's name, shown at the top when there are several shelves.
    title: Retained<NSTextField>,
    /// Where a tile being reordered would go.
    insertion: Retained<NSView>,
    /// The entry just reordered within the shelf, until its tile's drag
    /// session ends.
    reordered: Cell<Option<Uuid>>,
    /// A wash of the shelf's tag color behind everything else.
    tint: Retained<NSView>,
    /// Holds the name and the search field; stack views drop hidden views.
    header: Retained<NSStackView>,
    /// The tiles' top edge: at the shelf's top, or below the header.
    scroll_top: OnceCell<[Retained<NSLayoutConstraint>; 2]>,
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
            if self.own_tile(info).is_some() {
                // One of our tiles: a reorder, shown by the insertion line.
                self.show_insertion(info);
                return NSDragOperation::Move;
            }
            let operation = drop_operation(info);
            self.set_highlighted(operation != NSDragOperation::None);
            operation
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            if self.own_tile(info).is_some() {
                self.show_insertion(info);
                return NSDragOperation::Move;
            }
            drop_operation(info)
        }

        #[unsafe(method(draggingExited:))]
        fn dragging_exited(&self, _info: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            self.set_highlighted(false);
            self.ivars().insertion.setHidden(true);
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag_operation(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            self.set_highlighted(false);
            self.ivars().insertion.setHidden(true);
            if let Some(id) = self.own_tile(info) {
                let before = self.insertion_point(info);
                // The tile's drag session ends after this; it asks
                // `take_reordered` so it doesn't count as a drag out.
                self.ivars().reordered.set(Some(id));
                if let Some(model) = self.ivars().model.upgrade() {
                    model.move_entry(id, before);
                }
                true
            } else {
                let dropped = pasteboard::read(&info.draggingPasteboard());
                let accepted = match self.ivars().model.upgrade() {
                    Some(model) => model.accept(dropped),
                    None => false,
                };
                if accepted {
                    self.notify(ViewEvent::Used);
                }
                accepted
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
            if !self.handle_key(event) && !self.search_key(event) {
                // SAFETY: Calling the superclass implementation of the same method.
                let _: () = unsafe { msg_send![super(self), keyDown: event] };
            }
        }

        // Pressing the background starts a possible move of the shelf; a
        // click without moving clears the selection instead.
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            if let Some(window) = self.window() {
                self.ivars().press.set(Some(Press {
                    mouse: NSEvent::mouseLocation(),
                    origin: window.frame().origin,
                    moved: false,
                }));
            }
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, _event: &NSEvent) {
            let (Some(mut press), Some(window)) = (self.ivars().press.get(), self.window()) else {
                return;
            };
            let mouse = NSEvent::mouseLocation();
            let (dx, dy) = (mouse.x - press.mouse.x, mouse.y - press.mouse.y);
            if !press.moved && dx.abs().max(dy.abs()) < MOVE_THRESHOLD {
                return;
            }
            press.moved = true;
            self.ivars().press.set(Some(press));
            window.setFrameOrigin(NSPoint::new(press.origin.x + dx, press.origin.y + dy));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            match (self.ivars().press.take(), self.window()) {
                (Some(press), Some(window)) if press.moved => {
                    self.notify(ViewEvent::Moved(window.frame().origin));
                }
                _ => self.select(None),
            }
        }

        // Empty areas (between and below the tiles) belong to the shelf
        // itself, so pressing there moves the shelf. Tiles, the gear button
        // and the scroll bar keep their own clicks.
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            // SAFETY: Calling the superclass implementation of the same method.
            let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
            hit.map(|hit| {
                let keeps_own_clicks = hit.isKindOfClass(TileView::class())
                    || hit.isKindOfClass(MenuButton::class())
                    || hit.isKindOfClass(NSScroller::class());
                if keeps_own_clicks {
                    hit
                } else {
                    Retained::into_super(Retained::into_super(self.retain()))
                }
            })
        }

        // Scrolling goes to the list even though the shelf owns its clicks.
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            if let Some(scroll) = self.ivars().scroll.get() {
                scroll.scrollWheel(event);
            }
        }

        // ⌘C (Edit > Copy) on the shelf copies the selected entry.
        #[unsafe(method(copy:))]
        fn copy(&self, _sender: Option<&AnyObject>) {
            let copied = self
                .ivars()
                .selected
                .get()
                .zip(self.ivars().model.upgrade())
                .is_some_and(|(id, model)| model.copy(id));
            if !copied {
                NSBeep();
            }
        }

        // The search field's text changed (or its clear button was clicked).
        #[unsafe(method(searchChanged:))]
        fn search_changed(&self, _sender: Option<&AnyObject>) {
            let text = self.ivars().search.stringValue().to_string();
            let query = Query::parse(&text);
            if *self.ivars().query.borrow() != query {
                *self.ivars().query.borrow_mut() = query;
                self.refilter();
            }
        }

        #[unsafe(method(beginSearch:))]
        fn begin_search_action(&self, _sender: Option<&AnyObject>) {
            self.begin_search("");
        }

        // ⌘V (Edit > Paste) adds whatever is on the clipboard.
        #[unsafe(method(paste:))]
        fn paste(&self, _sender: Option<&AnyObject>) {
            let added = self
                .ivars()
                .model
                .upgrade()
                .is_some_and(|model| model.accept_clipboard());
            if !added {
                NSBeep();
            }
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.notify(ViewEvent::Exited);
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

    // SAFETY: The method signature matches the protocol.
    unsafe impl NSControlTextEditingDelegate for ShelfContentView {
        // In the search field: Escape ends the search; Down and Return move
        // to the first match.
        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn search_command(&self, _control: &NSControl, _text_view: &NSTextView, command: Sel) -> bool {
            self.handle_search_command(command)
        }
    }

    // SAFETY: No required methods.
    unsafe impl NSTextFieldDelegate for ShelfContentView {}

    // SAFETY: No required methods.
    unsafe impl NSSearchFieldDelegate for ShelfContentView {}
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

        let search = NSSearchField::new(mtm);
        search.setPlaceholderString(Some(ns_string!("Search")));
        search.setSendsSearchStringImmediately(true);
        search.setHidden(true);
        let title = NSTextField::labelWithString(ns_string!(""), mtm);
        title.setFont(Some(&objc2_app_kit::NSFont::boldSystemFontOfSize(11.0)));
        title.setTextColor(Some(&NSColor::secondaryLabelColor()));
        title.setAlignment(NSTextAlignment::Center);
        title.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
        title.setHidden(true);
        let header = NSStackView::stackViewWithViews(
            &NSArray::from_slice(&[&*title as &NSView, &*search]),
            mtm,
        );
        header.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        header.setAlignment(NSLayoutAttribute::CenterX);
        header.setSpacing(4.0);
        header.setHidden(true);

        let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(112.0, 200.0));
        let this = Self::alloc(mtm).set_ivars(ContentIvars {
            model,
            list,
            placeholder,
            order: RefCell::default(),
            selected: Cell::new(None),
            quick_look: Cell::new(false),
            menu_button: MenuButton::new(mtm),
            on_event: RefCell::new(None),
            press: Cell::new(None),
            scroll: OnceCell::new(),
            search,
            query: RefCell::new(None),
            title,
            tint: NSView::new(mtm),
            insertion: NSView::new(mtm),
            reordered: Cell::new(None),
            header,
            scroll_top: OnceCell::new(),
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
        // Report the pointer leaving, for "hide after use". `InVisibleRect`
        // keeps the area matched to the view as the shelf resizes.
        // SAFETY: We own the tracking area's owner (ourselves) for its life.
        let tracking = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                NSTrackingAreaOptions::MouseEnteredAndExited
                    | NSTrackingAreaOptions::ActiveAlways
                    | NSTrackingAreaOptions::InVisibleRect,
                Some(self.as_ref()),
                None,
            )
        };
        self.addTrackingArea(&tracking);

        // Scrolling column of tiles. The document view is flipped so tiles
        // stack from the top.
        let ivars = self.ivars();
        let document = FlippedView::new(mtm);
        document.addSubview(&ivars.list);
        let scroll = NSScrollView::new(mtm);
        let _ = ivars.scroll.set(scroll.clone());
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDocumentView(Some(&document));
        ivars.tint.setWantsLayer(true);
        ivars.tint.setHidden(true);
        self.addSubview(&ivars.tint);
        self.addSubview(&scroll);
        self.addSubview(&ivars.placeholder);
        self.addSubview(&ivars.menu_button);
        self.addSubview(&ivars.header);
        ivars.insertion.setWantsLayer(true);
        ivars.insertion.setHidden(true);
        if let Some(layer) = ivars.insertion.layer() {
            layer.setBackgroundColor(Some(&NSColor::controlAccentColor().CGColor()));
            layer.setCornerRadius(1.5);
        }
        self.addSubview(&ivars.insertion);
        // SAFETY: Actions take one sender argument; we outlive the field.
        unsafe {
            ivars.search.setTarget(Some(self.as_ref()));
            ivars.search.setAction(Some(sel!(searchChanged:)));
            ivars
                .search
                .setDelegate(Some(ProtocolObject::from_ref(self)));
        }

        let clip = scroll.contentView();
        let views: [&NSView; 7] = [
            &scroll,
            &document,
            &ivars.list,
            &ivars.placeholder,
            &ivars.menu_button,
            &ivars.header,
            &ivars.tint,
        ];
        for view in views {
            view.setTranslatesAutoresizingMaskIntoConstraints(false);
        }
        let top = scroll
            .topAnchor()
            .constraintEqualToAnchor(&self.topAnchor());
        let below_header = scroll
            .topAnchor()
            .constraintEqualToAnchor_constant(&ivars.header.bottomAnchor(), 4.0);
        let _ = ivars.scroll_top.set([top.clone(), below_header]);
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            top,
            ivars
                .tint
                .topAnchor()
                .constraintEqualToAnchor(&self.topAnchor()),
            ivars
                .tint
                .bottomAnchor()
                .constraintEqualToAnchor(&self.bottomAnchor()),
            ivars
                .tint
                .leadingAnchor()
                .constraintEqualToAnchor(&self.leadingAnchor()),
            ivars
                .tint
                .trailingAnchor()
                .constraintEqualToAnchor(&self.trailingAnchor()),
            ivars
                .header
                .topAnchor()
                .constraintEqualToAnchor_constant(&self.topAnchor(), 8.0),
            ivars
                .header
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&self.leadingAnchor(), 8.0),
            ivars
                .header
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&self.trailingAnchor(), -8.0),
            ivars
                .search
                .widthAnchor()
                .constraintEqualToAnchor(&ivars.header.widthAnchor()),
            ivars
                .title
                .widthAnchor()
                .constraintEqualToAnchor(&ivars.header.widthAnchor()),
            scroll
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&ivars.menu_button.topAnchor(), -2.0),
            ivars
                .menu_button
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&self.bottomAnchor(), -6.0),
            ivars
                .menu_button
                .centerXAnchor()
                .constraintEqualToAnchor(&self.centerXAnchor()),
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
        let query = ivars.query.borrow().clone();
        let matching: Vec<&Entry> = match &query {
            Some(query) => entries
                .iter()
                .filter(|entry| query.matches(entry, &|item| model.search_text(item)))
                .collect(),
            None => entries.iter().collect(),
        };
        let entries = matching.as_slice();
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
        ivars.placeholder.setStringValue(if query.is_some() {
            ns_string!("No matches")
        } else {
            ns_string!("Drop items here")
        });
        ivars.placeholder.setHidden(!entries.is_empty());
        // The shelf's name (with its color tag), once there is more than one
        // shelf or it has a color.
        let current = model.current_shelf();
        ivars
            .title
            .setHidden(model.shelves().len() < 2 && current.color.is_none());
        ivars.title.setAttributedStringValue(&shelf_title(&current));
        // The tag color as a light wash, if the shelf has one and tinting is
        // on in Preferences.
        let tint = model.tint();
        ivars.tint.setHidden(tint.is_none());
        if let (Some(color), Some(layer)) = (tint, ivars.tint.layer()) {
            let wash = crate::ui::tag_color(color).colorWithAlphaComponent(0.22);
            layer.setBackgroundColor(Some(&wash.CGColor()));
        }
        self.update_header();
        self.notify(ViewEvent::Changed);
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
            "v",
            None,
        ));
        menu.addItem(&menu_item(
            mtm,
            "Search Shelf",
            sel!(beginSearch:),
            "f",
            Some(self.as_ref()),
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

    /// The entry whose tile from this shelf is being dragged, if that is
    /// what the drag is.
    fn own_tile(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> Option<Uuid> {
        let tile = info.draggingSource()?.downcast::<TileView>().ok()?;
        let id = tile.entry_id();
        self.ivars().order.borrow().contains(&id).then_some(id)
    }

    /// The entry a dropped tile goes in front of (`None`: at the end), from
    /// where the pointer is relative to the tiles' middles.
    fn insertion_point(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> Option<Uuid> {
        let y = info.draggingLocation().y;
        self.tiles()
            .into_iter()
            .find(|tile| {
                let frame = tile.convertRect_toView(tile.bounds(), None);
                y > frame.origin.y + frame.size.height / 2.0
            })
            .map(|tile| tile.entry_id())
    }

    /// Puts the insertion line where a dragged tile would land.
    fn show_insertion(&self, info: &ProtocolObject<dyn NSDraggingInfo>) {
        let before = self.insertion_point(info);
        let tiles = self.tiles();
        // In window coordinates: the top of the tile it goes before, or the
        // bottom of the last tile.
        let y = match before.and_then(|id| tiles.iter().find(|t| t.entry_id() == id)) {
            Some(tile) => {
                let frame = tile.convertRect_toView(tile.bounds(), None);
                frame.origin.y + frame.size.height + 2.0
            }
            None => match tiles.last() {
                Some(tile) => tile.convertRect_toView(tile.bounds(), None).origin.y - 2.0,
                None => return,
            },
        };
        let local = self.convertPoint_fromView(NSPoint::new(0.0, y), None);
        let insertion = &self.ivars().insertion;
        insertion.setFrame(NSRect::new(
            NSPoint::new(10.0, local.y - 1.5),
            NSSize::new(self.bounds().size.width - 20.0, 3.0),
        ));
        insertion.setHidden(false);
    }

    /// True (once) if `id` was just reordered within the shelf, so its
    /// tile's drag doesn't count as a drag out.
    pub fn take_reordered(&self, id: Uuid) -> bool {
        if self.ivars().reordered.get() == Some(id) {
            self.ivars().reordered.set(None);
            return true;
        }
        false
    }

    /// Shows the search field with `text` in it and puts the cursor there.
    pub fn begin_search(&self, text: &str) {
        let ivars = self.ivars();
        ivars.search.setHidden(false);
        self.update_header();
        if !text.is_empty() {
            ivars.search.setStringValue(&NSString::from_str(text));
        }
        if let Some(window) = self.window() {
            window.makeKeyWindow();
            window.makeFirstResponder(Some(&ivars.search));
        }
        if let Some(editor) = ivars.search.currentEditor() {
            let end = ivars.search.stringValue().length();
            editor.setSelectedRange(NSRange::new(end, 0));
        }
        // SAFETY: Our own action method; it takes one sender argument.
        let _: () = unsafe { msg_send![self, searchChanged: None::<&AnyObject>] };
    }

    /// Clears and hides the search field, showing every entry again.
    pub fn end_search(&self) {
        let ivars = self.ivars();
        if ivars.search.isHidden() {
            return;
        }
        let focused = self
            .window()
            .and_then(|window| window.firstResponder())
            .is_some_and(|responder| {
                ivars.search.currentEditor().is_some_and(|editor| {
                    std::ptr::eq(&*responder, editor.as_ref() as &NSResponder)
                })
            });
        ivars.search.setStringValue(ns_string!(""));
        ivars.search.setHidden(true);
        self.update_header();
        if ivars.query.borrow_mut().take().is_some() {
            self.refilter();
        }
        if focused && let Some(window) = self.window() {
            window.makeFirstResponder(Some(self));
        }
    }

    /// Shows the header (name, search field) if either is showing, and
    /// moves the tiles below it.
    fn update_header(&self) {
        let ivars = self.ivars();
        let visible = !ivars.title.isHidden() || !ivars.search.isHidden();
        ivars.header.setHidden(!visible);
        if let Some([top, below_header]) = ivars.scroll_top.get() {
            if visible {
                top.setActive(false);
                below_header.setActive(true);
            } else {
                below_header.setActive(false);
                top.setActive(true);
            }
        }
    }

    fn refilter(&self) {
        if let Some(model) = self.ivars().model.upgrade() {
            self.reload(&model, &model.entries());
        }
    }

    /// ⌘F, or a letter, number or symbol typed on the shelf, starts a search.
    fn search_key(&self, event: &NSEvent) -> bool {
        let flags = event.modifierFlags();
        let chars = event
            .charactersIgnoringModifiers()
            .map(|c| c.to_string())
            .unwrap_or_default();
        if flags.contains(NSEventModifierFlags::Command) {
            if chars.eq_ignore_ascii_case("f") {
                self.begin_search("");
                return true;
            }
            // ⌘1 to ⌘9 show the first nine shelves.
            if let Ok(number @ 1..=9) = chars.parse::<usize>()
                && let Some(model) = self.ivars().model.upgrade()
            {
                match model.shelves().get(number - 1) {
                    Some(shelf) => {
                        model.switch_shelf(&shelf.id);
                    }
                    None => NSBeep(),
                }
                return true;
            }
            return false;
        }
        if flags.intersects(NSEventModifierFlags::Control | NSEventModifierFlags::Option) {
            return false;
        }
        let typed = event
            .characters()
            .map(|c| c.to_string())
            .unwrap_or_default();
        let printable = !typed.is_empty()
            && typed.chars().all(|c| {
                !c.is_control() && !c.is_whitespace() && !('\u{f700}'..='\u{f8ff}').contains(&c)
            });
        if printable {
            self.begin_search(&typed);
        }
        printable
    }

    fn handle_search_command(&self, command: Sel) -> bool {
        if command == sel!(cancelOperation:) {
            self.end_search();
        } else if command == sel!(moveDown:) || command == sel!(insertNewline:) {
            let first = self.ivars().order.borrow().first().copied();
            match first {
                Some(id) => self.select(Some(id)),
                None => NSBeep(),
            }
        } else {
            return false;
        }
        true
    }

    /// Where `ViewEvent`s go (the shelf).
    pub fn set_on_event(&self, f: impl Fn(ViewEvent) + 'static) {
        *self.ivars().on_event.borrow_mut() = Some(Box::new(f));
    }

    pub fn notify(&self, event: ViewEvent) {
        if let Some(f) = &*self.ivars().on_event.borrow() {
            f(event);
        }
    }

    /// True while Quick Look is showing one of this shelf's items.
    pub fn is_previewing(&self) -> bool {
        self.ivars().quick_look.get()
            && self
                .with_preview_panel(|panel| panel.isVisible())
                .unwrap_or(false)
    }

    /// The gear button at the bottom; `make` builds the menu it opens.
    pub fn set_menu(&self, make: impl Fn() -> Retained<NSMenu> + 'static) {
        self.ivars().menu_button.set_menu(make);
    }

    /// The gear button, e.g. to place windows next to it.
    pub fn menu_button(&self) -> &NSView {
        &self.ivars().menu_button
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

/// "● Name" with the dot in the shelf's tag color, or just the name.
fn shelf_title(shelf: &super::ShelfInfo) -> Retained<objc2_foundation::NSAttributedString> {
    use objc2_foundation::{NSMutableAttributedString, NSRange};
    let text = match shelf.color {
        Some(_) => format!("● {}", shelf.name),
        None => shelf.name.clone(),
    };
    let title = NSMutableAttributedString::from_nsstring(&NSString::from_str(&text));
    let all = NSRange::new(0, title.length());
    let paragraph = objc2_app_kit::NSMutableParagraphStyle::new();
    paragraph.setAlignment(NSTextAlignment::Center);
    paragraph.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
    // SAFETY: Each value has the type its attribute expects, and the font
    // weight is an immutable framework constant.
    unsafe {
        title.addAttribute_value_range(
            objc2_app_kit::NSFontAttributeName,
            &objc2_app_kit::NSFont::systemFontOfSize_weight(
                12.0,
                objc2_app_kit::NSFontWeightSemibold,
            ),
            all,
        );
        // Full label color: the secondary gray washes out on the
        // translucent shelf.
        title.addAttribute_value_range(
            objc2_app_kit::NSForegroundColorAttributeName,
            &NSColor::labelColor(),
            all,
        );
        title.addAttribute_value_range(
            objc2_app_kit::NSParagraphStyleAttributeName,
            &paragraph,
            all,
        );
    }
    if let Some(color) = shelf.color {
        // SAFETY: A color is the right value for the foreground attribute,
        // and the range covers the dot.
        unsafe {
            title.addAttribute_value_range(
                objc2_app_kit::NSForegroundColorAttributeName,
                &crate::ui::tag_color(color),
                NSRange::new(0, 1),
            );
        }
    }
    Retained::into_super(title)
}
