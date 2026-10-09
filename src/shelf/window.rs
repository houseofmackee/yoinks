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
    NSEvent, NSFloatingWindowLevel, NSLayoutConstraint, NSPanel, NSScreen, NSTextAlignment,
    NSTextField, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSVisualEffectView, NSWindow, NSWindowAnimationBehavior, NSWindowCollectionBehavior,
    NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{MainThreadMarker, NSArray, NSPoint, NSRect, NSSize, ns_string};

use super::geometry::{ShelfEdge, ShelfFrames, screen_containing, shelf_frames};

const CORNER_RADIUS: f64 = 14.0;
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

        panel.setContentView(Some(&content_view(mtm)));
        panel
    }
}

/// Rounded vibrancy background with a placeholder label. Phase 2 replaces the
/// label with the item grid.
fn content_view(mtm: MainThreadMarker) -> Retained<NSVisualEffectView> {
    let bounds = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(112.0, 200.0));
    let effect = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), bounds);
    effect.setMaterial(NSVisualEffectMaterial::Popover);
    effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    effect.setState(NSVisualEffectState::Active);
    effect.setWantsLayer(true);
    if let Some(layer) = effect.layer() {
        layer.setCornerRadius(CORNER_RADIUS);
        layer.setMasksToBounds(true);
    }

    let label = NSTextField::wrappingLabelWithString(ns_string!("Drop items here"), mtm);
    label.setAlignment(NSTextAlignment::Center);
    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
    label.setTranslatesAutoresizingMaskIntoConstraints(false);
    effect.addSubview(&label);
    // Auto Layout keeps the label centered whatever height the shelf takes on
    // the current screen.
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        label
            .centerXAnchor()
            .constraintEqualToAnchor(&effect.centerXAnchor()),
        label
            .centerYAnchor()
            .constraintEqualToAnchor(&effect.centerYAnchor()),
        label
            .widthAnchor()
            .constraintEqualToAnchor_constant(&effect.widthAnchor(), -16.0),
    ]));

    effect
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Visibility {
    Hidden,
    Shown,
    /// The hide animation is running; the panel is still on screen.
    Hiding,
}

/// Owns the shelf panel and drives its show/hide animations.
pub struct Shelf {
    panel: Retained<ShelfPanel>,
    edge: ShelfEdge,
    visibility: Rc<Cell<Visibility>>,
    /// Frames for the screen the shelf was last shown on, so it hides back
    /// toward the same edge even if the cursor moved to another display.
    frames: Cell<Option<ShelfFrames>>,
}

impl Shelf {
    pub fn new(mtm: MainThreadMarker, edge: ShelfEdge) -> Self {
        Self {
            panel: ShelfPanel::new(mtm),
            edge,
            visibility: Rc::new(Cell::new(Visibility::Hidden)),
            frames: Cell::new(None),
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visibility.get() == Visibility::Shown
    }

    pub fn toggle(&self) {
        if self.is_visible() {
            self.hide();
        } else {
            self.show();
        }
    }

    pub fn show(&self) {
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
                let Some(frames) = self.frames_for_active_screen() else {
                    return;
                };
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

    /// Frames for the screen under the mouse, falling back to the main screen.
    fn frames_for_active_screen(&self) -> Option<ShelfFrames> {
        let mtm = self.panel.mtm();
        let screens = NSScreen::screens(mtm);
        let rects: Vec<NSRect> = screens.iter().map(|screen| screen.frame()).collect();
        let screen = match screen_containing(&rects, NSEvent::mouseLocation()) {
            Some(index) => screens.objectAtIndex(index),
            None => NSScreen::mainScreen(mtm)?,
        };
        Some(shelf_frames(screen.visibleFrame(), self.edge))
    }
}

fn reduce_motion() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}
