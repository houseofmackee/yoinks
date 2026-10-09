//! The gear button at the bottom of the shelf. It opens the app menu
//! (Preferences, Clipboard History, Quit, ...), so everything stays
//! reachable when the menu bar icon is hidden.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBezelStyle, NSButton, NSControl, NSEvent, NSImage, NSMenu, NSResponder, NSView,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, ns_string};

type MakeMenu = Box<dyn Fn() -> Retained<NSMenu>>;

pub struct MenuButtonIvars {
    /// Builds the menu when clicked, so it reflects the current settings.
    menu: RefCell<Option<MakeMenu>>,
}

define_class!(
    // SAFETY:
    // - NSButton has no special subclassing requirements.
    // - `MenuButton` does not implement `Drop`.
    #[unsafe(super(NSButton, NSControl, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "YoinksShelfMenuButton"]
    #[ivars = MenuButtonIvars]
    pub struct MenuButton;

    impl MenuButton {
        // The shelf panel is usually not key; react to the first click.
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(showMenu:))]
        fn show_menu(&self, _sender: Option<&AnyObject>) {
            let Some(menu) = self.ivars().menu.borrow().as_ref().map(|make| make()) else {
                return;
            };
            // Open the menu upwards from the button. The location is where
            // the menu's top-left corner goes, so lift it by the menu's
            // height. (Anchoring an item instead would pre-select it, and the
            // last item is Quit.) Buttons use flipped coordinates, so
            // "above" is a negative y.
            let lift = menu.size().height + 4.0;
            let above = if self.isFlipped() {
                -lift
            } else {
                self.bounds().size.height + lift
            };
            menu.popUpMenuPositioningItem_atLocation_inView(
                None,
                NSPoint::new(0.0, above),
                Some(self),
            );
        }
    }
);

impl MenuButton {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(MenuButtonIvars {
            menu: RefCell::new(None),
        });
        // SAFETY: NSView's `init` calls the designated `initWithFrame:`.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.setBordered(false);
        this.setBezelStyle(NSBezelStyle::Push);
        this.setTitle(ns_string!(""));
        if let Some(gear) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            ns_string!("gearshape"),
            Some(ns_string!("Yoinks menu")),
        ) {
            this.setImage(Some(&gear));
        }
        this.setToolTip(Some(ns_string!("Preferences and more")));
        // SAFETY: The action takes one sender argument; we are the target.
        unsafe {
            this.setTarget(Some(&this));
            this.setAction(Some(sel!(showMenu:)));
        }
        this
    }

    pub fn set_menu(&self, make: impl Fn() -> Retained<NSMenu> + 'static) {
        *self.ivars().menu.borrow_mut() = Some(Box::new(make));
    }
}
