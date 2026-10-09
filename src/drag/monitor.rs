//! Brings up the shelf when a drag starts anywhere on the system, and puts it
//! away again afterwards if it was not used.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::{Rc, Weak};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSEvent, NSEventMask, NSPasteboard, NSPasteboardNameDrag, NSWorkspace};
use objc2_foundation::NSTimer;

use super::tracker::{DragTracker, Update};
use crate::settings::{AppearMode, Settings};
use crate::shelf::Shelf;
use crate::shelf::geometry::near_edge;

/// How often to check whether the mouse button is still down during a drag.
/// Polling is used because global monitors do not reliably see the mouse-up
/// that ends another app's drag session.
const RELEASE_POLL_INTERVAL: f64 = 0.05;

/// What the monitor decided for the drag in progress.
#[derive(Clone, Copy, Debug)]
struct Session {
    /// Do nothing for this drag: auto-show is off, the source app is
    /// ignored, or the shelf was already open.
    passive: bool,
    /// The shelf was brought up for this drag, so it may be put away after.
    shown: bool,
    /// The model revision when the drag started, to tell whether anything
    /// was dropped on the shelf.
    revision: u64,
}

pub struct DragMonitor {
    shelf: Rc<Shelf>,
    settings: Settings,
    drag_pasteboard: Retained<NSPasteboard>,
    tracker: RefCell<DragTracker>,
    session: Cell<Option<Session>>,
    release_timer: RefCell<Option<Retained<NSTimer>>>,
    /// Keeps the global event monitors installed.
    _monitors: RefCell<Vec<Retained<AnyObject>>>,
}

impl DragMonitor {
    /// Starts watching. Returns `None` when auto-show is turned off.
    pub fn start(shelf: Rc<Shelf>, settings: &Settings) -> Option<Rc<Self>> {
        if settings.appear_on_drag == AppearMode::Off {
            return None;
        }
        // SAFETY: Reading an immutable framework constant.
        let drag_pasteboard = NSPasteboard::pasteboardWithName(unsafe { NSPasteboardNameDrag });
        let monitor = Rc::new(Self {
            shelf,
            settings: settings.clone(),
            drag_pasteboard,
            tracker: RefCell::default(),
            session: Cell::new(None),
            release_timer: RefCell::new(None),
            _monitors: RefCell::default(),
        });

        let weak = Rc::downgrade(&monitor);
        let handler = RcBlock::new(move |event: NonNull<NSEvent>| {
            let Some(monitor) = weak.upgrade() else {
                return;
            };
            // SAFETY: AppKit passes a valid event for the duration of the call.
            let event = unsafe { event.as_ref() };
            match event.r#type() {
                objc2_app_kit::NSEventType::LeftMouseDown => monitor.mouse_down(),
                objc2_app_kit::NSEventType::LeftMouseDragged => monitor.mouse_dragged(),
                _ => {}
            }
        });
        // Global monitors see events sent to other apps (where drags start);
        // they need no special permission for mouse events.
        let installed = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
            NSEventMask::LeftMouseDown | NSEventMask::LeftMouseDragged,
            &handler,
        );
        match installed {
            Some(token) => monitor._monitors.borrow_mut().push(token),
            None => eprintln!("yoinks: could not install the drag monitor"),
        }
        Some(monitor)
    }

    fn mouse_down(&self) {
        self.tracker
            .borrow_mut()
            .mouse_down(self.drag_pasteboard.changeCount());
    }

    fn mouse_dragged(self: &Rc<Self>) {
        let update = self
            .tracker
            .borrow_mut()
            .mouse_dragged(|| self.drag_pasteboard.changeCount());
        match update {
            Update::Started => self.drag_started(),
            Update::Moved => self.drag_moved(),
            Update::None => {}
        }
    }

    fn drag_started(self: &Rc<Self>) {
        let source_ignored = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .and_then(|app| app.bundleIdentifier())
            .is_some_and(|id| self.settings.ignores_app(&id.to_string()));
        self.session.set(Some(Session {
            passive: source_ignored || self.shelf.is_visible(),
            shown: false,
            revision: self.shelf.model().revision(),
        }));
        self.watch_for_release();
        self.drag_moved();
    }

    fn drag_moved(&self) {
        let Some(mut session) = self.session.get() else {
            return;
        };
        if session.passive || session.shown {
            return;
        }
        match self.settings.appear_on_drag {
            AppearMode::Off => return,
            AppearMode::AtEdge => self.shelf.show(),
            AppearMode::AtCursor => self.shelf.show_at_cursor(),
            AppearMode::NearEdge => {
                let near = self.shelf.screen_under_cursor().is_some_and(|screen| {
                    near_edge(screen, NSEvent::mouseLocation(), self.shelf.edge())
                });
                if !near {
                    return;
                }
                self.shelf.show();
            }
        }
        session.shown = true;
        self.session.set(Some(session));
    }

    /// Polls the mouse button until it is released, then ends the drag.
    fn watch_for_release(self: &Rc<Self>) {
        let weak: Weak<Self> = Rc::downgrade(self);
        let block = RcBlock::new(move |_timer: NonNull<NSTimer>| {
            let Some(monitor) = weak.upgrade() else {
                return;
            };
            if NSEvent::pressedMouseButtons() & 1 == 0 {
                monitor.drag_ended();
            }
        });
        // SAFETY: The block only touches main-thread state and the timer is
        // scheduled on the main run loop.
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_repeats_block(
                RELEASE_POLL_INTERVAL,
                true,
                &block,
            )
        };
        if let Some(previous) = self.release_timer.replace(Some(timer)) {
            previous.invalidate();
        }
    }

    fn drag_ended(&self) {
        if let Some(timer) = self.release_timer.take() {
            timer.invalidate();
        }
        self.tracker.borrow_mut().mouse_released();
        let Some(session) = self.session.take() else {
            return;
        };
        // Put the shelf away unless something was dropped on it or the
        // cursor ended up over it (a drop may still be arriving).
        let used = self.shelf.model().revision() != session.revision;
        if session.shown && !used && !self.shelf.contains_mouse() {
            self.shelf.hide();
        }
    }
}
