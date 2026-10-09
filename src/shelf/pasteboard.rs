//! Reads a drag pasteboard into files, content and file promises.

use std::path::PathBuf;

use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2::{ClassType, Message};
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSFilePromiseReceiver, NSPasteboard, NSPasteboardItem,
    NSPasteboardType, NSPasteboardTypeFileURL, NSPasteboardTypePNG, NSPasteboardTypeRTF,
    NSPasteboardTypeString, NSPasteboardTypeTIFF, NSPasteboardTypeURL,
};
use objc2_foundation::{NSArray, NSDictionary, NSURL, ns_string};

use super::materialize::Content;

/// Everything usable from one drop.
#[derive(Default)]
pub struct Dropped {
    pub files: Vec<PathBuf>,
    pub contents: Vec<Content>,
    pub promises: Vec<Retained<NSFilePromiseReceiver>>,
}

fn jpeg_type() -> &'static NSPasteboardType {
    ns_string!("public.jpeg")
}

fn url_name_type() -> &'static NSPasteboardType {
    ns_string!("public.url-name")
}

/// The pasteboard types the shelf registers for as a drop target.
pub fn accepted_types() -> Retained<NSArray<NSPasteboardType>> {
    // SAFETY: Reading immutable framework constants.
    let constants = unsafe {
        [
            NSPasteboardTypeFileURL,
            NSPasteboardTypeURL,
            NSPasteboardTypePNG,
            NSPasteboardTypeTIFF,
            NSPasteboardTypeRTF,
            NSPasteboardTypeString,
        ]
    };
    let mut types: Vec<Retained<NSPasteboardType>> = constants.iter().map(|t| t.retain()).collect();
    types.push(jpeg_type().retain());
    types.extend(NSFilePromiseReceiver::readableDraggedTypes().iter());
    NSArray::from_retained_slice(&types)
}

/// Reads one drop. Each pasteboard item contributes at most one thing, in
/// this order: a file, a promised file, a web link, an image, rich text,
/// plain text.
pub fn read(pasteboard: &NSPasteboard) -> Dropped {
    let mut dropped = Dropped::default();
    let promise_types = NSFilePromiseReceiver::readableDraggedTypes();
    let mut has_promises = false;

    for item in pasteboard.pasteboardItems().unwrap_or_default().iter() {
        let types = item.types();
        let has = |t: &NSPasteboardType| types.containsObject(t);

        // SAFETY: Reading immutable framework constants.
        let (file_url, url, png, tiff, rtf, string) = unsafe {
            (
                NSPasteboardTypeFileURL,
                NSPasteboardTypeURL,
                NSPasteboardTypePNG,
                NSPasteboardTypeTIFF,
                NSPasteboardTypeRTF,
                NSPasteboardTypeString,
            )
        };

        if has(file_url)
            && let Some(path) = file_path(&item, file_url)
        {
            dropped.files.push(path);
            continue;
        }
        if promise_types.iter().any(|t| has(&t)) {
            has_promises = true;
            continue;
        }
        if has(url)
            && let Some(link) = item.stringForType(url)
        {
            let title = item.stringForType(url_name_type()).map(|t| t.to_string());
            dropped.contents.push(Content::Url {
                url: link.to_string(),
                title,
            });
            continue;
        }
        if let Some(content) = image(&item, png, tiff) {
            dropped.contents.push(content);
            continue;
        }
        if has(rtf)
            && let Some(data) = item.dataForType(rtf)
        {
            let plain = item.stringForType(string).map(|s| s.to_string());
            dropped.contents.push(Content::RichText {
                rtf: data.to_vec(),
                plain: plain.unwrap_or_default(),
            });
            continue;
        }
        if let Some(text) = item.stringForType(string) {
            let text = text.to_string();
            if !text.trim().is_empty() {
                dropped.contents.push(Content::Text(text));
            }
        }
    }

    if has_promises {
        let classes: Retained<NSArray<AnyClass>> =
            NSArray::from_slice(&[NSFilePromiseReceiver::class()]);
        // SAFETY: NSFilePromiseReceiver conforms to NSPasteboardReading, and
        // the returned objects are instances of it.
        let receivers = unsafe { pasteboard.readObjectsForClasses_options(&classes, None) };
        for object in receivers.unwrap_or_default().iter() {
            if let Ok(receiver) = object.downcast::<NSFilePromiseReceiver>() {
                dropped.promises.push(receiver);
            }
        }
    }

    dropped
}

fn file_path(item: &NSPasteboardItem, file_url: &NSPasteboardType) -> Option<PathBuf> {
    let string = item.stringForType(file_url)?;
    NSURL::URLWithString(&string)?.to_file_path()
}

/// PNG and JPEG are kept as is; TIFF (common from screenshots and some apps)
/// is converted to PNG to keep files small.
fn image(
    item: &NSPasteboardItem,
    png: &NSPasteboardType,
    tiff: &NSPasteboardType,
) -> Option<Content> {
    if let Some(data) = item.dataForType(png) {
        return Some(Content::Image {
            data: data.to_vec(),
            extension: "png",
        });
    }
    if let Some(data) = item.dataForType(jpeg_type()) {
        return Some(Content::Image {
            data: data.to_vec(),
            extension: "jpg",
        });
    }
    let tiff_data = item.dataForType(tiff)?;
    let rep = NSBitmapImageRep::imageRepWithData(&tiff_data)?;
    let properties = NSDictionary::new();
    // SAFETY: An empty properties dictionary is valid.
    let png_data =
        unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties) };
    Some(match png_data {
        Some(data) => Content::Image {
            data: data.to_vec(),
            extension: "png",
        },
        None => Content::Image {
            data: tiff_data.to_vec(),
            extension: "tiff",
        },
    })
}
