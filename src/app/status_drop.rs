//! Dropping onto the menu bar icon adds to the shelf. The icon's window is
//! registered for the shelf's drag types and this object, as its delegate,
//! receives the drag (windows pass dragging messages to their delegate).

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSDragOperation, NSDraggingInfo, NSPasteboard, NSStatusBarButton, NSStatusItem,
    NSWindowDelegate,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol};

/// Adds what was dropped; false if nothing could be used.
type OnDrop = Box<dyn Fn(&NSPasteboard) -> bool>;

pub struct DropIvars {
    button: Retained<NSStatusBarButton>,
    on_drop: OnDrop,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - `StatusDrop` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "YoinksStatusDrop"]
    #[ivars = DropIvars]
    pub struct StatusDrop;

    // SAFETY: `NSObjectProtocol` has no safety requirements.
    unsafe impl NSObjectProtocol for StatusDrop {}

    // SAFETY: No required methods.
    unsafe impl NSWindowDelegate for StatusDrop {}

    impl StatusDrop {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            let operation = drop_operation(info);
            self.ivars().button.highlight(operation != NSDragOperation::None);
            operation
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            drop_operation(info)
        }

        #[unsafe(method(draggingExited:))]
        fn dragging_exited(&self, _info: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            self.ivars().button.highlight(false);
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag_operation(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            self.ivars().button.highlight(false);
            (self.ivars().on_drop)(&info.draggingPasteboard())
        }
    }
);

impl StatusDrop {
    /// Makes `status_item`'s icon a drop target. Keep the returned object
    /// alive: the window holds its delegate weakly.
    pub fn install(
        mtm: MainThreadMarker,
        status_item: &NSStatusItem,
        on_drop: impl Fn(&NSPasteboard) -> bool + 'static,
    ) -> Option<Retained<Self>> {
        let button = status_item.button(mtm)?;
        let window = button.window()?;
        let this = Self::alloc(mtm).set_ivars(DropIvars {
            button,
            on_drop: Box::new(on_drop),
        });
        // SAFETY: NSObject's `init` has the correct signature.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        window.registerForDraggedTypes(&crate::shelf::accepted_types());
        window.setDelegate(Some(ProtocolObject::from_ref(&*this)));
        Some(this)
    }
}

/// Copy when the source allows it (the icon never moves anything).
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
