//! `ShelfStore`: the ordered list of shelf entries and its persistence.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::item::{Entry, Item, ItemKind};

/// Where the shelf is saved. A trait so a sync backend can be added later
/// without touching the model.
pub trait Persistence {
    fn load(&self) -> io::Result<Vec<Entry>>;
    fn save(&self, entries: &[Entry]) -> io::Result<()>;
}

const FORMAT_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct ShelfFile {
    version: u32,
    entries: Vec<Entry>,
}

/// Saves the shelf as JSON, writing to a temporary file and renaming it so a
/// crash never leaves a half-written file behind.
pub struct JsonFile {
    path: PathBuf,
}

impl JsonFile {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Persistence for JsonFile {
    fn load(&self) -> io::Result<Vec<Entry>> {
        match fs::read(&self.path) {
            Ok(bytes) => {
                let file: ShelfFile = serde_json::from_slice(&bytes)?;
                Ok(file.entries)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }

    fn save(&self, entries: &[Entry]) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = ShelfFile {
            version: FORMAT_VERSION,
            entries: entries.to_vec(),
        };
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&file)?)?;
        fs::rename(tmp, &self.path)
    }
}

/// Ordered shelf entries, newest first. Every mutation is saved immediately.
pub struct ShelfStore {
    entries: Vec<Entry>,
    persistence: Box<dyn Persistence>,
}

impl ShelfStore {
    pub fn load(persistence: Box<dyn Persistence>) -> io::Result<Self> {
        let entries = persistence.load()?;
        Ok(Self {
            entries,
            persistence,
        })
    }

    /// An empty shelf, for when the saved one cannot be read.
    pub fn empty(persistence: Box<dyn Persistence>) -> Self {
        Self {
            entries: Vec::new(),
            persistence,
        }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn entry(&self, id: Uuid) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn insert(&mut self, entry: Entry) {
        self.entries.insert(0, entry);
        self.save();
    }

    pub fn remove(&mut self, id: Uuid) -> Option<Entry> {
        let index = self.entries.iter().position(|entry| entry.id == id)?;
        let entry = self.entries.remove(index);
        self.save();
        Some(entry)
    }

    pub fn clear(&mut self) -> Vec<Entry> {
        let entries = std::mem::take(&mut self.entries);
        self.save();
        entries
    }

    /// Applies `f` to an entry and saves. Returns false if it does not exist.
    pub fn update(&mut self, id: Uuid, f: impl FnOnce(&mut Entry)) -> bool {
        let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) else {
            return false;
        };
        f(entry);
        entry.touch();
        self.save();
        true
    }

    /// Splits a stack into one entry per item, in place. Returns false if the
    /// entry does not exist, is not a stack, or is still receiving files.
    pub fn unstack(&mut self, id: Uuid) -> bool {
        let Some(index) = self.entries.iter().position(|entry| entry.id == id) else {
            return false;
        };
        let entry = &self.entries[index];
        if !entry.is_stack() || entry.has_pending() {
            return false;
        }
        let added_at = entry.added_at;
        let singles: Vec<Entry> = self.entries[index]
            .items
            .iter()
            .map(|item| {
                let mut single = Entry::new(vec![item.clone()]);
                single.added_at = added_at;
                single
            })
            .collect();
        self.entries.splice(index..=index, singles);
        self.save();
        true
    }

    /// Replaces the first pending item of `entry_id` with `resolved`, or
    /// drops it when the promised file failed to arrive. When nothing is
    /// pending (a promise delivered more than one file), `resolved` is
    /// appended instead. Entries left empty are removed.
    pub fn resolve_pending(&mut self, entry_id: Uuid, resolved: Option<Item>) {
        let Some(index) = self.entries.iter().position(|entry| entry.id == entry_id) else {
            return;
        };
        let entry = &mut self.entries[index];
        let slot = entry
            .items
            .iter()
            .position(|item| item.kind == ItemKind::Pending);
        match (slot, resolved) {
            (Some(slot), Some(item)) => entry.items[slot] = item,
            (Some(slot), None) => {
                entry.items.remove(slot);
            }
            (None, Some(item)) => entry.items.push(item),
            (None, None) => return,
        }
        entry.touch();
        if entry.items.is_empty() {
            self.entries.remove(index);
        }
        self.save();
    }

    /// Maps every item through `f`, dropping items for which it returns
    /// `None` and entries left empty. Used to revalidate files on launch.
    pub fn retain_map(&mut self, mut f: impl FnMut(Item) -> Option<Item>) {
        for entry in &mut self.entries {
            entry.items = std::mem::take(&mut entry.items)
                .into_iter()
                .filter_map(&mut f)
                .collect();
        }
        self.entries.retain(|entry| !entry.items.is_empty());
        self.save();
    }

    /// Persists everything except items still being received.
    fn save(&self) {
        let entries: Vec<Entry> = self
            .entries
            .iter()
            .filter_map(|entry| {
                let items: Vec<Item> = entry
                    .items
                    .iter()
                    .filter(|item| item.kind != ItemKind::Pending)
                    .cloned()
                    .collect();
                (!items.is_empty()).then(|| Entry {
                    items,
                    ..entry.clone()
                })
            })
            .collect();
        if let Err(error) = self.persistence.save(&entries) {
            eprintln!("yoinks: failed to save shelf: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    /// In-memory persistence that records the last save.
    #[derive(Clone, Default)]
    struct Memory(Rc<RefCell<Vec<Entry>>>);

    impl Persistence for Memory {
        fn load(&self) -> io::Result<Vec<Entry>> {
            Ok(self.0.borrow().clone())
        }
        fn save(&self, entries: &[Entry]) -> io::Result<()> {
            *self.0.borrow_mut() = entries.to_vec();
            Ok(())
        }
    }

    fn file(name: &str) -> Item {
        Item::user_file(PathBuf::from("/tmp").join(name), None)
    }

    fn store() -> (ShelfStore, Memory) {
        let memory = Memory::default();
        let store = ShelfStore::load(Box::new(memory.clone())).unwrap();
        (store, memory)
    }

    #[test]
    fn insert_puts_newest_first_and_saves() {
        let (mut store, saved) = store();
        store.insert(Entry::new(vec![file("a")]));
        store.insert(Entry::new(vec![file("b")]));
        let titles: Vec<_> = store.entries().iter().map(Entry::title).collect();
        assert_eq!(titles, ["b", "a"]);
        assert_eq!(saved.0.borrow().len(), 2);
    }

    #[test]
    fn remove_and_clear() {
        let (mut store, saved) = store();
        let entry = Entry::new(vec![file("a")]);
        let id = entry.id;
        store.insert(entry);
        store.insert(Entry::new(vec![file("b")]));
        assert_eq!(store.remove(id).unwrap().title(), "a");
        assert!(store.remove(id).is_none());
        assert_eq!(store.clear().len(), 1);
        assert!(store.is_empty());
        assert!(saved.0.borrow().is_empty());
    }

    #[test]
    fn unstack_splits_in_place() {
        let (mut store, _) = store();
        store.insert(Entry::new(vec![file("old")]));
        let stack = Entry::new(vec![file("x"), file("y")]);
        let id = stack.id;
        store.insert(stack);
        store.insert(Entry::new(vec![file("new")]));

        assert!(store.unstack(id));
        let titles: Vec<_> = store.entries().iter().map(Entry::title).collect();
        assert_eq!(titles, ["new", "x", "y", "old"]);
        assert!(!store.unstack(id), "the stack no longer exists");
    }

    #[test]
    fn single_items_and_pending_stacks_do_not_unstack() {
        let (mut store, _) = store();
        let single = Entry::new(vec![file("a")]);
        let pending = Entry::new(vec![file("b"), Item::pending()]);
        let (single_id, pending_id) = (single.id, pending.id);
        store.insert(single);
        store.insert(pending);
        assert!(!store.unstack(single_id));
        assert!(!store.unstack(pending_id));
    }

    #[test]
    fn pending_items_are_not_saved_until_resolved() {
        let (mut store, saved) = store();
        let entry = Entry::new(vec![Item::pending(), Item::pending()]);
        let id = entry.id;
        store.insert(entry);
        assert!(saved.0.borrow().is_empty());

        store.resolve_pending(id, Some(file("mail.pdf")));
        assert_eq!(saved.0.borrow()[0].items.len(), 1);
        assert_eq!(store.entry(id).unwrap().items.len(), 2);

        store.resolve_pending(id, None);
        assert_eq!(store.entry(id).unwrap().title(), "mail.pdf");
        assert!(!store.entry(id).unwrap().has_pending());
    }

    #[test]
    fn extra_promised_files_are_appended() {
        let (mut store, _) = store();
        let entry = Entry::new(vec![Item::pending()]);
        let id = entry.id;
        store.insert(entry);
        store.resolve_pending(id, Some(file("one")));
        store.resolve_pending(id, Some(file("two")));
        assert_eq!(store.entry(id).unwrap().title(), "2 items");
        store.resolve_pending(id, None);
        assert_eq!(
            store.entry(id).unwrap().items.len(),
            2,
            "nothing left to drop"
        );
    }

    #[test]
    fn failed_promise_removes_empty_entry() {
        let (mut store, _) = store();
        let entry = Entry::new(vec![Item::pending()]);
        let id = entry.id;
        store.insert(entry);
        store.resolve_pending(id, None);
        assert!(store.is_empty());
    }

    #[test]
    fn retain_map_drops_items_and_empty_entries() {
        let (mut store, _) = store();
        store.insert(Entry::new(vec![file("gone")]));
        store.insert(Entry::new(vec![file("keep"), file("gone")]));
        store.retain_map(|item| (item.title != "gone").then_some(item));
        assert_eq!(store.entries().len(), 1);
        assert_eq!(store.entries()[0].title(), "keep");
    }

    #[test]
    fn json_file_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/shelf.json");
        let json = JsonFile::new(path.clone());
        assert!(
            json.load().unwrap().is_empty(),
            "missing file loads as empty"
        );

        let entries = vec![Entry::new(vec![file("a"), file("b")])];
        json.save(&entries).unwrap();
        assert_eq!(json.load().unwrap(), entries);
        assert!(!path.with_extension("json.tmp").exists());
    }
}
