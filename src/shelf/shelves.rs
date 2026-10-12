//! The list of named shelves and which one is showing, saved as
//! `shelves.json`. The first shelf ("Shelf") keeps `shelf.json`, so data from
//! before named shelves carries over; the others live in `Shelves/<id>.json`.
//! Plain Rust, so it can be unit tested.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The shelf that always exists and cannot be deleted.
pub const DEFAULT_ID: &str = "default";
const DEFAULT_NAME: &str = "Shelf";
/// Longest shelf name, in characters.
const MAX_NAME: usize = 40;

/// A color tag, as in Finder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TagColor {
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Purple,
    Gray,
}

impl TagColor {
    pub const ALL: [TagColor; 7] = [
        TagColor::Red,
        TagColor::Orange,
        TagColor::Yellow,
        TagColor::Green,
        TagColor::Blue,
        TagColor::Purple,
        TagColor::Gray,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TagColor::Red => "Red",
            TagColor::Orange => "Orange",
            TagColor::Yellow => "Yellow",
            TagColor::Green => "Green",
            TagColor::Blue => "Blue",
            TagColor::Purple => "Purple",
            TagColor::Gray => "Gray",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShelfInfo {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<TagColor>,
}

#[derive(Serialize, Deserialize)]
struct ShelvesFile {
    shelves: Vec<ShelfInfo>,
    current: String,
}

pub struct Shelves {
    data_dir: PathBuf,
    list: Vec<ShelfInfo>,
    current: String,
}

impl Shelves {
    /// Reads `shelves.json` from `data_dir`; a missing or unreadable file
    /// gives just the default shelf.
    pub fn load(data_dir: &Path) -> Self {
        let file: Option<ShelvesFile> = fs::read(data_dir.join("shelves.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let (mut list, current) = file.map_or_else(
            || (Vec::new(), DEFAULT_ID.to_owned()),
            |file| (file.shelves, file.current),
        );
        if !list.iter().any(|shelf| shelf.id == DEFAULT_ID) {
            list.insert(
                0,
                ShelfInfo {
                    id: DEFAULT_ID.to_owned(),
                    name: DEFAULT_NAME.to_owned(),
                    color: None,
                },
            );
        }
        let current = if list.iter().any(|shelf| shelf.id == current) {
            current
        } else {
            DEFAULT_ID.to_owned()
        };
        Self {
            data_dir: data_dir.to_path_buf(),
            list,
            current,
        }
    }

    pub fn list(&self) -> &[ShelfInfo] {
        &self.list
    }

    pub fn current(&self) -> &ShelfInfo {
        self.list
            .iter()
            .find(|shelf| shelf.id == self.current)
            .unwrap_or(&self.list[0])
    }

    /// Where a shelf's entries are saved.
    pub fn path(&self, id: &str) -> PathBuf {
        if id == DEFAULT_ID {
            self.data_dir.join("shelf.json")
        } else {
            self.data_dir.join("Shelves").join(format!("{id}.json"))
        }
    }

    /// Makes `id` the shelf that is showing. False if it does not exist.
    pub fn set_current(&mut self, id: &str) -> bool {
        if !self.list.iter().any(|shelf| shelf.id == id) {
            return false;
        }
        self.current = id.to_owned();
        self.save();
        true
    }

    /// Adds a shelf and returns its id.
    pub fn add(&mut self, name: &str) -> Result<String, String> {
        let name = self.check_name(name, None)?;
        let id = Uuid::new_v4().to_string();
        self.list.push(ShelfInfo {
            id: id.clone(),
            name,
            color: None,
        });
        self.save();
        Ok(id)
    }

    pub fn rename(&mut self, id: &str, name: &str) -> Result<(), String> {
        let name = self.check_name(name, Some(id))?;
        let shelf = self
            .list
            .iter_mut()
            .find(|shelf| shelf.id == id)
            .ok_or("No such shelf")?;
        shelf.name = name;
        self.save();
        Ok(())
    }

    /// Sets or clears a shelf's color tag. False if it does not exist.
    pub fn set_color(&mut self, id: &str, color: Option<TagColor>) -> bool {
        let Some(shelf) = self.list.iter_mut().find(|shelf| shelf.id == id) else {
            return false;
        };
        shelf.color = color;
        self.save();
        true
    }

    /// Removes a shelf from the list (its entries are the caller's to
    /// clean up). The default shelf cannot be removed; removing the current
    /// one makes the default current.
    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        if id == DEFAULT_ID {
            return Err(format!("\"{}\" can't be deleted", self.list[0].name));
        }
        let before = self.list.len();
        self.list.retain(|shelf| shelf.id != id);
        if self.list.len() == before {
            return Err("No such shelf".to_owned());
        }
        if self.current == id {
            self.current = DEFAULT_ID.to_owned();
        }
        let _ = fs::remove_file(self.path(id));
        self.save();
        Ok(())
    }

    /// A trimmed, unique name, or why it can't be used.
    fn check_name(&self, name: &str, except: Option<&str>) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Enter a name".to_owned());
        }
        if name.chars().count() > MAX_NAME {
            return Err(format!("Use at most {MAX_NAME} characters"));
        }
        let taken = self.list.iter().any(|shelf| {
            Some(shelf.id.as_str()) != except && shelf.name.eq_ignore_ascii_case(name)
        });
        if taken {
            return Err(format!("There is already a shelf called \"{name}\""));
        }
        Ok(name.to_owned())
    }

    fn save(&self) {
        let file = ShelvesFile {
            shelves: self.list.clone(),
            current: self.current.clone(),
        };
        let result = serde_json::to_vec_pretty(&file)
            .map_err(io::Error::from)
            .and_then(|bytes| {
                fs::create_dir_all(&self.data_dir)?;
                let path = self.data_dir.join("shelves.json");
                let tmp = path.with_extension("json.tmp");
                fs::write(&tmp, bytes)?;
                fs::rename(tmp, path)
            });
        if let Err(error) = result {
            eprintln!("yoinks: could not save shelves.json: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_with_the_default_shelf() {
        let tmp = tempfile::tempdir().unwrap();
        let shelves = Shelves::load(tmp.path());
        assert_eq!(shelves.list().len(), 1);
        assert_eq!(shelves.current().id, DEFAULT_ID);
        assert_eq!(shelves.current().name, "Shelf");
        assert_eq!(shelves.path(DEFAULT_ID), tmp.path().join("shelf.json"));
    }

    #[test]
    fn add_rename_switch_and_remove_are_saved() {
        let tmp = tempfile::tempdir().unwrap();
        let mut shelves = Shelves::load(tmp.path());
        let id = shelves.add("  Invoices ").unwrap();
        assert_eq!(shelves.list()[1].name, "Invoices", "trimmed");
        assert!(shelves.set_current(&id));
        assert!(shelves.path(&id).starts_with(tmp.path().join("Shelves")));

        let reloaded = Shelves::load(tmp.path());
        assert_eq!(reloaded.current().name, "Invoices");

        shelves.rename(&id, "Receipts").unwrap();
        assert_eq!(Shelves::load(tmp.path()).current().name, "Receipts");

        fs::create_dir_all(tmp.path().join("Shelves")).unwrap();
        fs::write(shelves.path(&id), b"{}").unwrap();
        shelves.remove(&id).unwrap();
        assert_eq!(shelves.current().id, DEFAULT_ID, "back to the default");
        assert!(!shelves.path(&id).exists(), "its file is gone");
        assert_eq!(Shelves::load(tmp.path()).list().len(), 1);
    }

    #[test]
    fn names_must_be_unique_and_not_blank() {
        let tmp = tempfile::tempdir().unwrap();
        let mut shelves = Shelves::load(tmp.path());
        let id = shelves.add("Design").unwrap();
        assert!(shelves.add("design").is_err(), "case-insensitive");
        assert!(shelves.add("   ").is_err());
        assert!(shelves.add(&"x".repeat(41)).is_err());
        assert!(
            shelves.rename(&id, "Design").is_ok(),
            "its own name is fine"
        );
        assert!(shelves.rename(&id, "Shelf").is_err());
    }

    #[test]
    fn color_tags_are_saved() {
        let tmp = tempfile::tempdir().unwrap();
        let mut shelves = Shelves::load(tmp.path());
        let id = shelves.add("Design").unwrap();
        assert!(shelves.set_color(&id, Some(TagColor::Purple)));
        assert!(!shelves.set_color("nope", Some(TagColor::Red)));
        let reloaded = Shelves::load(tmp.path());
        assert_eq!(reloaded.list()[1].color, Some(TagColor::Purple));
        assert_eq!(reloaded.list()[0].color, None);
        let written = fs::read_to_string(tmp.path().join("shelves.json")).unwrap();
        assert!(written.contains("\"purple\""), "{written}");
        shelves.set_color(&id, None);
        assert_eq!(Shelves::load(tmp.path()).list()[1].color, None);
    }

    #[test]
    fn the_default_shelf_stays() {
        let tmp = tempfile::tempdir().unwrap();
        let mut shelves = Shelves::load(tmp.path());
        assert!(shelves.remove(DEFAULT_ID).is_err());
        assert!(!shelves.set_current("nope"));
        shelves.rename(DEFAULT_ID, "Inbox").unwrap();
        assert_eq!(Shelves::load(tmp.path()).current().name, "Inbox");
    }

    #[test]
    fn a_broken_file_falls_back_to_the_default() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("shelves.json"), b"{ nope").unwrap();
        assert_eq!(Shelves::load(tmp.path()).current().id, DEFAULT_ID);
    }
}
