//! `ShelfModel` ties the store, the items folder and the view together.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::rc::{Rc, Weak};

use block2::RcBlock;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{NSAttributedStringAppKitDocumentFormats, NSPasteboard, NSWorkspace};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSDictionary, NSError, NSOperationQueue, NSString, NSURL,
};
use objc2_uniform_type_identifiers::UTType;
use uuid::Uuid;

use super::bookmark;
use super::describe::{self, FileFacts};
use super::item::{Entry, Item, ItemKind, file_name};
use super::materialize::ItemsDir;
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
        if self.keep_after_drag_out.get() {
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

    /// Adds whatever is on the clipboard (files, text, links, images) to the
    /// shelf. Returns false if there was nothing usable.
    pub fn accept_clipboard(self: &Rc<Self>) -> bool {
        self.accept(pasteboard::read(&NSPasteboard::generalPasteboard()))
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
