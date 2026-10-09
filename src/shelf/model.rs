//! `ShelfModel` ties the store, the items folder and the view together.

use std::cell::RefCell;
use std::fs;
use std::path::Path;
use std::ptr::NonNull;
use std::rc::{Rc, Weak};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{MainThreadMarker, NSArray, NSDictionary, NSError, NSOperationQueue, NSURL};
use uuid::Uuid;

use super::bookmark;
use super::item::{Entry, Item, ItemKind, file_name};
use super::materialize::ItemsDir;
use super::pasteboard::Dropped;
use super::store::{JsonFile, ShelfStore};
use super::view::ShelfContentView;

pub struct ShelfModel {
    store: RefCell<ShelfStore>,
    items_dir: ItemsDir,
    view: Retained<ShelfContentView>,
}

impl ShelfModel {
    pub fn new(mtm: MainThreadMarker, data_dir: &Path) -> Rc<Self> {
        let items_dir = ItemsDir::new(data_dir.join("Items"));
        items_dir.clean_incoming();
        let store = load_store(&data_dir.join("shelf.json"));
        let model = Rc::new_cyclic(|weak: &Weak<Self>| Self {
            store: RefCell::new(store),
            items_dir,
            view: ShelfContentView::new(mtm, weak.clone()),
        });
        model.store.borrow_mut().retain_map(revalidate);
        model.reload();
        model
    }

    pub fn view(&self) -> &ShelfContentView {
        &self.view
    }

    pub fn is_empty(&self) -> bool {
        self.store.borrow().is_empty()
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
                    NSURL::URLWithString(&objc2_foundation::NSString::from_str(url))
                }
                _ => NSURL::from_file_path(&item.path),
            })
            .collect()
    }

    fn delete_files(&self, entries: &[Entry]) {
        for item in entries.iter().flat_map(|entry| &entry.items) {
            if let Err(error) = self.items_dir.delete(item) {
                eprintln!("yoinks: could not delete {}: {error}", item.path.display());
            }
        }
    }

    fn reload(&self) {
        let store = self.store.borrow();
        self.view.reload(store.entries());
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

/// On launch: follow moved or renamed user files through their bookmarks and
/// drop items whose files are gone.
fn revalidate(mut item: Item) -> Option<Item> {
    if item.kind == ItemKind::Pending {
        return None;
    }
    if !item.owned
        && let Some(resolved) = item.bookmark.as_deref().and_then(bookmark::resolve)
    {
        if resolved.path != item.path {
            item.title = file_name(&resolved.path);
            item.path = resolved.path;
        }
        if resolved.stale {
            item.bookmark = bookmark::create(&item.path).or(item.bookmark);
        }
    }
    item.path.exists().then_some(item)
}
