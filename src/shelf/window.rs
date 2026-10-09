//! The floating shelf panel and the controller that shows and hides it.

use std::cell::Cell;
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSApplication, NSBackingStoreType, NSColor,
    NSEvent, NSFloatingWindowLevel, NSPanel, NSScreen, NSWindow, NSWindowAnimationBehavior,
    NSWindowCollectionBehavior, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};

use super::geometry::{ShelfEdge, ShelfFrames, cursor_frames, screen_containing, shelf_frames};
use super::model::ShelfModel;

const SLIDE_DURATION: f64 = 0.22;
const FADE_DURATION: f64 = 0.15;

define_class!(
    // SAFETY:
    // - NSPanel has no special subclassing requirements.
    // - `ShelfPanel` does not implement `Drop`.
    #[unsafe(super(NSPanel, NSWindow, objc2_app_kit::NSResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "YoinksShelfPanel"]
    pub struct ShelfPanel;

    impl ShelfPanel {
        // Borderless windows refuse key status by default; the shelf needs it
        // so Escape (and later, keyboard navigation) works.
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }

        // Escape: ask whoever handles `hideShelf:` (the app delegate) to hide
        // us with the usual animation.
        #[unsafe(method(cancelOperation:))]
        fn cancel_operation(&self, _sender: Option<&AnyObject>) {
            let app = NSApplication::sharedApplication(self.mtm());
            // SAFETY: `hideShelf:` takes a single object argument.
            unsafe { app.sendAction_to_from(sel!(hideShelf:), None, Some(self)) };
        }
    }
);

impl ShelfPanel {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0));
        let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
        // SAFETY: Inherited designated initializer; `ShelfPanel` has no ivars.
        let panel: Retained<Self> = unsafe {
            msg_send![
                Self::alloc(mtm),
                initWithContentRect: rect,
                styleMask: style,
                backing: NSBackingStoreType::Buffered,
                defer: true,
            ]
        };

        // SAFETY: We hold a strong reference, so AppKit must not release it.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setFloatingPanel(true);
        panel.setLevel(NSFloatingWindowLevel);
        // Visible on every Space and on top of full-screen apps.
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        // Panels hide when their app deactivates by default; ours is an
        // accessory app that is rarely active.
        panel.setHidesOnDeactivate(false);
        panel.setAnimationBehavior(NSWindowAnimationBehavior::None);
        panel.setMovable(false);
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(true);

        panel
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Visibility {
    Hidden,
    Shown,
    /// The hide animation is running; the panel is still on screen.
    Hiding,
}

/// Owns the shelf panel and its contents, and drives the show/hide
/// animations.
pub struct Shelf {
    panel: Retained<ShelfPanel>,
    model: Rc<ShelfModel>,
    edge: ShelfEdge,
    visibility: Rc<Cell<Visibility>>,
    /// Frames for the screen the shelf was last shown on, so it hides back
    /// toward the same edge even if the cursor moved to another display.
    frames: Cell<Option<ShelfFrames>>,
}

impl Shelf {
    pub fn new(mtm: MainThreadMarker, edge: ShelfEdge, model: Rc<ShelfModel>) -> Self {
        let panel = ShelfPanel::new(mtm);
        panel.setContentView(Some(model.view()));
        Self {
            panel,
            model,
            edge,
            visibility: Rc::new(Cell::new(Visibility::Hidden)),
            frames: Cell::new(None),
        }
    }

    pub fn model(&self) -> &Rc<ShelfModel> {
        &self.model
    }

    pub fn edge(&self) -> ShelfEdge {
        self.edge
    }

    pub fn is_visible(&self) -> bool {
        self.visibility.get() == Visibility::Shown
    }

    /// True while the shelf is on screen and the cursor is over it.
    pub fn contains_mouse(&self) -> bool {
        self.visibility.get() != Visibility::Hidden
            && screen_containing(&[self.panel.frame()], NSEvent::mouseLocation()).is_some()
    }

    /// The full frame of the screen under the cursor.
    pub fn screen_under_cursor(&self) -> Option<NSRect> {
        self.active_screen().map(|screen| screen.frame())
    }

    pub fn toggle(&self) {
        if self.is_visible() {
            self.hide();
        } else {
            self.show();
        }
    }

    /// Slides the shelf in at its screen edge.
    pub fn show(&self) {
        self.present(|visible, _| shelf_frames(visible, self.edge));
    }

    /// Fades the shelf in beside the cursor.
    pub fn show_at_cursor(&self) {
        self.present(|visible, cursor| cursor_frames(visible, cursor, self.edge));
    }

    /// Shows the shelf on the screen under the cursor, placed by `layout`
    /// from that screen's visible frame and the cursor position.
    fn present(&self, layout: impl FnOnce(NSRect, NSPoint) -> ShelfFrames) {
        match self.visibility.get() {
            Visibility::Shown => {}
            Visibility::Hiding => {
                // Reverse the running hide animation in place.
                if let Some(frames) = self.frames.get() {
                    self.visibility.set(Visibility::Shown);
                    self.animate(frames.shown, 1.0, None);
                }
            }
            Visibility::Hidden => {
                let Some(screen) = self.active_screen() else {
                    return;
                };
                let frames = layout(screen.visibleFrame(), NSEvent::mouseLocation());
                self.frames.set(Some(frames));
                self.visibility.set(Visibility::Shown);

                let start = if reduce_motion() {
                    frames.shown
                } else {
                    frames.hidden
                };
                self.panel.setFrame_display(start, false);
                self.panel.setAlphaValue(0.0);
                self.panel.orderFrontRegardless();
                self.panel.makeKeyWindow();
                self.animate(frames.shown, 1.0, None);
            }
        }
    }

    pub fn hide(&self) {
        if self.visibility.get() != Visibility::Shown {
            return;
        }
        self.visibility.set(Visibility::Hiding);

        let target = match self.frames.get() {
            Some(frames) if !reduce_motion() => frames.hidden,
            _ => self.panel.frame(),
        };
        let panel = self.panel.clone();
        let visibility = Rc::clone(&self.visibility);
        let completion = RcBlock::new(move || {
            // A `show()` during the animation flips the state back to Shown;
            // only order out if we are still hiding.
            if visibility.get() == Visibility::Hiding {
                panel.orderOut(None);
                visibility.set(Visibility::Hidden);
            }
        });
        self.animate(target, 0.0, Some(completion));
    }

    fn animate(&self, frame: NSRect, alpha: f64, completion: Option<RcBlock<dyn Fn()>>) {
        let window: &NSWindow = &self.panel;
        let animator = window.animator();
        let duration = if reduce_motion() {
            FADE_DURATION
        } else {
            SLIDE_DURATION
        };
        let changes = RcBlock::new(move |context: NonNull<NSAnimationContext>| {
            // SAFETY: AppKit passes a valid context for the duration of the block.
            unsafe { context.as_ref() }.setDuration(duration);
            animator.setFrame_display(frame, true);
            animator.setAlphaValue(alpha);
        });
        NSAnimationContext::runAnimationGroup_completionHandler(&changes, completion.as_deref());
    }

    /// The screen under the mouse, falling back to the main screen.
    fn active_screen(&self) -> Option<Retained<NSScreen>> {
        let mtm = self.panel.mtm();
        let screens = NSScreen::screens(mtm);
        let rects: Vec<NSRect> = screens.iter().map(|screen| screen.frame()).collect();
        match screen_containing(&rects, NSEvent::mouseLocation()) {
            Some(index) => Some(screens.objectAtIndex(index)),
            None => NSScreen::mainScreen(mtm),
        }
    }
}

fn reduce_motion() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}
