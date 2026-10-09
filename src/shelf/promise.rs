//! Drag-out writer for content items (text, rich text, links).
//!
//! These are offered as a file promise plus the content itself. Finder
//! fulfils the promise and gets a file; text editors, text fields and
//! browsers take the content. (Offering a file URL instead makes apps like
//! TextEdit insert the file as an attachment rather than its text.)

use std::fs;
use std::path::PathBuf;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSFilePromiseProvider, NSFilePromiseProviderDelegate, NSPasteboard, NSPasteboardType,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSCocoaErrorDomain, NSError, NSObjectProtocol, NSOperationQueue,
    NSString, NSURL,
};

pub struct PromiseIvars {
    /// The owned file Finder receives a copy of.
    source: PathBuf,
    /// Extra pasteboard types and their values, e.g. `public.utf8-plain-text`.
    extras: Vec<(Retained<NSPasteboardType>, Retained<AnyObject>)>,
}

define_class!(
    // SAFETY:
    // - NSFilePromiseProvider has no special subclassing requirements.
    // - `ContentPromise` does not implement `Drop`.
    #[unsafe(super(NSFilePromiseProvider))]
    #[thread_kind = MainThreadOnly]
    #[name = "YoinksContentPromise"]
    #[ivars = PromiseIvars]
    pub struct ContentPromise;

    // NSPasteboardWriting overrides: the promise types plus the content.
    impl ContentPromise {
        #[unsafe(method_id(writableTypesForPasteboard:))]
        fn writable_types(&self, pasteboard: &NSPasteboard) -> Retained<NSArray<NSPasteboardType>> {
            // SAFETY: Calling the superclass implementation of the same method.
            let inherited: Retained<NSArray<NSPasteboardType>> =
                unsafe { msg_send![super(self), writableTypesForPasteboard: pasteboard] };
            let mut types: Vec<Retained<NSPasteboardType>> = inherited.to_vec();
            types.extend(self.ivars().extras.iter().map(|(t, _)| t.clone()));
            NSArray::from_retained_slice(&types)
        }

        #[unsafe(method_id(pasteboardPropertyListForType:))]
        fn property_list(&self, r#type: &NSPasteboardType) -> Option<Retained<AnyObject>> {
            match self.ivars().extras.iter().find(|(t, _)| **t == *r#type) {
                Some((_, value)) => Some(value.clone()),
                // SAFETY: Calling the superclass implementation of the same method.
                None => unsafe { msg_send![super(self), pasteboardPropertyListForType: r#type] },
            }
        }
    }

    // SAFETY: `NSObjectProtocol` has no safety requirements.
    unsafe impl NSObjectProtocol for ContentPromise {}

    // SAFETY: The method signatures match the protocol.
    unsafe impl NSFilePromiseProviderDelegate for ContentPromise {
        #[unsafe(method_id(filePromiseProvider:fileNameForType:))]
        fn file_name(&self, _provider: &NSFilePromiseProvider, _type: &NSString) -> Retained<NSString> {
            let name = self.ivars().source.file_name().unwrap_or_default();
            NSString::from_str(&name.to_string_lossy())
        }

        #[unsafe(method(filePromiseProvider:writePromiseToURL:completionHandler:))]
        fn write_promise(
            &self,
            _provider: &NSFilePromiseProvider,
            url: &NSURL,
            completion: &block2::DynBlock<dyn Fn(*mut NSError)>,
        ) {
            let result = match url.to_file_path() {
                Some(destination) => fs::copy(&self.ivars().source, destination).map(|_| ()),
                None => Err(std::io::Error::other("not a file URL")),
            };
            match result {
                Ok(()) => completion.call((std::ptr::null_mut(),)),
                Err(error) => {
                    eprintln!("yoinks: could not write dragged file: {error}");
                    // SAFETY: A Cocoa domain error with no user info is valid.
                    let error = unsafe {
                        // NSFileWriteUnknownError
                        NSError::errorWithDomain_code_userInfo(NSCocoaErrorDomain, 512, None)
                    };
                    completion.call((Retained::as_ptr(&error).cast_mut(),));
                }
            }
        }

        // Our state lives on the main thread; write there too.
        #[unsafe(method_id(operationQueueForFilePromiseProvider:))]
        fn operation_queue(&self, _provider: &NSFilePromiseProvider) -> Retained<NSOperationQueue> {
            NSOperationQueue::mainQueue()
        }
    }
);

impl ContentPromise {
    /// A promise for the file at `source` (a UTI is required by AppKit) that
    /// also offers `extras` directly on the pasteboard.
    pub fn writer(
        mtm: MainThreadMarker,
        source: PathBuf,
        file_type: &str,
        extras: Vec<(Retained<NSPasteboardType>, Retained<AnyObject>)>,
    ) -> Retained<ProtocolObject<dyn objc2_app_kit::NSPasteboardWriting>> {
        let this = Self::alloc(mtm).set_ivars(PromiseIvars { source, extras });
        // SAFETY: NSFilePromiseProvider's `init` is valid; the file type and
        // delegate are set right after.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.setFileType(&NSString::from_str(file_type));
        // The provider is its own delegate; the reference is weak, so there
        // is no cycle.
        this.setDelegate(Some(ProtocolObject::from_ref(&*this)));
        ProtocolObject::from_retained(Retained::into_super(this))
    }
}
