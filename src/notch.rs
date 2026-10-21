//! The notch as a drop target, on Macs with one. A borderless panel the size
//! of the pill sits at the notch; normally it only draws the notch itself
//! (black, where the screen shows nothing), so everywhere else its clear
//! pixels let clicks and drags through. While something is dragged onto the
//! notch it draws the whole pill: "Drop to add to Yoinks". The panel never
//! changes size during a drag, which would upset the drag's tracking.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::{Rc, Weak};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplicationDidChangeScreenParametersNotification, NSBackingStoreType, NSBezierPath, NSColor,
    NSDragOperation, NSDraggingDestination, NSDraggingInfo, NSFont, NSPanel, NSPasteboard,
    NSScreen, NSTextAlignment, NSTextField, NSView, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSNotificationCenter, NSObjectProtocol, NSPoint, NSRect,
    NSSize, ns_string,
};

use crate::shelf::geometry::notch_rect;

/// Adds what was dropped; false if nothing could be used.
type OnDrop = Box<dyn Fn(&NSPasteboard) -> bool>;

/// On every desktop (Space), above full-screen apps, out of Cmd-backtick.
const NOTCH_SPACES: NSWindowCollectionBehavior = NSWindowCollectionBehavior::CanJoinAllSpaces
    .union(NSWindowCollectionBehavior::Stationary)
    .union(NSWindowCollectionBehavior::FullScreenAuxiliary)
    .union(NSWindowCollectionBehavior::IgnoresCycle);

/// The pill's size below the notch while a drag hovers.
const PILL_HEIGHT: f64 = 44.0;
const PILL_MIN_WIDTH: f64 = 230.0;

pub struct NotchIvars {
    on_drop: OnDrop,
    /// The notch, in the view's coordinates (at the top, centered).
    notch: Cell<NSRect>,
    expanded: Cell<bool>,
    label: Retained<NSTextField>,
}

define_class!(
    // SAFETY:
    // - NSView has no special subclassing requirements.
    // - `NotchView` does not implement `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "YoinksNotchView"]
    #[ivars = NotchIvars]
    struct NotchView;

    impl NotchView {
        // Black like the notch: invisible over it, and the panel then takes
        // drags there (fully transparent pixels let events through).
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            NSColor::blackColor().setFill();
            if self.ivars().expanded.get() {
                // Rounded at the bottom, flush with the top of the screen.
                let bounds = self.bounds();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(bounds, 14.0, 14.0).fill();
                NSBezierPath::fillRect(NSRect::new(
                    NSPoint::new(0.0, bounds.size.height / 2.0),
                    NSSize::new(bounds.size.width, bounds.size.height / 2.0),
                ));
            } else {
                NSBezierPath::fillRect(self.ivars().notch.get());
            }
        }
    }

    // SAFETY: `NSObjectProtocol` has no safety requirements.
    unsafe impl NSObjectProtocol for NotchView {}

    // SAFETY: The method signatures match the protocol.
    unsafe impl NSDraggingDestination for NotchView {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            let operation = drop_operation(info);
            if operation != NSDragOperation::None {
                self.set_expanded(true);
            }
            operation
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            drop_operation(info)
        }

        #[unsafe(method(draggingExited:))]
        fn dragging_exited(&self, _info: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            self.set_expanded(false);
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag_operation(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            self.set_expanded(false);
            (self.ivars().on_drop)(&info.draggingPasteboard())
        }
    }
);

impl NotchView {
    fn new(mtm: MainThreadMarker, on_drop: OnDrop) -> Retained<Self> {
        let label = NSTextField::labelWithString(ns_string!("Drop to add to Yoinks"), mtm);
        label.setTextColor(Some(&NSColor::whiteColor()));
        label.setFont(Some(&NSFont::systemFontOfSize_weight(
            13.0,
            // SAFETY: Reading an immutable framework constant.
            unsafe { objc2_app_kit::NSFontWeightSemibold },
        )));
        label.setAlignment(NSTextAlignment::Center);
        label.setHidden(true);
        let this = Self::alloc(mtm).set_ivars(NotchIvars {
            on_drop,
            notch: Cell::new(NSRect::ZERO),
            expanded: Cell::new(false),
            label,
        });
        // SAFETY: `init` calls NSView's designated `initWithFrame:`.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.addSubview(&this.ivars().label);
        this.registerForDraggedTypes(&crate::shelf::accepted_types());
        this
    }

    /// Draws the whole pill, or just the notch.
    fn set_expanded(&self, expanded: bool) {
        self.ivars().expanded.set(expanded);
        let label = &self.ivars().label;
        label.setHidden(!expanded);
        label.setFrame(NSRect::new(
            NSPoint::new(0.0, 12.0),
            NSSize::new(self.bounds().size.width, 20.0),
        ));
        self.setNeedsDisplay(true);
    }
}

pub struct NotchTarget {
    panel: Retained<NSPanel>,
    view: Retained<NotchView>,
    enabled: Cell<bool>,
    /// Keeps the screen change observer registered.
    observer: RefCell<Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>>,
    /// Keeps the panel on the desktop the user switches to.
    space_observer: RefCell<Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>>,
}

impl NotchTarget {
    /// Puts the drop target over the notch (if this Mac has one) and keeps
    /// it there as displays change.
    pub fn start(
        mtm: MainThreadMarker,
        on_drop: impl Fn(&NSPasteboard) -> bool + 'static,
    ) -> Rc<Self> {
        let view = NotchView::new(mtm, Box::new(on_drop));
        let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
        // SAFETY: NSPanel's designated initializer.
        let panel: Retained<NSPanel> = unsafe {
            msg_send![
                NSPanel::alloc(mtm),
                initWithContentRect: NSRect::ZERO,
                styleMask: style,
                backing: NSBackingStoreType::Buffered,
                defer: true,
            ]
        };
        // SAFETY: We hold a strong reference to the panel.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(false);
        // Above the menu bar, on every Space and over full-screen apps.
        panel.setLevel(objc2_app_kit::NSMainMenuWindowLevel + 2);
        panel.setCollectionBehavior(NOTCH_SPACES);
        panel.setHidesOnDeactivate(false);
        panel.setContentView(Some(&view));
        let this = Rc::new(Self {
            panel,
            view,
            enabled: Cell::new(true),
            observer: RefCell::new(None),
            space_observer: RefCell::new(None),
        });
        let weak: Weak<Self> = Rc::downgrade(&this);
        let block = RcBlock::new(move |_notification: NonNull<NSNotification>| {
            if let Some(target) = weak.upgrade() {
                target.place();
            }
        });
        // SAFETY: The block only touches main-thread state and is called on
        // the main queue (no queue given: the posting thread, the main one).
        let observer = unsafe {
            NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                Some(NSApplicationDidChangeScreenParametersNotification),
                None,
                None,
                &block,
            )
        };
        *this.observer.borrow_mut() = Some(observer);
        let weak: Weak<Self> = Rc::downgrade(&this);
        let space_observer = crate::ui::on_space_change(move || {
            if let Some(target) = weak.upgrade() {
                crate::ui::ensure_on_active_space(&target.panel, NOTCH_SPACES);
            }
        });
        *this.space_observer.borrow_mut() = Some(space_observer);
        this.place();
        this
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.set(enabled);
        self.place();
    }

    /// Shows the panel over the notch of the first screen that has one, or
    /// hides it.
    fn place(&self) {
        let mtm = MainThreadMarker::from(&*self.view);
        let notch = NSScreen::screens(mtm).iter().find_map(|screen| {
            let left = screen.auxiliaryTopLeftArea();
            let right = screen.auxiliaryTopRightArea();
            notch_rect(screen.frame(), screen.safeAreaInsets().top, left, right)
        });
        match notch.filter(|_| self.enabled.get()) {
            Some(notch) => {
                let frame = pill_frame(notch);
                self.view.ivars().notch.set(NSRect::new(
                    NSPoint::new(notch.origin.x - frame.origin.x, PILL_HEIGHT),
                    notch.size,
                ));
                self.panel.setFrame_display(frame, true);
                self.panel.orderFrontRegardless();
                crate::ui::ensure_on_active_space(&self.panel, NOTCH_SPACES);
            }
            None => self.panel.orderOut(None),
        }
    }
}

impl Drop for NotchTarget {
    fn drop(&mut self) {
        self.panel.orderOut(None);
        if let Some(observer) = self.observer.take() {
            // SAFETY: Removing the observer we added.
            unsafe {
                NSNotificationCenter::defaultCenter().removeObserver(observer.as_ref());
            }
        }
        if let Some(observer) = self.space_observer.take() {
            // SAFETY: Removing the observer we added to the workspace's center.
            unsafe {
                objc2_app_kit::NSWorkspace::sharedWorkspace()
                    .notificationCenter()
                    .removeObserver(observer.as_ref());
            }
        }
    }
}

/// The panel's frame: the notch, widened to the pill and extended below.
fn pill_frame(notch: NSRect) -> NSRect {
    let width = notch.size.width.max(PILL_MIN_WIDTH);
    NSRect::new(
        NSPoint::new(
            notch.origin.x - (width - notch.size.width) / 2.0,
            notch.origin.y - PILL_HEIGHT,
        ),
        NSSize::new(width, notch.size.height + PILL_HEIGHT),
    )
}

/// Copy when the source allows it.
fn drop_operation(info: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
    let mask = info.draggingSourceOperationMask();
    [
        NSDragOperation::Copy,
        NSDragOperation::Generic,
        NSDragOperation::Link,
    ]
    .into_iter()
    .find(|operation| mask.contains(*operation))
    .unwrap_or(NSDragOperation::None)
}
