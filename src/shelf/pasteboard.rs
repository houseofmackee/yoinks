//! Reads a drag pasteboard into files, content and file promises.

use std::path::PathBuf;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, ProtocolObject};
use objc2::{AnyThread, ClassType, MainThreadMarker, Message};
use objc2_app_kit::{
    NSAttributedStringAppKitDocumentFormats, NSBitmapImageFileType, NSBitmapImageRep,
    NSFilePromiseReceiver, NSPasteboard, NSPasteboardItem, NSPasteboardType,
    NSPasteboardTypeFileURL, NSPasteboardTypePNG, NSPasteboardTypeRTF, NSPasteboardTypeString,
    NSPasteboardTypeTIFF, NSPasteboardTypeURL, NSPasteboardWriting,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSData, NSDictionary, NSString, NSURL, ns_string,
};

use super::item::{Item, ItemKind};
use super::materialize::Content;
use super::promise::ContentPromise;

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

/// What to put on the pasteboard when dragging `item` out of the shelf.
///
/// Files and images go out as the file itself, so Finder moves or copies them
/// with its usual rules; images also carry their data. Text, rich text and
/// links go out as their content plus a file promise (see `promise.rs`).
pub fn writer(
    mtm: MainThreadMarker,
    item: &Item,
) -> Option<Retained<ProtocolObject<dyn NSPasteboardWriting>>> {
    // SAFETY: Reading immutable framework constants.
    let (file_url_type, url_type, png, rtf, string) = unsafe {
        (
            NSPasteboardTypeFileURL,
            NSPasteboardTypeURL,
            NSPasteboardTypePNG,
            NSPasteboardTypeRTF,
            NSPasteboardTypeString,
        )
    };
    let promise = |file_type: &str, extras: Vec<(&NSPasteboardType, Retained<AnyObject>)>| {
        let extras = extras
            .into_iter()
            .map(|(t, value)| (t.retain(), value))
            .collect();
        Some(ContentPromise::writer(
            mtm,
            item.path.clone(),
            file_type,
            extras,
        ))
    };

    match &item.kind {
        ItemKind::Text => {
            let text = std::fs::read_to_string(&item.path).ok()?;
            promise(
                "public.plain-text",
                vec![(string, object(NSString::from_str(&text)))],
            )
        }
        ItemKind::RichText => {
            let data = NSData::from_vec(std::fs::read(&item.path).ok()?);
            // SAFETY: Passing no document attributes out-parameter is allowed.
            let plain = unsafe {
                NSAttributedString::initWithRTF_documentAttributes(
                    NSAttributedString::alloc(),
                    &data,
                    None,
                )
            };
            let mut extras = vec![(rtf, object(data))];
            if let Some(plain) = plain {
                extras.push((string, object(plain.string())));
            }
            promise("public.rtf", extras)
        }
        ItemKind::Url { url } => {
            let url = NSString::from_str(url);
            promise(
                "com.apple.web-internet-location",
                vec![
                    (url_type, object(url.clone())),
                    (url_name_type(), object(NSString::from_str(&item.title))),
                    (string, object(url)),
                ],
            )
        }
        ItemKind::File | ItemKind::Image => {
            let file_url = NSURL::from_file_path(&item.path)?.absoluteString()?;
            let writer = NSPasteboardItem::new();
            writer.setString_forType(&file_url, file_url_type);
            let image_type = match item.path.extension().and_then(|e| e.to_str()) {
                _ if item.kind != ItemKind::Image => None,
                Some("png") => Some(png),
                Some("jpg") => Some(jpeg_type()),
                _ => None,
            };
            if let (Some(image_type), Ok(bytes)) = (image_type, std::fs::read(&item.path)) {
                writer.setData_forType(&NSData::from_vec(bytes), image_type);
            }
            Some(ProtocolObject::from_retained(writer))
        }
        ItemKind::Pending => None,
    }
}

fn object<T: Message>(value: Retained<T>) -> Retained<AnyObject> {
    // SAFETY: Every Objective-C object is an `AnyObject`.
    unsafe { Retained::cast_unchecked(value) }
}

/// Clipboard items for Copy. Files and images are copied as files (images
/// also as image data, for apps that paste pictures); text, rich text and
/// links as their content only, so pasting gives text rather than a file.
pub fn clipboard_items(items: &[Item]) -> Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> {
    // SAFETY: Reading immutable framework constants.
    let (file_url_type, url_type, png, rtf, string) = unsafe {
        (
            NSPasteboardTypeFileURL,
            NSPasteboardTypeURL,
            NSPasteboardTypePNG,
            NSPasteboardTypeRTF,
            NSPasteboardTypeString,
        )
    };
    items
        .iter()
        .filter_map(|item| {
            let writer = NSPasteboardItem::new();
            match &item.kind {
                ItemKind::Text => {
                    let text = std::fs::read_to_string(&item.path).ok()?;
                    writer.setString_forType(&NSString::from_str(&text), string);
                }
                ItemKind::RichText => {
                    let data = NSData::from_vec(std::fs::read(&item.path).ok()?);
                    writer.setData_forType(&data, rtf);
                    // SAFETY: Passing no document attributes out-parameter is allowed.
                    let plain = unsafe {
                        NSAttributedString::initWithRTF_documentAttributes(
                            NSAttributedString::alloc(),
                            &data,
                            None,
                        )
                    };
                    if let Some(plain) = plain {
                        writer.setString_forType(&plain.string(), string);
                    }
                }
                ItemKind::Url { url } => {
                    let url = NSString::from_str(url);
                    writer.setString_forType(&url, url_type);
                    writer.setString_forType(&NSString::from_str(&item.title), url_name_type());
                    writer.setString_forType(&url, string);
                }
                ItemKind::File | ItemKind::Image => {
                    let file_url = NSURL::from_file_path(&item.path)?.absoluteString()?;
                    writer.setString_forType(&file_url, file_url_type);
                    if item.kind == ItemKind::Image
                        && let Ok(bytes) = std::fs::read(&item.path)
                    {
                        let image_type = match item.path.extension().and_then(|e| e.to_str()) {
                            Some("jpg") => jpeg_type(),
                            _ => png,
                        };
                        writer.setData_forType(&NSData::from_vec(bytes), image_type);
                    }
                }
                ItemKind::Pending => return None,
            }
            Some(ProtocolObject::from_retained(writer))
        })
        .collect()
}
