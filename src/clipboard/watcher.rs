//! Watches the general pasteboard and records copies into the history.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSPasteboard, NSPasteboardItem, NSPasteboardType, NSPasteboardTypeFileURL, NSPasteboardTypePNG,
    NSPasteboardTypeRTF, NSPasteboardTypeString, NSPasteboardTypeURL, NSPasteboardWriting,
    NSWorkspace,
};
use objc2_foundation::{NSArray, NSData, NSString, NSTimer, NSURL, ns_string};
use uuid::Uuid;

use super::filters;
use super::history::{Clip, ClipContent, History, Source};
use crate::settings::Settings;
use crate::shelf::{Content, Dropped, read_pasteboard};

const POLL_INTERVAL: f64 = 0.5;
/// Larger copies are not recorded.
const MAX_TEXT_BYTES: usize = 1_000_000;
const MAX_IMAGE_BYTES: usize = 20_000_000;

pub struct ClipboardHistory {
    history: RefCell<History>,
    /// Payload files (`<clip-id>.png`, `.rtf`).
    clips_dir: PathBuf,
    ignored_apps: Vec<String>,
    paused: Cell<bool>,
    /// The general pasteboard's change count last seen (or set by us).
    last_change: Cell<isize>,
    on_change: RefCell<Option<Box<dyn Fn()>>>,
    _timer: RefCell<Option<Retained<NSTimer>>>,
}

impl ClipboardHistory {
    /// Starts watching, unless clipboard history is turned off. Whatever is
    /// on the clipboard at launch is not recorded.
    pub fn start(data_dir: &Path, settings: &Settings) -> Option<Rc<Self>> {
        if !settings.clipboard_history {
            return None;
        }
        let clips_dir = data_dir.join("Clips");
        let _ = fs::create_dir_all(&clips_dir);
        let history = History::load(
            data_dir.join("clipboard.json"),
            settings.clipboard_history_size,
        );
        remove_orphan_payloads(&clips_dir, history.clips());

        let this = Rc::new(Self {
            history: RefCell::new(history),
            clips_dir,
            ignored_apps: settings.clipboard_ignored_apps.clone(),
            paused: Cell::new(false),
            last_change: Cell::new(NSPasteboard::generalPasteboard().changeCount()),
            on_change: RefCell::new(None),
            _timer: RefCell::new(None),
        });
        let weak = Rc::downgrade(&this);
        let block = RcBlock::new(move |_timer: NonNull<NSTimer>| {
            if let Some(this) = weak.upgrade() {
                this.poll();
            }
        });
        // SAFETY: Scheduled on the main run loop; the block only touches
        // main-thread state.
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_repeats_block(POLL_INTERVAL, true, &block)
        };
        *this._timer.borrow_mut() = Some(timer);
        Some(this)
    }

    /// Called whenever the history changes (for the history window).
    pub fn set_on_change(&self, f: impl Fn() + 'static) {
        *self.on_change.borrow_mut() = Some(Box::new(f));
    }

    pub fn is_paused(&self) -> bool {
        self.paused.get()
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.set(paused);
        // Copies made while paused are skipped, not recorded on resume.
        self.last_change
            .set(NSPasteboard::generalPasteboard().changeCount());
    }

    pub fn search(&self, query: &str) -> Vec<Clip> {
        self.history.borrow().search(query)
    }

    /// Puts a clip back on the clipboard and moves it to the top.
    pub fn copy(&self, id: Uuid) -> bool {
        let Some(clip) = self.history.borrow().get(id).cloned() else {
            return false;
        };
        let writers = writers(&clip.content);
        if writers.is_empty() {
            return false;
        }
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        let ok = pasteboard.writeObjects(&NSArray::from_retained_slice(&writers));
        // Our own write is not a new copy.
        self.last_change.set(pasteboard.changeCount());
        self.history.borrow_mut().touch(id);
        self.changed();
        ok
    }

    /// The clip as a shelf drop.
    pub fn to_dropped(&self, id: Uuid) -> Option<Dropped> {
        let history = self.history.borrow();
        let clip = history.get(id)?;
        let mut dropped = Dropped::default();
        match &clip.content {
            ClipContent::Files { paths } => dropped.files = paths.clone(),
            ClipContent::Text { text } => dropped.contents.push(Content::Text(text.clone())),
            ClipContent::RichText { plain, file } => dropped.contents.push(Content::RichText {
                rtf: fs::read(file).ok()?,
                plain: plain.clone(),
            }),
            ClipContent::Image { file, .. } => {
                let extension = match file.extension().and_then(|e| e.to_str()) {
                    Some("jpg") => "jpg",
                    Some("tiff") => "tiff",
                    _ => "png",
                };
                dropped.contents.push(Content::Image {
                    data: fs::read(file).ok()?,
                    extension,
                });
            }
            ClipContent::Url { url, title } => dropped.contents.push(Content::Url {
                url: url.clone(),
                title: title.clone(),
            }),
        }
        Some(dropped)
    }

    pub fn toggle_pin(&self, id: Uuid) {
        let removed = self.history.borrow_mut().toggle_pin(id);
        self.delete_payloads(&removed);
        self.changed();
    }

    pub fn remove(&self, id: Uuid) {
        let removed = self.history.borrow_mut().remove(id);
        self.delete_payloads(removed.as_slice());
        self.changed();
    }

    /// Removes every unpinned clip.
    pub fn clear(&self) {
        let removed = self.history.borrow_mut().clear();
        self.delete_payloads(&removed);
        self.changed();
    }

    fn poll(&self) {
        let pasteboard = NSPasteboard::generalPasteboard();
        let count = pasteboard.changeCount();
        if count == self.last_change.replace(count) || self.paused.get() {
            return;
        }
        self.record(&pasteboard);
    }

    fn record(&self, pasteboard: &NSPasteboard) {
        let types: Vec<String> = pasteboard
            .types()
            .map(|types| types.iter().map(|t| t.to_string()).collect())
            .unwrap_or_default();
        let source = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|app| Source {
                bundle_id: app.bundleIdentifier().map(|id| id.to_string()),
                name: app.localizedName().map(|name| name.to_string()),
            });
        let source_id = source.as_ref().and_then(|s| s.bundle_id.as_deref());
        if !filters::should_record(&types, source_id, &self.ignored_apps) {
            return;
        }
        let id = Uuid::new_v4();
        let Some(content) = self.content(id, read_pasteboard(pasteboard)) else {
            return;
        };
        let clip = Clip {
            id,
            ..Clip::new(content, source)
        };
        let removed = self.history.borrow_mut().push(clip);
        self.delete_payloads(&removed);
        self.changed();
    }

    /// Turns a pasteboard read into clip content, writing large payloads to
    /// files named after the clip.
    fn content(&self, id: Uuid, dropped: Dropped) -> Option<ClipContent> {
        if !dropped.files.is_empty() {
            return Some(ClipContent::Files {
                paths: dropped.files,
            });
        }
        let write = |extension: &str, bytes: &[u8]| -> Option<PathBuf> {
            let file = self.clips_dir.join(format!("{id}.{extension}"));
            fs::write(&file, bytes)
                .inspect_err(|error| eprintln!("yoinks: could not save clip: {error}"))
                .ok()?;
            Some(file)
        };
        match dropped.contents.into_iter().next()? {
            Content::Text(text) if text.len() <= MAX_TEXT_BYTES => Some(ClipContent::Text { text }),
            Content::RichText { rtf, plain } if rtf.len() <= MAX_TEXT_BYTES => {
                Some(ClipContent::RichText {
                    plain,
                    file: write("rtf", &rtf)?,
                })
            }
            Content::Image { data, extension } if data.len() <= MAX_IMAGE_BYTES => {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                data.hash(&mut hasher);
                Some(ClipContent::Image {
                    digest: hasher.finish(),
                    file: write(extension, &data)?,
                })
            }
            Content::Url { url, title } => Some(ClipContent::Url { url, title }),
            _ => None,
        }
    }

    fn delete_payloads(&self, clips: &[Clip]) {
        for file in clips.iter().filter_map(|clip| clip.content.file()) {
            // Only ever delete inside our own folder.
            if file.parent() == Some(self.clips_dir.as_path()) {
                let _ = fs::remove_file(file);
            }
        }
    }

    fn changed(&self) {
        if let Some(on_change) = &*self.on_change.borrow() {
            on_change();
        }
    }
}

/// Deletes payload files no clip refers to.
fn remove_orphan_payloads(dir: &Path, clips: &[Clip]) {
    let live: HashSet<&Path> = clips.iter().filter_map(|c| c.content.file()).collect();
    let Ok(files) = fs::read_dir(dir) else { return };
    for file in files.flatten() {
        if !live.contains(file.path().as_path()) {
            let _ = fs::remove_file(file.path());
        }
    }
}

/// Pasteboard items that recreate a clip.
fn writers(content: &ClipContent) -> Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> {
    // SAFETY: Reading immutable framework constants.
    let (string, rtf, png, url_type, file_url) = unsafe {
        (
            NSPasteboardTypeString,
            NSPasteboardTypeRTF,
            NSPasteboardTypePNG,
            NSPasteboardTypeURL,
            NSPasteboardTypeFileURL,
        )
    };
    let item = |entries: Vec<(&NSPasteboardType, PasteValue)>| {
        let item = NSPasteboardItem::new();
        for (t, value) in entries {
            match value {
                PasteValue::String(s) => item.setString_forType(&s, t),
                PasteValue::Data(d) => item.setData_forType(&d, t),
            };
        }
        ProtocolObject::from_retained(item)
    };
    match content {
        ClipContent::Text { text } => {
            vec![item(vec![(
                string,
                PasteValue::String(NSString::from_str(text)),
            )])]
        }
        ClipContent::RichText { plain, file } => {
            let mut entries = vec![(string, PasteValue::String(NSString::from_str(plain)))];
            if let Ok(bytes) = fs::read(file) {
                entries.insert(0, (rtf, PasteValue::Data(NSData::from_vec(bytes))));
            }
            vec![item(entries)]
        }
        ClipContent::Image { file, .. } => match fs::read(file) {
            Ok(bytes) => {
                let image_type: &NSPasteboardType = if file.extension().is_some_and(|e| e == "jpg")
                {
                    ns_string!("public.jpeg")
                } else {
                    png
                };
                vec![item(vec![(
                    image_type,
                    PasteValue::Data(NSData::from_vec(bytes)),
                )])]
            }
            Err(_) => Vec::new(),
        },
        ClipContent::Files { paths } => paths
            .iter()
            .filter_map(|path| NSURL::from_file_path(path)?.absoluteString())
            .map(|url| item(vec![(file_url, PasteValue::String(url))]))
            .collect(),
        ClipContent::Url { url, title } => {
            let mut entries = vec![
                (url_type, PasteValue::String(NSString::from_str(url))),
                (string, PasteValue::String(NSString::from_str(url))),
            ];
            if let Some(title) = title {
                entries.push((
                    ns_string!("public.url-name"),
                    PasteValue::String(NSString::from_str(title)),
                ));
            }
            vec![item(entries)]
        }
    }
}

enum PasteValue {
    String(Retained<NSString>),
    Data(Retained<NSData>),
}
