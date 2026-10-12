//! `ShelfModel` ties the store, the items folder and the view together.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use dispatch2::{DispatchQueue, MainThreadBound};

use block2::RcBlock;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{NSAttributedStringAppKitDocumentFormats, NSPasteboard, NSWorkspace};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSDictionary, NSError, NSOperationQueue, NSString, NSURL,
};
use objc2_uniform_type_identifiers::UTType;
use uuid::Uuid;

use super::actions;
use super::bookmark;
use super::describe::{self, FileFacts};
use super::item::{Entry, Item, ItemKind, file_name};
use super::materialize::ItemsDir;
use super::ocr;
use super::pasteboard::{self, Dropped};
use super::recent::Recent;
use super::store::{JsonFile, ShelfStore};
use super::thumbnail::Thumbnails;
use super::view::ShelfContentView;

pub struct ShelfModel {
    /// Handed to views and background work that call back into the model.
    this: Weak<ShelfModel>,
    store: RefCell<ShelfStore>,
    thumbnails: Thumbnails,
    items_dir: ItemsDir,
    view: Retained<ShelfContentView>,
    /// Entries dragged out of the shelf, most recent first, for recall.
    recent: RefCell<Recent>,
    /// Keep entries on the shelf after they are dragged out.
    keep_after_drag_out: Cell<bool>,
    /// Bumped on every change, so observers can tell whether anything
    /// happened between two points in time.
    revision: Cell<u64>,
}

impl ShelfModel {
    pub fn new(mtm: MainThreadMarker, data_dir: &Path) -> Rc<Self> {
        let items_dir = ItemsDir::new(data_dir.join("Items"));
        items_dir.clean_incoming();
        let store = load_store(&data_dir.join("shelf.json"));
        let model = Rc::new_cyclic(|weak: &Weak<Self>| Self {
            this: weak.clone(),
            store: RefCell::new(store),
            thumbnails: Thumbnails::new(data_dir.join("Thumbnails")),
            items_dir,
            view: ShelfContentView::new(mtm, weak.clone()),
            recent: RefCell::default(),
            keep_after_drag_out: Cell::new(false),
            revision: Cell::new(0),
        });
        model
            .store
            .borrow_mut()
            .retain_map(|item| model.locate(item));
        let live: HashSet<Uuid> = model
            .store
            .borrow()
            .entries()
            .iter()
            .flat_map(|entry| entry.items.iter().map(|item| item.id))
            .collect();
        model.items_dir.remove_orphans(&live);
        model.thumbnails.remove_orphans(&live);
        model.reload();
        model
    }

    pub fn view(&self) -> &ShelfContentView {
        &self.view
    }

    /// A copy of the shelf's entries, newest first.
    pub fn entries(&self) -> Vec<Entry> {
        self.store.borrow().entries().to_vec()
    }

    pub fn is_empty(&self) -> bool {
        self.store.borrow().is_empty()
    }

    pub fn revision(&self) -> u64 {
        self.revision.get()
    }

    pub fn set_keep_after_drag_out(&self, keep: bool) {
        self.keep_after_drag_out.set(keep);
    }

    /// The items to put on the pasteboard when entry `id` is dragged out, or
    /// none while it is still receiving files. Every item gets a bookmark
    /// first, so it can be found again after Finder moves it.
    pub fn drag_items(&self, id: Uuid) -> Vec<Item> {
        let mut store = self.store.borrow_mut();
        let Some(entry) = store.entry(id) else {
            return Vec::new();
        };
        if entry.has_pending() {
            return Vec::new();
        }
        if entry.items.iter().any(|item| item.bookmark.is_none()) {
            store.update(id, |entry| {
                for item in &mut entry.items {
                    if item.bookmark.is_none() {
                        item.bookmark = bookmark::create(&item.path);
                    }
                }
            });
        }
        store
            .entry(id)
            .map(|entry| entry.items.clone())
            .unwrap_or_default()
    }

    /// Entry `id` was dropped somewhere that accepted it. It leaves the shelf
    /// and can be recalled, unless the shelf keeps dragged-out items.
    pub fn dragged_out(&self, id: Uuid) {
        let pinned = self
            .store
            .borrow()
            .entry(id)
            .is_some_and(|entry| entry.pinned);
        if self.keep_after_drag_out.get() || pinned {
            // Finder may have moved the files; follow them so the tile keeps
            // working.
            let mut store = self.store.borrow_mut();
            store.update(id, |entry| {
                entry.items = std::mem::take(&mut entry.items)
                    .into_iter()
                    .filter_map(|item| self.locate(item))
                    .collect();
            });
            if store.entry(id).is_some_and(|entry| entry.items.is_empty()) {
                store.remove(id);
            }
        } else {
            let removed = self.store.borrow_mut().remove(id);
            // Thumbnails are kept while the entry can still be recalled.
            let evicted = removed.and_then(|entry| self.recent.borrow_mut().push(entry));
            if let Some(evicted) = evicted {
                self.delete_files(&[evicted]);
            }
        }
        self.reload();
    }

    pub fn is_pinned(&self, id: Uuid) -> bool {
        self.store
            .borrow()
            .entry(id)
            .is_some_and(|entry| entry.pinned)
    }

    pub fn toggle_pin(&self, id: Uuid) {
        let pinned = self.is_pinned(id);
        if self.store.borrow_mut().set_pinned(id, !pinned) {
            self.reload();
        }
    }

    /// True if Clear Shelf would remove anything.
    pub fn has_unpinned(&self) -> bool {
        self.store.borrow().has_unpinned()
    }

    /// Puts an entry's items on the clipboard: files as files, text as text,
    /// links as links, images as images.
    pub fn copy(&self, id: Uuid) -> bool {
        let items = self.ready_items(id);
        let writers = pasteboard::clipboard_items(&items);
        if writers.is_empty() {
            return false;
        }
        let general = NSPasteboard::generalPasteboard();
        general.clearContents();
        general.writeObjects(&NSArray::from_retained_slice(&writers))
    }

    /// Puts the items' paths (links: their URLs) on the clipboard as text,
    /// one per line.
    pub fn copy_paths(&self, id: Uuid) -> bool {
        let lines: Vec<String> = self
            .ready_items(id)
            .iter()
            .map(|item| match &item.kind {
                ItemKind::Url { url } => url.clone(),
                _ => item.path.display().to_string(),
            })
            .collect();
        if lines.is_empty() {
            return false;
        }
        let general = NSPasteboard::generalPasteboard();
        general.clearContents();
        // SAFETY: Reading an immutable framework constant.
        general.setString_forType(&NSString::from_str(&lines.join("\n")), unsafe {
            objc2_app_kit::NSPasteboardTypeString
        })
    }

    /// URLs to hand to other apps (links as web links).
    pub fn open_urls(&self, id: Uuid) -> Vec<Retained<NSURL>> {
        self.urls(id, true)
    }

    /// Opens the entry's items with the app at `app`.
    pub fn open_with(&self, id: Uuid, app: &NSURL) {
        let urls = self.urls(id, true);
        if urls.is_empty() {
            return;
        }
        let configuration = objc2_app_kit::NSWorkspaceOpenConfiguration::configuration();
        NSWorkspace::sharedWorkspace()
            .openURLs_withApplicationAtURL_configuration_completionHandler(
                &NSArray::from_retained_slice(&urls),
                app,
                &configuration,
                None,
            );
    }

    /// The current file name, if the entry is a single item Yoinks owns
    /// (text, links, images and attachments it saved). User files are never
    /// renamed.
    pub fn renamable_name(&self, id: Uuid) -> Option<String> {
        let store = self.store.borrow();
        match store.entry(id)?.items.as_slice() {
            [item] if item.owned && item.kind != ItemKind::Pending => Some(file_name(&item.path)),
            _ => None,
        }
    }

    /// Renames an owned item's file, which is the name it gets when dragged
    /// out.
    pub fn rename(&self, id: Uuid, name: &str) -> Result<(), String> {
        let name = actions::validate_name(name)?;
        let Some(old) = self.renamable_name(id).and_then(|_| {
            self.store
                .borrow()
                .entry(id)
                .map(|entry| entry.items[0].path.clone())
        }) else {
            return Err("Only items Yoinks saved can be renamed".to_owned());
        };
        let new = actions::renamed_path(&old, &name);
        if new == old {
            return Ok(());
        }
        if new.exists() {
            return Err(format!("\"{}\" already exists", file_name(&new)));
        }
        fs::rename(&old, &new).map_err(|error| error.to_string())?;
        self.store.borrow_mut().update(id, |entry| {
            let item = &mut entry.items[0];
            item.title = match item.kind {
                ItemKind::File | ItemKind::Image => file_name(&new),
                _ => new
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| file_name(&new)),
            };
            item.path = new.clone();
            item.bookmark = bookmark::create(&new);
        });
        self.thumbnails
            .remove(self.store.borrow().entry(id).map_or(id, |e| e.items[0].id));
        self.reload();
        Ok(())
    }

    /// Paths of the entry's items that are images.
    pub fn image_paths(&self, id: Uuid) -> Vec<PathBuf> {
        self.ready_items(id)
            .into_iter()
            .filter(is_image)
            .map(|item| item.path)
            .collect()
    }

    /// Paths of the entry's items (for Compress).
    pub fn item_paths(&self, id: Uuid) -> Vec<PathBuf> {
        self.ready_items(id)
            .into_iter()
            .map(|item| item.path)
            .collect()
    }

    /// Runs `job` on a background thread with a fresh folder to write into;
    /// the files it returns become a new entry (a stack if several). Beeps
    /// if it fails.
    pub fn run_job(
        &self,
        mtm: MainThreadMarker,
        job: impl FnOnce(&Path) -> std::io::Result<Vec<PathBuf>> + Send + 'static,
    ) {
        let Ok(out_dir) = self.items_dir.incoming_dir() else {
            objc2_app_kit::NSBeep();
            return;
        };
        let model = Arc::new(MainThreadBound::new(self.this.clone(), mtm));
        std::thread::spawn(move || {
            let result = job(&out_dir);
            DispatchQueue::main().exec_async(move || {
                let mtm = MainThreadMarker::new().expect("main queue runs on the main thread");
                if let Some(model) = model.get(mtm).upgrade() {
                    model.job_finished(&out_dir, result);
                }
            });
        });
    }

    /// Copy Text: recognizes the text in the entry's images on a background
    /// thread and puts it on the clipboard. Beeps if there is none.
    pub fn copy_text(&self, id: Uuid) {
        let images = self.image_paths(id);
        if images.is_empty() {
            objc2_app_kit::NSBeep();
            return;
        }
        std::thread::spawn(move || {
            let result = ocr::recognize_text(&images);
            DispatchQueue::main().exec_async(move || {
                let text = result
                    .inspect_err(|error| eprintln!("yoinks: text recognition failed: {error}"))
                    .unwrap_or_default();
                if text.trim().is_empty() {
                    objc2_app_kit::NSBeep();
                    return;
                }
                let general = NSPasteboard::generalPasteboard();
                general.clearContents();
                // SAFETY: Reading an immutable framework constant.
                general.setString_forType(&NSString::from_str(&text), unsafe {
                    objc2_app_kit::NSPasteboardTypeString
                });
            });
        });
    }

    fn job_finished(&self, out_dir: &Path, result: std::io::Result<Vec<PathBuf>>) {
        let items: Vec<Item> = match result {
            Ok(paths) => paths
                .iter()
                .filter_map(|path| {
                    self.items_dir
                        .adopt(path)
                        .inspect_err(|error| {
                            eprintln!("yoinks: could not keep {}: {error}", path.display())
                        })
                        .ok()
                })
                .collect(),
            Err(error) => {
                eprintln!("yoinks: action failed: {error}");
                Vec::new()
            }
        };
        let _ = fs::remove_dir_all(out_dir);
        if items.is_empty() {
            objc2_app_kit::NSBeep();
            return;
        }
        self.store.borrow_mut().insert(Entry::new(items));
        self.reload();
    }

    /// The entry's items, unless it is still receiving files.
    fn ready_items(&self, id: Uuid) -> Vec<Item> {
        let store = self.store.borrow();
        match store.entry(id) {
            Some(entry) if !entry.has_pending() => entry.items.clone(),
            _ => Vec::new(),
        }
    }

    /// Adds whatever is on the clipboard (files, text, links, images) to the
    /// shelf. Returns false if there was nothing usable.
    pub fn accept_clipboard(self: &Rc<Self>) -> bool {
        self.accept(pasteboard::read(&NSPasteboard::generalPasteboard()))
    }

    /// The text of a text item, for searching the shelf (the first 64 KB).
    pub fn search_text(&self, item: &Item) -> Option<String> {
        use std::io::Read;
        const LIMIT: u64 = 64 * 1024;
        if item.kind != ItemKind::Text {
            return None;
        }
        let mut text = Vec::new();
        fs::File::open(&item.path)
            .ok()?
            .take(LIMIT)
            .read_to_end(&mut text)
            .ok()?;
        Some(String::from_utf8_lossy(&text).into_owned())
    }

    /// The cached Quick Look thumbnail for `item`; if there is none yet,
    /// one is generated in the background and the view reloads when ready.
    pub fn thumbnail(&self, mtm: MainThreadMarker, item: &Item) -> Option<PathBuf> {
        self.thumbnails.get_or_request(mtm, item, &self.this)
    }

    /// Deletes all cached thumbnails; they are regenerated as needed.
    pub fn clear_thumbnails(&self) {
        self.thumbnails.clear();
        self.reload();
    }

    pub(super) fn thumbnail_finished(&self, _id: Uuid, ok: bool) {
        if ok {
            self.reload();
        }
    }

    /// Hover text for a tile: names, kinds, sizes, locations, text excerpts.
    pub fn tooltip(&self, entry: &Entry) -> String {
        let facts: Vec<FileFacts> = entry.items.iter().map(file_facts).collect();
        let home = std::env::home_dir();
        describe::tooltip(entry, &facts, home.as_deref())
    }

    /// File URLs to show in Quick Look for entry `id`.
    pub fn preview_urls(&self, id: Uuid) -> Vec<Retained<NSURL>> {
        self.urls(id, false)
    }

    pub fn can_recall(&self) -> bool {
        !self.recent.borrow().is_empty()
    }

    /// Puts the most recently dragged-out entry back on the shelf, following
    /// its files to wherever they were moved. Entries whose files are all
    /// gone are skipped. Returns false if there was nothing to recall.
    pub fn recall(&self) -> bool {
        loop {
            let Some(mut entry) = self.recent.borrow_mut().pop() else {
                return false;
            };
            entry.items = std::mem::take(&mut entry.items)
                .into_iter()
                .filter_map(|item| self.locate(item))
                .collect();
            if !entry.items.is_empty() {
                self.store.borrow_mut().insert(entry);
                self.reload();
                return true;
            }
        }
    }

    /// Adds a drop to the shelf as one entry (a stack if it holds several
    /// items). Returns false if nothing in the drop was usable.
    pub fn accept(self: &Rc<Self>, dropped: Dropped) -> bool {
        let mut items: Vec<Item> = dropped
            .files
            .into_iter()
            .map(|path| {
                let bookmark = bookmark::create(&path);
                Item::user_file(path, bookmark)
            })
            .collect();
        for content in dropped.contents {
            match self.items_dir.materialize(content) {
                Ok(item) => items.push(item),
                Err(error) => eprintln!("yoinks: could not save dropped content: {error}"),
            }
        }
        // One placeholder per promise. A promise can deliver several files;
        // extras are appended as they arrive. (`fileNames` cannot be used to
        // count them: it is nil until the files have been received.)
        items.extend(dropped.promises.iter().map(|_| Item::pending()));
        if items.is_empty() {
            return false;
        }

        let entry = Entry::new(items);
        let entry_id = entry.id;
        self.store.borrow_mut().insert(entry);
        for receiver in &dropped.promises {
            self.receive_promise(receiver, entry_id);
        }
        self.reload();
        true
    }

    /// Asks the source app to write its promised files, then swaps each
    /// pending placeholder for the received file.
    fn receive_promise(
        self: &Rc<Self>,
        receiver: &objc2_app_kit::NSFilePromiseReceiver,
        entry_id: Uuid,
    ) {
        let destination = self
            .items_dir
            .incoming_dir()
            .ok()
            .and_then(NSURL::from_directory_path);
        let Some(destination) = destination else {
            eprintln!("yoinks: could not create a folder for promised files");
            self.store.borrow_mut().resolve_pending(entry_id, None);
            return;
        };

        let model = Rc::downgrade(self);
        let reader = RcBlock::new(move |url: NonNull<NSURL>, error: *mut NSError| {
            let Some(model) = model.upgrade() else { return };
            // SAFETY: AppKit passes a valid URL and a valid-or-null error.
            let (url, error) = unsafe { (url.as_ref(), error.as_ref()) };
            let item = match (error, url.to_file_path()) {
                (None, Some(path)) => model
                    .items_dir
                    .adopt(&path)
                    .inspect_err(|error| eprintln!("yoinks: could not keep promised file: {error}"))
                    .ok(),
                (Some(error), _) => {
                    eprintln!(
                        "yoinks: promised file failed: {}",
                        error.localizedDescription()
                    );
                    None
                }
                (None, None) => None,
            };
            model.store.borrow_mut().resolve_pending(entry_id, item);
            model.reload();
        });
        // SAFETY: The options dictionary may be empty; the reader runs on the
        // main queue, where the model lives.
        unsafe {
            receiver.receivePromisedFilesAtDestination_options_operationQueue_reader(
                &destination,
                &NSDictionary::new(),
                &NSOperationQueue::mainQueue(),
                &reader,
            );
        }
    }

    pub fn open(&self, id: Uuid) {
        let workspace = NSWorkspace::sharedWorkspace();
        for url in self.urls(id, true) {
            workspace.openURL(&url);
        }
    }

    pub fn reveal(&self, id: Uuid) {
        let urls = self.urls(id, false);
        if !urls.is_empty() {
            NSWorkspace::sharedWorkspace()
                .activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&urls));
        }
    }

    pub fn unstack(&self, id: Uuid) {
        if self.store.borrow_mut().unstack(id) {
            self.reload();
        }
    }

    pub fn remove(&self, id: Uuid) {
        let removed = self.store.borrow_mut().remove(id);
        if let Some(entry) = removed {
            self.delete_files(&[entry]);
            self.reload();
        }
    }

    /// Removes unpinned entries older than `max_age`.
    pub fn expire(&self, max_age: std::time::Duration) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let removed = self
            .store
            .borrow_mut()
            .expire(now.saturating_sub(max_age.as_secs()));
        if !removed.is_empty() {
            self.delete_files(&removed);
            self.reload();
        }
    }

    pub fn clear(&self) {
        let removed = self.store.borrow_mut().clear();
        self.delete_files(&removed);
        self.reload();
    }

    /// URLs for an entry's items. With `web_links`, link items yield their
    /// web URL instead of their `.webloc` file.
    fn urls(&self, id: Uuid, web_links: bool) -> Vec<Retained<NSURL>> {
        let store = self.store.borrow();
        let Some(entry) = store.entry(id) else {
            return Vec::new();
        };
        entry
            .items
            .iter()
            .filter_map(|item| match &item.kind {
                ItemKind::Pending => None,
                ItemKind::Url { url } if web_links => {
                    NSURL::URLWithString(&NSString::from_str(url))
                }
                _ => NSURL::from_file_path(&item.path),
            })
            .collect()
    }

    fn delete_files(&self, entries: &[Entry]) {
        for item in entries.iter().flat_map(|entry| &entry.items) {
            self.thumbnails.remove(item.id);
            if let Err(error) = self.items_dir.delete(item) {
                eprintln!("yoinks: could not delete {}: {error}", item.path.display());
            }
        }
    }

    /// Finds where an item's file is now, following its bookmark through
    /// renames and moves. An owned file that has left the items folder (moved
    /// out by a drag) now belongs to the user and will never be deleted.
    /// Returns `None` if the file is gone.
    fn locate(&self, mut item: Item) -> Option<Item> {
        if item.kind == ItemKind::Pending {
            return None;
        }
        if let Some(resolved) = item.bookmark.as_deref().and_then(bookmark::resolve) {
            if resolved.path != item.path {
                if item.kind == ItemKind::File {
                    item.title = file_name(&resolved.path);
                }
                item.path = resolved.path;
            }
            if resolved.stale {
                item.bookmark = bookmark::create(&item.path).or(item.bookmark);
            }
        }
        item.owned = item.owned && self.items_dir.owns(&item.path);
        item.path.exists().then_some(item)
    }

    fn reload(&self) {
        self.revision.set(self.revision.get() + 1);
        let store = self.store.borrow();
        self.view.reload(self, store.entries());
    }
}

/// Loads the saved shelf. A corrupt file is set aside rather than lost, and
/// the shelf starts empty.
fn load_store(path: &Path) -> ShelfStore {
    match ShelfStore::load(Box::new(JsonFile::new(path.to_path_buf()))) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("yoinks: could not read {}: {error}", path.display());
            let _ = fs::rename(path, path.with_extension("json.corrupt"));
            ShelfStore::empty(Box::new(JsonFile::new(path.to_path_buf())))
        }
    }
}

/// Size, kind and (for text) an excerpt of an item's file.
fn file_facts(item: &Item) -> FileFacts {
    if item.kind == ItemKind::Pending {
        return FileFacts::default();
    }
    let metadata = fs::metadata(&item.path).ok();
    let is_dir = metadata.as_ref().is_some_and(|m| m.is_dir());
    let size = metadata.filter(|m| m.is_file()).map(|m| m.len());
    let kind = if is_dir {
        Some("Folder".to_owned())
    } else {
        item.path
            .extension()
            .and_then(|ext| {
                UTType::typeWithFilenameExtension(&NSString::from_str(&ext.to_string_lossy()))
            })
            .and_then(|uti| uti.localizedDescription())
            .map(|kind| kind.to_string())
    };
    let excerpt = match item.kind {
        ItemKind::Text => fs::read_to_string(&item.path).ok(),
        ItemKind::RichText => fs::read(&item.path).ok().and_then(|bytes| {
            let data = objc2_foundation::NSData::from_vec(bytes);
            // SAFETY: Passing no document attributes out-parameter is allowed.
            let text = unsafe {
                objc2_foundation::NSAttributedString::initWithRTF_documentAttributes(
                    objc2_foundation::NSAttributedString::alloc(),
                    &data,
                    None,
                )
            };
            text.map(|text| text.string().to_string())
        }),
        _ => None,
    }
    .map(|text| describe::excerpt(&text));
    FileFacts {
        size,
        kind,
        excerpt,
    }
}

/// Images by kind or by file type (a dropped photo is a `File`).
fn is_image(item: &Item) -> bool {
    if item.kind == ItemKind::Image {
        return true;
    }
    if item.kind != ItemKind::File {
        return false;
    }
    let Some(ext) = item.path.extension() else {
        return false;
    };
    // SAFETY: Reading an immutable framework constant.
    let image = unsafe { objc2_uniform_type_identifiers::UTTypeImage };
    UTType::typeWithFilenameExtension(&NSString::from_str(&ext.to_string_lossy()))
        .is_some_and(|uti| uti.conformsToType(image))
}
