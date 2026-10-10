//! The floating shelf panel and the controller that shows and hides it.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSApplication, NSBackingStoreType, NSColor,
    NSEvent, NSFloatingWindowLevel, NSPanel, NSScreen, NSWindow, NSWindowAnimationBehavior,
    NSWindowCollectionBehavior, NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSTimer};

use super::geometry::{
    ShelfEdge, ShelfFrames, cursor_frames, custom_frames, screen_containing, shelf_frames,
};
use super::model::ShelfModel;
use super::view::ViewEvent;

/// How long the pointer must stay away before a used shelf hides.
pub const HIDE_AFTER_USE: Duration = Duration::from_millis(1000);
/// The same, when the shelf came up because something was added to it from
/// elsewhere (shortcut, command line, Services) and was never visited.
pub const HIDE_AFTER_ADD: Duration = Duration::from_millis(3000);
const AUTO_HIDE_POLL: f64 = 0.25;

type MovedHandler = Box<dyn Fn(Option<NSPoint>)>;

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
    this: Weak<Shelf>,
    panel: Retained<ShelfPanel>,
    model: Rc<ShelfModel>,
    edge: Cell<ShelfEdge>,
    /// Where the user dragged the shelf (bottom-left corner), if anywhere.
    custom_origin: Cell<Option<NSPoint>>,
    /// Called when the user moves the shelf or puts it back at the edge.
    on_moved: RefCell<Option<MovedHandler>>,
    hide_after_use: Cell<bool>,
    /// Auto-hide watcher: runs while armed; hides the shelf once the
    /// pointer has been away from it for `hide_delay`.
    auto_hide: RefCell<Option<Retained<NSTimer>>>,
    hide_delay: Cell<Duration>,
    outside_since: Cell<Option<Instant>>,
    visibility: Rc<Cell<Visibility>>,
    /// Frames for the screen the shelf was last shown on, so it hides back
    /// toward the same edge even if the cursor moved to another display.
    frames: Cell<Option<ShelfFrames>>,
}

impl Shelf {
    pub fn new(mtm: MainThreadMarker, edge: ShelfEdge, model: Rc<ShelfModel>) -> Rc<Self> {
        let panel = ShelfPanel::new(mtm);
        panel.setContentView(Some(model.view()));
        // Keyboard input (Space for Quick Look, arrows, Delete) goes to the
        // content view.
        panel.makeFirstResponder(Some(model.view()));
        let shelf = Rc::new_cyclic(|this: &Weak<Self>| Self {
            this: this.clone(),
            panel,
            model,
            edge: Cell::new(edge),
            custom_origin: Cell::new(None),
            on_moved: RefCell::new(None),
            hide_after_use: Cell::new(true),
            auto_hide: RefCell::new(None),
            hide_delay: Cell::new(HIDE_AFTER_USE),
            outside_since: Cell::new(None),
            visibility: Rc::new(Cell::new(Visibility::Hidden)),
            frames: Cell::new(None),
        });
        let weak = Rc::downgrade(&shelf);
        shelf.model.view().set_on_event(move |event| {
            if let Some(shelf) = weak.upgrade() {
                shelf.view_event(event);
            }
        });
        shelf
    }

    pub fn set_hide_after_use(&self, enabled: bool) {
        self.hide_after_use.set(enabled);
        if !enabled {
            self.disarm_auto_hide();
        }
    }

    /// Where the shelf appears instead of its edge (`None`: at the edge).
    pub fn set_custom_origin(&self, origin: Option<NSPoint>) {
        self.custom_origin.set(origin);
    }

    pub fn has_custom_position(&self) -> bool {
        self.custom_origin.get().is_some()
    }

    /// Called with the new position whenever the user moves the shelf, or
    /// with `None` when it is put back at the edge.
    pub fn set_on_moved(&self, f: impl Fn(Option<NSPoint>) + 'static) {
        *self.on_moved.borrow_mut() = Some(Box::new(f));
    }

    /// Forgets the dragged-to position and slides the shelf back to its
    /// edge if it is showing.
    pub fn reset_position(&self) {
        self.custom_origin.set(None);
        if let Some(f) = &*self.on_moved.borrow() {
            f(None);
        }
        if self.visibility.get() == Visibility::Shown
            && let Some(frames) = self.edge_or_custom_frames()
        {
            self.frames.set(Some(frames));
            self.animate(frames.shown, 1.0, None);
        }
    }

    /// Hides the shelf once it is empty and the pointer has been away from
    /// it for `after` (if `hide_after_use` is on). Called after the shelf was
    /// used. A shelf with items is left alone.
    pub fn arm_auto_hide(&self, after: Duration) {
        if !self.hide_after_use.get() || self.visibility.get() != Visibility::Shown {
            return;
        }
        self.hide_delay.set(after);
        self.outside_since.set(None);
        if self.auto_hide.borrow().is_some() {
            return;
        }
        let weak = self.this.clone();
        let block = RcBlock::new(move |_timer: NonNull<NSTimer>| {
            if let Some(shelf) = weak.upgrade() {
                shelf.auto_hide_tick();
            }
        });
        // SAFETY: Scheduled on the main run loop (default mode, so it pauses
        // while a menu is open); the block only touches main-thread state.
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_repeats_block(AUTO_HIDE_POLL, true, &block)
        };
        *self.auto_hide.borrow_mut() = Some(timer);
    }

    fn disarm_auto_hide(&self) {
        if let Some(timer) = self.auto_hide.take() {
            timer.invalidate();
        }
        self.outside_since.set(None);
    }

    fn auto_hide_tick(&self) {
        if self.visibility.get() != Visibility::Shown {
            self.disarm_auto_hide();
            return;
        }
        // A shelf with items stays until the user hides it; watching
        // resumes when it becomes empty (see `ViewEvent::Changed`).
        if !self.model.is_empty() {
            self.disarm_auto_hide();
            return;
        }
        // Not while the pointer is on the shelf, a drag or click is in
        // progress, or Quick Look is showing one of its items.
        let busy = self.contains_mouse()
            || NSEvent::pressedMouseButtons() != 0
            || self.model.view().is_previewing();
        if busy {
            self.outside_since.set(None);
            return;
        }
        let now = Instant::now();
        let since = self.outside_since.get().unwrap_or(now);
        self.outside_since.set(Some(since));
        if now.duration_since(since) >= self.hide_delay.get() {
            self.hide();
        }
    }

    fn view_event(&self, event: ViewEvent) {
        match event {
            ViewEvent::Exited | ViewEvent::Used => self.arm_auto_hide(HIDE_AFTER_USE),
            // The last item was dragged out, removed or cleared.
            ViewEvent::Changed if self.model.is_empty() => self.arm_auto_hide(HIDE_AFTER_USE),
            ViewEvent::Changed => {}
            ViewEvent::Moved(origin) => {
                self.custom_origin.set(Some(origin));
                let frame = self.panel.frame();
                self.frames.set(Some(ShelfFrames {
                    shown: frame,
                    hidden: frame,
                }));
                if let Some(f) = &*self.on_moved.borrow() {
                    f(Some(origin));
                }
            }
        }
    }

    pub fn model(&self) -> &Rc<ShelfModel> {
        &self.model
    }

    /// Sets how the shelf's gear button builds its menu.
    pub fn set_menu(&self, make: impl Fn() -> Retained<objc2_app_kit::NSMenu> + 'static) {
        self.model.view().set_menu(make);
    }

    /// The shelf's gear button.
    pub fn menu_button(&self) -> &objc2_app_kit::NSView {
        self.model.view().menu_button()
    }

    pub fn edge(&self) -> ShelfEdge {
        self.edge.get()
    }

    /// Takes effect the next time the shelf is shown.
    pub fn set_edge(&self, edge: ShelfEdge) {
        self.edge.set(edge);
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

    /// Shows the shelf at its edge (or where the user dragged it) and gives
    /// it keyboard focus, for when the user asked for it (shortcut, menu,
    /// relaunch). It then stays until dismissed or used.
    pub fn show(&self) {
        self.disarm_auto_hide();
        self.present(|| self.edge_or_custom_frames(), true);
    }

    /// Shows the shelf at its edge (or where the user dragged it) without
    /// taking keyboard focus, for automatic appearance: typing must keep
    /// going to the app the user is working in.
    pub fn appear_at_edge(&self) {
        self.present(|| self.edge_or_custom_frames(), false);
    }

    /// Fades the shelf in beside the cursor, without taking keyboard focus.
    pub fn appear_at_cursor(&self) {
        self.present(
            || {
                let screen = self.active_screen()?;
                let cursor = NSEvent::mouseLocation();
                Some(cursor_frames(
                    screen.visibleFrame(),
                    cursor,
                    self.edge.get(),
                ))
            },
            false,
        );
    }

    /// Frames at the position the user dragged the shelf to, if that is on
    /// a connected screen; otherwise at the edge of the screen under the
    /// cursor.
    fn edge_or_custom_frames(&self) -> Option<ShelfFrames> {
        let mtm = self.panel.mtm();
        if let Some(origin) = self.custom_origin.get() {
            let screens = NSScreen::screens(mtm);
            let rects: Vec<NSRect> = screens.iter().map(|screen| screen.frame()).collect();
            let probe = NSPoint::new(origin.x + 20.0, origin.y + 20.0);
            if let Some(index) = screen_containing(&rects, probe) {
                let visible = screens.objectAtIndex(index).visibleFrame();
                return Some(custom_frames(visible, origin, self.edge.get()));
            }
        }
        let screen = self.active_screen()?;
        Some(shelf_frames(screen.visibleFrame(), self.edge.get()))
    }

    /// Shows the shelf with frames from `layout`. With `focus`, the panel
    /// also becomes key (it never activates the app).
    fn present(&self, layout: impl FnOnce() -> Option<ShelfFrames>, focus: bool) {
        if focus {
            self.panel.makeKeyWindow();
        }
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
                let Some(frames) = layout() else {
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
                if focus {
                    self.panel.makeKeyWindow();
                }
                self.animate(frames.shown, 1.0, None);
            }
        }
    }

    pub fn hide(&self) {
        self.disarm_auto_hide();
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
