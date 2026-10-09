//! The clipboard history model: newest first, deduplicated, bounded,
//! searchable, with pinned clips that are never evicted. Plain Rust; large
//! payloads (images, rich text) live in files the caller manages.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::shelf::{file_name, text_title, unix_now};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: Uuid,
    /// Unix seconds.
    pub copied_at: u64,
    /// The app that was frontmost when the copy happened.
    #[serde(default)]
    pub source: Option<Source>,
    #[serde(default)]
    pub pinned: bool,
    pub content: ClipContent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub bundle_id: Option<String>,
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClipContent {
    Text {
        text: String,
    },
    /// The RTF data is stored in `file`; `plain` is for display and search.
    RichText {
        plain: String,
        file: PathBuf,
    },
    Image {
        file: PathBuf,
        /// Content hash, to recognize the same image copied again.
        digest: u64,
    },
    Files {
        paths: Vec<PathBuf>,
    },
    Url {
        url: String,
        title: Option<String>,
    },
}

impl ClipContent {
    /// A one-line label.
    pub fn title(&self) -> String {
        match self {
            Self::Text { text } | Self::RichText { plain: text, .. } => {
                let title = text_title(text);
                if title.is_empty() {
                    "(whitespace)".to_owned()
                } else {
                    title
                }
            }
            Self::Image { .. } => "Image".to_owned(),
            Self::Files { paths } => match paths.as_slice() {
                [path] => file_name(path),
                paths => format!("{} files", paths.len()),
            },
            Self::Url { url, title } => title.clone().unwrap_or_else(|| url.clone()),
        }
    }

    /// Payload file owned by this clip, if any.
    pub fn file(&self) -> Option<&Path> {
        match self {
            Self::RichText { file, .. } | Self::Image { file, .. } => Some(file),
            _ => None,
        }
    }

    /// True if copying `other` again should replace this clip rather than
    /// add a duplicate.
    fn same_as(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Text { text: a }, Self::Text { text: b }) => a == b,
            (Self::RichText { plain: a, .. }, Self::RichText { plain: b, .. }) => a == b,
            (Self::Image { digest: a, .. }, Self::Image { digest: b, .. }) => a == b,
            (Self::Files { paths: a }, Self::Files { paths: b }) => a == b,
            (Self::Url { url: a, .. }, Self::Url { url: b, .. }) => a == b,
            _ => false,
        }
    }

    /// Case-insensitive search over the text, file names and links.
    fn matches(&self, needle: &str) -> bool {
        let haystack = match self {
            Self::Text { text } | Self::RichText { plain: text, .. } => text.clone(),
            Self::Image { .. } => "image".to_owned(),
            Self::Files { paths } => paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("\n"),
            Self::Url { url, title } => format!("{url}\n{}", title.as_deref().unwrap_or("")),
        };
        haystack.to_lowercase().contains(needle)
    }
}

impl Clip {
    pub fn new(content: ClipContent, source: Option<Source>) -> Self {
        Self {
            id: Uuid::new_v4(),
            copied_at: unix_now(),
            source,
            pinned: false,
            content,
        }
    }

    pub fn matches(&self, query: &str) -> bool {
        let needle = query.trim().to_lowercase();
        needle.is_empty()
            || self.content.matches(&needle)
            || self
                .source
                .as_ref()
                .and_then(|s| s.name.as_deref())
                .is_some_and(|name| name.to_lowercase().contains(&needle))
    }
}

#[derive(Serialize, Deserialize)]
struct HistoryFile {
    version: u32,
    clips: Vec<Clip>,
}

pub struct History {
    clips: Vec<Clip>,
    /// Maximum number of unpinned clips.
    limit: usize,
    path: Option<PathBuf>,
}

impl History {
    /// Loads `path` (missing or unreadable files start an empty history).
    pub fn load(path: PathBuf, limit: usize) -> Self {
        let clips = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<HistoryFile>(&bytes).ok())
            .map(|file| file.clips)
            .unwrap_or_default();
        Self {
            clips,
            limit,
            path: Some(path),
        }
    }

    #[cfg(test)]
    fn in_memory(limit: usize) -> Self {
        Self {
            clips: Vec::new(),
            limit,
            path: None,
        }
    }

    /// Changes the maximum number of unpinned clips, returning any dropped.
    #[must_use]
    pub fn set_limit(&mut self, limit: usize) -> Vec<Clip> {
        self.limit = limit;
        let removed = self.trim();
        if !removed.is_empty() {
            self.save();
        }
        removed
    }

    pub fn clips(&self) -> &[Clip] {
        &self.clips
    }

    pub fn get(&self, id: Uuid) -> Option<&Clip> {
        self.clips.iter().find(|clip| clip.id == id)
    }

    pub fn search(&self, query: &str) -> Vec<Clip> {
        self.clips
            .iter()
            .filter(|clip| clip.matches(query))
            .cloned()
            .collect()
    }

    /// Adds `clip` on top. A clip with the same content is replaced (its pin
    /// carries over) and the oldest unpinned clips beyond the limit are
    /// dropped. Returns the clips that left, so their files can be deleted.
    #[must_use]
    pub fn push(&mut self, mut clip: Clip) -> Vec<Clip> {
        let mut removed = Vec::new();
        if let Some(index) = self
            .clips
            .iter()
            .position(|c| c.content.same_as(&clip.content))
        {
            let old = self.clips.remove(index);
            clip.pinned |= old.pinned;
            removed.push(old);
        }
        self.clips.insert(0, clip);
        removed.extend(self.trim());
        self.save();
        removed
    }

    /// Moves a clip to the top (it was just copied again).
    pub fn touch(&mut self, id: Uuid) {
        if let Some(index) = self.clips.iter().position(|clip| clip.id == id) {
            let mut clip = self.clips.remove(index);
            clip.copied_at = unix_now();
            self.clips.insert(0, clip);
            self.save();
        }
    }

    pub fn toggle_pin(&mut self, id: Uuid) -> Vec<Clip> {
        if let Some(clip) = self.clips.iter_mut().find(|clip| clip.id == id) {
            clip.pinned = !clip.pinned;
        }
        let removed = self.trim();
        self.save();
        removed
    }

    pub fn remove(&mut self, id: Uuid) -> Option<Clip> {
        let index = self.clips.iter().position(|clip| clip.id == id)?;
        let clip = self.clips.remove(index);
        self.save();
        Some(clip)
    }

    /// Removes every unpinned clip.
    pub fn clear(&mut self) -> Vec<Clip> {
        let (pinned, removed) = std::mem::take(&mut self.clips)
            .into_iter()
            .partition(|clip| clip.pinned);
        self.clips = pinned;
        self.save();
        removed
    }

    /// Drops the oldest unpinned clips beyond the limit.
    fn trim(&mut self) -> Vec<Clip> {
        let mut removed = Vec::new();
        while self.clips.iter().filter(|clip| !clip.pinned).count() > self.limit {
            let Some(index) = self.clips.iter().rposition(|clip| !clip.pinned) else {
                break;
            };
            removed.push(self.clips.remove(index));
        }
        removed
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        let file = HistoryFile {
            version: 1,
            clips: self.clips.clone(),
        };
        let result = (|| -> io::Result<()> {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let tmp = path.with_extension("json.tmp");
            fs::write(&tmp, serde_json::to_vec_pretty(&file)?)?;
            fs::rename(tmp, path)
        })();
        if let Err(error) = result {
            eprintln!("yoinks: failed to save clipboard history: {error}");
        }
    }
}

/// "just now", "5 min ago", "3 h ago", "yesterday", "4 days ago".
pub fn age(now: u64, then: u64) -> String {
    let seconds = now.saturating_sub(then);
    match seconds {
        0..60 => "just now".to_owned(),
        60..3_600 => format!("{} min ago", seconds / 60),
        3_600..86_400 => format!("{} h ago", seconds / 3_600),
        86_400..172_800 => "yesterday".to_owned(),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Clip {
        Clip::new(ClipContent::Text { text: s.into() }, None)
    }

    fn titles(history: &History) -> Vec<String> {
        history.clips().iter().map(|c| c.content.title()).collect()
    }

    #[test]
    fn newest_first_and_duplicates_move_to_top() {
        let mut h = History::in_memory(10);
        assert!(h.push(text("a")).is_empty());
        assert!(h.push(text("b")).is_empty());
        let removed = h.push(text("a"));
        assert_eq!(removed.len(), 1, "the old copy of a is returned");
        assert_eq!(titles(&h), ["a", "b"]);
    }

    #[test]
    fn limit_evicts_oldest_unpinned_only() {
        let mut h = History::in_memory(2);
        let _ = h.push(text("old pinned"));
        let pinned = h.clips()[0].id;
        let _ = h.toggle_pin(pinned);
        let _ = h.push(text("x"));
        let _ = h.push(text("y"));
        let removed = h.push(text("z"));
        assert_eq!(
            removed
                .iter()
                .map(|c| c.content.title())
                .collect::<Vec<_>>(),
            ["x"]
        );
        assert_eq!(titles(&h), ["z", "y", "old pinned"]);
    }

    #[test]
    fn pin_survives_recopy_and_clear() {
        let mut h = History::in_memory(10);
        let _ = h.push(text("keep"));
        let id = h.clips()[0].id;
        let _ = h.toggle_pin(id);
        let _ = h.push(text("keep"));
        assert!(h.clips()[0].pinned, "re-copying keeps the pin");
        let _ = h.push(text("temp"));
        assert_eq!(h.clear().len(), 1);
        assert_eq!(titles(&h), ["keep"]);
    }

    #[test]
    fn unpinning_over_the_limit_trims() {
        let mut h = History::in_memory(1);
        let _ = h.push(text("a"));
        let a = h.clips()[0].id;
        let _ = h.toggle_pin(a);
        let _ = h.push(text("b"));
        assert_eq!(
            h.toggle_pin(a).len(),
            1,
            "now two unpinned with a limit of one"
        );
    }

    #[test]
    fn lowering_the_limit_trims_oldest() {
        let mut h = History::in_memory(10);
        for s in ["a", "b", "c"] {
            let _ = h.push(text(s));
        }
        let removed = h.set_limit(1);
        assert_eq!(removed.len(), 2);
        assert_eq!(titles(&h), ["c"]);
    }

    #[test]
    fn touch_remove_and_search() {
        let mut h = History::in_memory(10);
        let _ = h.push(text("Hello World"));
        let _ = h.push(Clip::new(
            ClipContent::Url {
                url: "https://example.com".into(),
                title: Some("Example".into()),
            },
            Some(Source {
                bundle_id: Some("com.apple.Safari".into()),
                name: Some("Safari".into()),
            }),
        ));
        let hello = h.clips()[1].id;
        h.touch(hello);
        assert_eq!(titles(&h), ["Hello World", "Example"]);
        assert_eq!(h.search("world").len(), 1);
        assert_eq!(h.search("EXAMPLE.COM").len(), 1);
        assert_eq!(h.search("safari").len(), 1, "matches the source app");
        assert_eq!(h.search("  ").len(), 2);
        assert!(h.remove(hello).is_some());
        assert!(h.get(hello).is_none());
    }

    #[test]
    fn titles_for_each_kind() {
        let files = ClipContent::Files {
            paths: vec!["/a/x.pdf".into(), "/a/y.pdf".into()],
        };
        assert_eq!(files.title(), "2 files");
        assert_eq!(
            ClipContent::Text {
                text: "  \n ".into()
            }
            .title(),
            "(whitespace)"
        );
        let image = ClipContent::Image {
            file: "/c/i.png".into(),
            digest: 1,
        };
        assert_eq!(image.title(), "Image");
        assert_eq!(image.file(), Some(Path::new("/c/i.png")));
    }

    #[test]
    fn persists_and_reloads() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("history.json");
        let mut h = History::load(path.clone(), 5);
        let _ = h.push(text("saved"));
        let reloaded = History::load(path, 5);
        assert_eq!(titles(&reloaded), ["saved"]);
    }

    #[test]
    fn ages() {
        assert_eq!(age(100, 100), "just now");
        assert_eq!(age(1_000, 100), "15 min ago");
        assert_eq!(age(10_000, 0), "2 h ago");
        assert_eq!(age(100_000, 0), "yesterday");
        assert_eq!(age(400_000, 0), "4 days ago");
        assert_eq!(age(0, 100), "just now", "clock skew");
    }
}
