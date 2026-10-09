//! Registers the global shortcut with Carbon's `RegisterEventHotKey`, which,
//! unlike event taps, needs no Accessibility or Input Monitoring permission.
//! macOS does not report clashes with other apps' shortcuts: if another app
//! registers the same one, both receive it.

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::rc::{Rc, Weak};
use std::time::Instant;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_foundation::NSTimer;

use super::gesture::{Gesture, GestureTracker};
use super::shortcut::Shortcut;

type OSStatus = i32;
type EventRef = *mut c_void;
type EventHandlerCallRef = *mut c_void;
type EventTargetRef = *mut c_void;
type EventHandlerRef = *mut c_void;
type EventHotKeyRef = *mut c_void;
type EventHandlerProc = extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OSStatus;

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

const NO_ERR: OSStatus = 0;
const EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const EVENT_HOT_KEY_PRESSED: u32 = 5;
const EVENT_HOT_KEY_RELEASED: u32 = 6;
const SIGNATURE: u32 = u32::from_be_bytes(*b"YNKS");

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> EventTargetRef;
    fn InstallEventHandler(
        target: EventTargetRef,
        handler: EventHandlerProc,
        num_types: usize,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut EventHandlerRef,
    ) -> OSStatus;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        id: EventHotKeyID,
        target: EventTargetRef,
        options: u32,
        out_ref: *mut EventHotKeyRef,
    ) -> OSStatus;
    fn GetEventKind(event: EventRef) -> u32;
}

thread_local! {
    /// The registered hotkey. Carbon calls back on the main thread with no
    /// way to carry a Rust reference, so it is looked up here.
    static ACTIVE: RefCell<Weak<HotKey>> = const { RefCell::new(Weak::new()) };
}

pub struct HotKey {
    tracker: RefCell<GestureTracker>,
    timer: RefCell<Option<Retained<NSTimer>>>,
    on_gesture: Box<dyn Fn(Gesture)>,
}

impl HotKey {
    /// Registers `shortcut` system-wide. Only one shortcut is supported.
    pub fn register(
        shortcut: &Shortcut,
        on_gesture: impl Fn(Gesture) + 'static,
    ) -> Result<Rc<Self>, String> {
        let hotkey = Rc::new(Self {
            tracker: RefCell::default(),
            timer: RefCell::new(None),
            on_gesture: Box::new(on_gesture),
        });
        let types = [
            EventTypeSpec {
                event_class: EVENT_CLASS_KEYBOARD,
                event_kind: EVENT_HOT_KEY_PRESSED,
            },
            EventTypeSpec {
                event_class: EVENT_CLASS_KEYBOARD,
                event_kind: EVENT_HOT_KEY_RELEASED,
            },
        ];
        let mut handler_ref = ptr::null_mut();
        let mut hotkey_ref = ptr::null_mut();
        // SAFETY: Plain Carbon calls with valid pointers; the handler is a
        // `'static` function and the type list outlives the call.
        unsafe {
            let target = GetApplicationEventTarget();
            let status = InstallEventHandler(
                target,
                handle_event,
                types.len(),
                types.as_ptr(),
                ptr::null_mut(),
                &mut handler_ref,
            );
            if status != NO_ERR {
                return Err(format!("InstallEventHandler failed ({status})"));
            }
            let id = EventHotKeyID {
                signature: SIGNATURE,
                id: 1,
            };
            let status = RegisterEventHotKey(
                shortcut.key_code,
                shortcut.modifiers.carbon_mask(),
                id,
                target,
                0,
                &mut hotkey_ref,
            );
            if status != NO_ERR {
                return Err(format!(
                    "could not register the shortcut {shortcut} ({status})"
                ));
            }
        }
        ACTIVE.with(|active| *active.borrow_mut() = Rc::downgrade(&hotkey));
        Ok(hotkey)
    }

    fn key_down(self: &Rc<Self>) {
        let gesture = self.tracker.borrow_mut().key_down(Instant::now());
        self.finish(gesture);
    }

    fn key_up(self: &Rc<Self>) {
        let gesture = self.tracker.borrow_mut().key_up(Instant::now());
        self.finish(gesture);
    }

    fn tick(self: &Rc<Self>) {
        let gesture = self.tracker.borrow_mut().tick(Instant::now());
        self.finish(gesture);
    }

    /// Reports `gesture`, then arms a one-shot timer for the tracker's next
    /// deadline, if any.
    fn finish(self: &Rc<Self>, gesture: Option<Gesture>) {
        if let Some(previous) = self.timer.take() {
            previous.invalidate();
        }
        if let Some(gesture) = gesture {
            (self.on_gesture)(gesture);
        }
        let Some(deadline) = self.tracker.borrow().next_deadline() else {
            return;
        };
        let delay = deadline
            .saturating_duration_since(Instant::now())
            .as_secs_f64();
        let weak = Rc::downgrade(self);
        let block = RcBlock::new(move |_timer: NonNull<NSTimer>| {
            if let Some(hotkey) = weak.upgrade() {
                hotkey.tick();
            }
        });
        // SAFETY: Scheduled on the main run loop; the block only touches
        // main-thread state.
        let timer =
            unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(delay, false, &block) };
        *self.timer.borrow_mut() = Some(timer);
    }
}

extern "C" fn handle_event(
    _call: EventHandlerCallRef,
    event: EventRef,
    _data: *mut c_void,
) -> OSStatus {
    let Some(hotkey) = ACTIVE.with(|active| active.borrow().upgrade()) else {
        return NO_ERR;
    };
    // SAFETY: Carbon passes a valid event.
    match unsafe { GetEventKind(event) } {
        EVENT_HOT_KEY_PRESSED => hotkey.key_down(),
        EVENT_HOT_KEY_RELEASED => hotkey.key_up(),
        _ => {}
    }
    NO_ERR
}
