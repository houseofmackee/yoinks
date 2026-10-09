//! A button that records a keyboard shortcut: click it, then press keys.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBezelStyle, NSButton, NSControl, NSEvent, NSEventModifierFlags, NSResponder, NSView,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSString};

use crate::hotkey::{Modifiers, Shortcut};

/// What happened during a recording.
pub enum Recorded {
    Shortcut(Shortcut),
    /// Delete or Backspace: no shortcut.
    Cleared,
    /// Escape, or focus moved away.
    Cancelled,
    /// A key that cannot be used; the message says why.
    Invalid(String),
}

pub struct RecorderIvars {
    recording: Cell<bool>,
    /// Called when recording starts (true) and ends (false), so the global
    /// shortcut can be suspended meanwhile.
    on_recording: Box<dyn Fn(bool)>,
    on_recorded: Box<dyn Fn(Recorded)>,
}

const ESCAPE: u16 = 53;
const DELETE: u16 = 51;
const FORWARD_DELETE: u16 = 117;

define_class!(
    // SAFETY:
    // - NSButton has no special subclassing requirements.
    // - `ShortcutRecorder` does not implement `Drop`.
    #[unsafe(super(NSButton, NSControl, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "YoinksShortcutRecorder"]
    #[ivars = RecorderIvars]
    pub struct ShortcutRecorder;

    impl ShortcutRecorder {
        #[unsafe(method(startRecording:))]
        fn start_recording(&self, _sender: Option<&AnyObject>) {
            if let Some(window) = self.window() {
                window.makeFirstResponder(Some(self));
            }
            self.ivars().recording.set(true);
            self.setTitle(&NSString::from_str("Type a shortcut…"));
            (self.ivars().on_recording)(true);
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> bool {
            self.finish(Recorded::Cancelled);
            true
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if self.ivars().recording.get() {
                self.record(event);
            } else {
                // SAFETY: Calling the superclass implementation of the same method.
                let _: () = unsafe { msg_send![super(self), keyDown: event] };
            }
        }

        // Command shortcuts arrive here before keyDown:.
        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> bool {
            if self.ivars().recording.get() {
                self.record(event);
                true
            } else {
                // SAFETY: Calling the superclass implementation of the same method.
                unsafe { msg_send![super(self), performKeyEquivalent: event] }
            }
        }
    }
);

impl ShortcutRecorder {
    pub fn new(
        mtm: MainThreadMarker,
        on_recording: impl Fn(bool) + 'static,
        on_recorded: impl Fn(Recorded) + 'static,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(RecorderIvars {
            recording: Cell::new(false),
            on_recording: Box::new(on_recording),
            on_recorded: Box::new(on_recorded),
        });
        // SAFETY: NSView's `init` calls the designated `initWithFrame:`.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.setBezelStyle(NSBezelStyle::Push);
        // SAFETY: The action takes one sender argument; we are the target.
        unsafe {
            this.setTarget(Some(&this));
            this.setAction(Some(sel!(startRecording:)));
        }
        this
    }

    /// Shows the current shortcut (or "None") when not recording.
    pub fn show(&self, shortcut: Option<&Shortcut>) {
        let title = shortcut.map_or_else(|| "None".to_owned(), Shortcut::symbols);
        self.setTitle(&NSString::from_str(&title));
    }

    fn record(&self, event: &NSEvent) {
        let flags = event.modifierFlags();
        let modifiers = Modifiers {
            control: flags.contains(NSEventModifierFlags::Control),
            option: flags.contains(NSEventModifierFlags::Option),
            shift: flags.contains(NSEventModifierFlags::Shift),
            command: flags.contains(NSEventModifierFlags::Command),
        };
        let result = match event.keyCode() {
            ESCAPE if modifiers == Modifiers::default() => Recorded::Cancelled,
            DELETE | FORWARD_DELETE if modifiers == Modifiers::default() => Recorded::Cleared,
            code => match Shortcut::from_key(u32::from(code), modifiers) {
                Ok(shortcut) => Recorded::Shortcut(shortcut),
                Err(message) => Recorded::Invalid(message),
            },
        };
        self.finish(result);
    }

    fn finish(&self, result: Recorded) {
        if !self.ivars().recording.replace(false) {
            return;
        }
        (self.ivars().on_recording)(false);
        (self.ivars().on_recorded)(result);
    }
}
