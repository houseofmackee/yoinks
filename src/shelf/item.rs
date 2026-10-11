//! The shelf data model. Plain Rust, no AppKit, so it can be unit tested and
//! serialized.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One tile on the shelf. A drop of several items creates a single entry
/// holding all of them: a stack.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: Uuid,
    /// Unix timestamps in seconds. Stable ids and timestamps keep the door
    /// open for syncing shelves between Macs later.
    pub added_at: u64,
    pub modified_at: u64,
    /// Pinned entries stay on the shelf after being dragged out and are left
    /// alone by Clear Shelf.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    pub items: Vec<Item>,
}

impl Entry {
    pub fn new(items: Vec<Item>) -> Self {
        let now = unix_now();
        Self {
            id: Uuid::new_v4(),
            added_at: now,
            modified_at: now,
            pinned: false,
            items,
        }
    }

    pub fn is_stack(&self) -> bool {
        self.items.len() > 1
    }

    pub fn has_pending(&self) -> bool {
        self.items.iter().any(|item| item.kind == ItemKind::Pending)
    }

    pub fn title(&self) -> String {
        match self.items.as_slice() {
            [item] => item.title.clone(),
            items => format!("{} items", items.len()),
        }
    }

    pub(super) fn touch(&mut self) {
        self.modified_at = unix_now();
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: Uuid,
    pub kind: ItemKind,
    /// Short human-readable name shown under the tile.
    pub title: String,
    /// The file backing this item. For `File` items that were dropped from
    /// Finder this is the user's own file; for everything else it is a copy
    /// inside the Yoinks items folder (see `owned`).
    pub path: PathBuf,
    /// True when Yoinks created the file and may delete it when the item is
    /// removed. User files are never deleted.
    pub owned: bool,
    /// Bookmark data for user files, so the item follows renames and moves.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "base64_bytes"
    )]
    pub bookmark: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ItemKind {
    File,
    Text,
    RichText,
    Image,
    Url {
        url: String,
    },
    /// A promised file (Mail attachment, Photos) that has not arrived yet.
    /// Never persisted.
    Pending,
}

impl Item {
    /// A reference to a file the user owns.
    pub fn user_file(path: PathBuf, bookmark: Option<Vec<u8>>) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind: ItemKind::File,
            title: file_name(&path),
            path,
            owned: false,
            bookmark,
        }
    }

    pub fn pending() -> Self {
        Self {
            id: Uuid::new_v4(),
            kind: ItemKind::Pending,
            title: "Receiving…".to_owned(),
            path: PathBuf::new(),
            owned: false,
            bookmark: None,
        }
    }
}

pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Serializes bookmark bytes as base64 instead of a JSON number array.
mod base64_bytes {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match bytes {
            Some(bytes) => s.serialize_some(&STANDARD.encode(bytes)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        Option::<String>::deserialize(d)?
            .map(|text| STANDARD.decode(text).map_err(serde::de::Error::custom))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_title_for_single_item_and_stack() {
        let one = Entry::new(vec![Item::user_file("/tmp/report.pdf".into(), None)]);
        assert_eq!(one.title(), "report.pdf");
        assert!(!one.is_stack());

        let stack = Entry::new(vec![Item::pending(), Item::pending(), Item::pending()]);
        assert_eq!(stack.title(), "3 items");
        assert!(stack.is_stack());
        assert!(stack.has_pending());
    }

    #[test]
    fn json_round_trip_with_bookmark() {
        let mut item = Item::user_file("/tmp/a.txt".into(), Some(vec![0, 1, 2, 255]));
        item.kind = ItemKind::Url {
            url: "https://example.com".into(),
        };
        let entry = Entry::new(vec![item]);
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"bookmark\":\"AAEC/w==\""));
        assert!(json.contains("\"type\":\"url\""));
        let back: Entry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, entry);
    }

    #[test]
    fn missing_bookmark_is_omitted() {
        let entry = Entry::new(vec![Item::user_file("/tmp/a.txt".into(), None)]);
        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.contains("bookmark"));
        let back: Entry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.items[0].bookmark, None);
    }
}
