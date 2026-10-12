//! A small prompt for a name: a text field with a confirm button and
//! Cancel, used for renaming items and naming shelves. It is a
//! non-activating panel, like the shelf, so it takes typing without
//! bringing Yoinks to the front.

use std::cell::{OnceCell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSColor, NSControl, NSControlTextEditingDelegate, NSFont,
    NSLayoutConstraint, NSPanel, NSResponder, NSStatusWindowLevel, NSTextField,
    NSTextFieldDelegate, NSTextView, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSObject, NSObjectProtocol, NSPoint, NSRange, NSRect, NSSize,
    NSString, ns_string,
};

/// Checks the new name and applies it; an error is shown in the prompt.
type Submit = Box<dyn Fn(&str) -> Result<(), String>>;

const WIDTH: f64 = 360.0;

pub struct PromptIvars {
    submit: Submit,
    panel: OnceCell<Retained<PromptPanel>>,
    field: OnceCell<Retained<NSTextField>>,
    error: OnceCell<Retained<NSTextField>>,
    /// Keeps the prompt alive while it is open.
    this: RefCell<Option<Retained<RenamePrompt>>>,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - `RenamePrompt` does not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "YoinksRenamePrompt"]
    #[ivars = PromptIvars]
    pub struct RenamePrompt;

    impl RenamePrompt {
        #[unsafe(method(confirm:))]
        fn confirm(&self, _sender: Option<&AnyObject>) {
            self.submit();
        }

        #[unsafe(method(cancel:))]
        fn cancel(&self, _sender: Option<&AnyObject>) {
            self.close();
        }
    }

    // SAFETY: `NSObjectProtocol` has no safety requirements.
    unsafe impl NSObjectProtocol for RenamePrompt {}

    // SAFETY: The method signature matches the protocol.
    unsafe impl NSControlTextEditingDelegate for RenamePrompt {
        // Return renames, Escape cancels.
        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn do_command(&self, _control: &NSControl, _text_view: &NSTextView, command: Sel) -> bool {
            self.handle_command(command)
        }
    }

    // SAFETY: No required methods.
    unsafe impl NSTextFieldDelegate for RenamePrompt {}
);

define_class!(
    // SAFETY: NSPanel has no special subclassing requirements.
    #[unsafe(super(NSPanel, NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "YoinksPromptPanel"]
    struct PromptPanel;

    impl PromptPanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }
    }
);

impl RenamePrompt {
    /// Shows the Rename prompt near `anchor` with the file name `current`
    /// filled in; the part before the extension is selected, like Finder.
    pub fn show(
        mtm: MainThreadMarker,
        anchor: &NSView,
        current: &str,
        submit: impl Fn(&str) -> Result<(), String> + 'static,
    ) {
        Self::ask(mtm, anchor, "Rename", "Rename", current, true, submit);
    }

    /// Shows a prompt titled `title` with a `button` to confirm and `current`
    /// filled in: the stem selected if `file_name`, else all of it.
    pub fn ask(
        mtm: MainThreadMarker,
        anchor: &NSView,
        title: &str,
        button: &str,
        current: &str,
        file_name: bool,
        submit: impl Fn(&str) -> Result<(), String> + 'static,
    ) {
        let this = Self::alloc(mtm).set_ivars(PromptIvars {
            submit: Box::new(submit),
            panel: OnceCell::new(),
            field: OnceCell::new(),
            error: OnceCell::new(),
            this: RefCell::new(None),
        });
        // SAFETY: NSObject's `init` has the correct signature.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.build(title, button, current);
        *this.ivars().this.borrow_mut() = Some(this.clone());

        let (Some(panel), Some(field)) = (this.ivars().panel.get(), this.ivars().field.get())
        else {
            return;
        };
        if let Some(window) = anchor.window() {
            // Beside the shelf, at the anchor's height, kept on screen.
            let frame = window.frame();
            let visible = window.screen().map(|s| s.visibleFrame()).unwrap_or(frame);
            let size = panel.frame().size;
            let left = frame.origin.x - size.width - 8.0;
            let x = if left >= visible.origin.x {
                left
            } else {
                frame.origin.x + frame.size.width + 8.0
            };
            let anchor_y = window
                .convertRectToScreen(anchor.convertRect_toView(anchor.bounds(), None))
                .origin
                .y;
            let y = anchor_y.clamp(
                visible.origin.y,
                visible.origin.y + visible.size.height - size.height,
            );
            panel.setFrameOrigin(NSPoint::new(x, y));
        }
        panel.orderFrontRegardless();
        panel.makeKeyWindow();
        panel.makeFirstResponder(Some(field));
        let whole = current.encode_utf16().count();
        let stem_len = if file_name {
            std::path::Path::new(current)
                .file_stem()
                .map_or(whole, |stem| stem.to_string_lossy().encode_utf16().count())
        } else {
            whole
        };
        if let Some(editor) = field.currentEditor() {
            editor.setSelectedRange(NSRange::new(0, stem_len));
        }
    }

    fn handle_command(&self, command: Sel) -> bool {
        if command == sel!(insertNewline:) {
            self.submit();
        } else if command == sel!(cancelOperation:) {
            self.close();
        } else {
            return false;
        }
        true
    }

    /// Applies the typed name, or shows why it can't be used.
    fn submit(&self) {
        let name = self
            .ivars()
            .field
            .get()
            .map(|field| field.stringValue().to_string())
            .unwrap_or_default();
        match (self.ivars().submit)(&name) {
            Ok(()) => self.close(),
            Err(message) => {
                if let Some(error) = self.ivars().error.get() {
                    error.setStringValue(&NSString::from_str(&message));
                    error.setHidden(false);
                }
            }
        }
    }

    fn close(&self) {
        if let Some(panel) = self.ivars().panel.get() {
            panel.orderOut(None);
        }
        // Let go of ourselves once AppKit is done with this event.
        self.ivars().this.borrow_mut().take();
    }

    fn build(&self, title: &str, button: &str, current: &str) {
        let mtm = self.mtm();
        let target = Some::<&AnyObject>(self.as_ref());
        let field = NSTextField::textFieldWithString(&NSString::from_str(current), mtm);
        // SAFETY: We implement the delegate protocol and outlive the field.
        unsafe { field.setDelegate(Some(ProtocolObject::from_ref(self))) };
        let error = NSTextField::wrappingLabelWithString(ns_string!(""), mtm);
        error.setTextColor(Some(&NSColor::systemRedColor()));
        error.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        error.setHidden(true);
        // SAFETY: The actions take one sender argument; we are the target.
        let (cancel, rename) = unsafe {
            (
                NSButton::buttonWithTitle_target_action(
                    ns_string!("Cancel"),
                    target,
                    Some(sel!(cancel:)),
                    mtm,
                ),
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(button),
                    target,
                    Some(sel!(confirm:)),
                    mtm,
                ),
            )
        };
        rename.setKeyEquivalent(ns_string!("\r"));

        let content = NSView::new(mtm);
        let views: [&NSView; 4] = [&field, &error, &cancel, &rename];
        for view in views {
            view.setTranslatesAutoresizingMaskIntoConstraints(false);
            content.addSubview(view);
        }
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            content.widthAnchor().constraintEqualToConstant(WIDTH),
            field
                .topAnchor()
                .constraintEqualToAnchor_constant(&content.topAnchor(), 16.0),
            field
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&content.leadingAnchor(), 16.0),
            field
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&content.trailingAnchor(), -16.0),
            error
                .topAnchor()
                .constraintEqualToAnchor_constant(&field.bottomAnchor(), 6.0),
            error
                .leadingAnchor()
                .constraintEqualToAnchor(&field.leadingAnchor()),
            error
                .trailingAnchor()
                .constraintEqualToAnchor(&field.trailingAnchor()),
            rename
                .topAnchor()
                .constraintEqualToAnchor_constant(&error.bottomAnchor(), 10.0),
            rename
                .trailingAnchor()
                .constraintEqualToAnchor(&field.trailingAnchor()),
            rename
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&content.bottomAnchor(), -14.0),
            cancel
                .centerYAnchor()
                .constraintEqualToAnchor(&rename.centerYAnchor()),
            cancel
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&rename.leadingAnchor(), -8.0),
        ]));

        let style = NSWindowStyleMask::Titled | NSWindowStyleMask::NonactivatingPanel;
        // SAFETY: Inherited designated initializer; `PromptPanel` has no ivars.
        let panel: Retained<PromptPanel> = unsafe {
            msg_send![
                PromptPanel::alloc(mtm),
                initWithContentRect: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, 120.0)),
                styleMask: style,
                backing: NSBackingStoreType::Buffered,
                defer: true,
            ]
        };
        // SAFETY: We hold a strong reference to the panel while it is open.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setLevel(NSStatusWindowLevel);
        panel.setHidesOnDeactivate(false);
        panel.setTitle(&NSString::from_str(title));
        panel.setContentView(Some(&content));
        let fitted = content.fittingSize();
        panel.setContentSize(fitted);

        let ivars = self.ivars();
        let _ = ivars.panel.set(panel);
        let _ = ivars.field.set(field);
        let _ = ivars.error.set(error);
    }
}
