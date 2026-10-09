//! Turns dropped non-file content (text, images, links) into real files inside
//! the Yoinks items folder, so every shelf item can later be dragged out as a
//! file. Also owns cleanup of those files.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::item::{Item, ItemKind, file_name};

/// Dropped content that is not already a file.
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Text(String),
    RichText {
        rtf: Vec<u8>,
        plain: String,
    },
    Image {
        data: Vec<u8>,
        extension: &'static str,
    },
    Url {
        url: String,
        title: Option<String>,
    },
}

const MAX_TITLE_CHARS: usize = 40;
const INCOMING: &str = "Incoming";

/// The folder holding files Yoinks owns. Each item gets its own subfolder
/// named after its id, so names never collide and removal is one
/// `remove_dir_all`.
pub struct ItemsDir {
    root: PathBuf,
}

impl ItemsDir {
    /// The root is created and canonicalized, so paths that come back from
    /// bookmarks (which resolve symlinks such as `/tmp` to `/private/tmp`)
    /// still compare equal to it.
    pub fn new(root: PathBuf) -> Self {
        let root = fs::create_dir_all(&root)
            .and_then(|()| fs::canonicalize(&root))
            .unwrap_or(root);
        Self { root }
    }

    pub fn materialize(&self, content: Content) -> io::Result<Item> {
        let id = Uuid::new_v4();
        let (kind, title, file_name, bytes) = match content {
            Content::Text(text) => {
                let title = text_title(&text);
                let name = format!("{}.txt", safe_file_stem(&title, "Text"));
                (ItemKind::Text, title, name, text.into_bytes())
            }
            Content::RichText { rtf, plain } => {
                let title = text_title(&plain);
                let name = format!("{}.rtf", safe_file_stem(&title, "Text"));
                (ItemKind::RichText, title, name, rtf)
            }
            Content::Image { data, extension } => {
                let name = format!("Image.{extension}");
                (ItemKind::Image, name.clone(), name, data)
            }
            Content::Url { url, title } => {
                let title = title
                    .map(|title| text_title(&title))
                    .filter(|title| !title.is_empty())
                    .unwrap_or_else(|| url_title(&url));
                let name = format!("{}.webloc", safe_file_stem(&title, "Link"));
                let bytes = webloc(&url).into_bytes();
                (ItemKind::Url { url }, title, name, bytes)
            }
        };

        let dir = self.root.join(id.to_string());
        fs::create_dir_all(&dir)?;
        let path = dir.join(file_name);
        fs::write(&path, bytes)?;
        Ok(Item {
            id,
            kind,
            title,
            path,
            owned: true,
            bookmark: None,
        })
    }

    /// A fresh, empty folder for a promised file to be written into.
    pub fn incoming_dir(&self) -> io::Result<PathBuf> {
        let dir = self.root.join(INCOMING).join(Uuid::new_v4().to_string());
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    /// Moves a received promised file into its own item folder.
    pub fn adopt(&self, received: &Path) -> io::Result<Item> {
        let id = Uuid::new_v4();
        let dir = self.root.join(id.to_string());
        fs::create_dir_all(&dir)?;
        let path = dir.join(received.file_name().unwrap_or("Untitled".as_ref()));
        fs::rename(received, &path)?;
        if let Some(parent) = received.parent() {
            // Only succeeds once the incoming folder is empty.
            let _ = fs::remove_dir(parent);
        }
        Ok(Item {
            id,
            kind: ItemKind::File,
            title: file_name(&path),
            path,
            owned: true,
            bookmark: None,
        })
    }

    /// True if `path` sits directly inside an item folder
    /// (`<root>/<item-id>/<file>`), i.e. Yoinks created it and it has not been
    /// moved out.
    pub fn owns(&self, path: &Path) -> bool {
        let Some(dir) = path.parent() else {
            return false;
        };
        let Some(parent) = dir.parent() else {
            return false;
        };
        let in_root =
            parent == self.root || fs::canonicalize(parent).is_ok_and(|parent| parent == self.root);
        in_root
            && dir
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| Uuid::parse_str(name).is_ok())
    }

    /// Deletes the folder of an owned item. User files are never touched, and
    /// nothing outside the items folder is ever deleted.
    pub fn delete(&self, item: &Item) -> io::Result<()> {
        if !item.owned || !self.owns(&item.path) {
            return Ok(());
        }
        let dir = item.path.parent().expect("owned paths have a parent");
        match fs::remove_dir_all(dir) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }

    /// Deletes item folders not referenced by `live` item ids: leftovers from
    /// dragged-out items still recallable when the app quit, or from a crash.
    pub fn remove_orphans(&self, live: &HashSet<Uuid>) {
        let Ok(dirs) = fs::read_dir(&self.root) else {
            return;
        };
        for dir in dirs.flatten() {
            let name = dir.file_name();
            let Some(id) = name.to_str().and_then(|name| Uuid::parse_str(name).ok()) else {
                continue;
            };
            if !live.contains(&id) && dir.path().is_dir() {
                let _ = fs::remove_dir_all(dir.path());
            }
        }
    }

    /// Removes leftovers from transfers interrupted by a quit or crash.
    pub fn clean_incoming(&self) {
        let _ = fs::remove_dir_all(self.root.join(INCOMING));
    }
}

/// First line of the text, whitespace collapsed, shortened to a tile label.
pub fn text_title(text: &str) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > MAX_TITLE_CHARS {
        let short: String = collapsed.chars().take(MAX_TITLE_CHARS - 1).collect();
        format!("{}…", short.trim_end())
    } else {
        collapsed
    }
}

/// Host plus first path segment, e.g. `example.com/docs`.
fn url_title(url: &str) -> String {
    let without_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let mut parts = without_scheme
        .split(['/', '?', '#'])
        .filter(|part| !part.is_empty());
    match (parts.next(), parts.next()) {
        (Some(host), Some(segment)) => format!("{host}/{segment}"),
        (Some(host), None) => host.to_owned(),
        _ => "Link".to_owned(),
    }
}

/// Makes `title` safe as a file name on APFS and in Finder.
fn safe_file_stem(title: &str, fallback: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            '/' | ':' | '\\' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.').trim();
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// A Finder `.webloc` file: double-clicking it opens the URL.
fn webloc(url: &str) -> String {
    let escaped = url
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>URL</key>
	<string>{escaped}</string>
</dict>
</plist>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_titles() {
        assert_eq!(text_title("  \n  Hello   world \nsecond"), "Hello world");
        assert_eq!(text_title(""), "");
        let long = "a".repeat(60);
        let title = text_title(&long);
        assert_eq!(title.chars().count(), MAX_TITLE_CHARS);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn url_titles() {
        assert_eq!(url_title("https://example.com"), "example.com");
        assert_eq!(
            url_title("https://example.com/docs/page?q=1"),
            "example.com/docs"
        );
        assert_eq!(url_title("mailto:someone"), "mailto:someone");
        assert_eq!(url_title(""), "Link");
    }

    #[test]
    fn safe_stems() {
        assert_eq!(safe_file_stem("a/b:c", "x"), "a-b-c");
        assert_eq!(safe_file_stem("..hidden", "x"), "hidden");
        assert_eq!(safe_file_stem("  ", "Text"), "Text");
    }

    #[test]
    fn webloc_escapes_xml() {
        let plist = webloc("https://a.com/?x=1&y=<2>");
        assert!(plist.contains("<string>https://a.com/?x=1&amp;y=&lt;2&gt;</string>"));
    }

    #[test]
    fn materializes_each_content_kind() {
        let tmp = tempfile::tempdir().unwrap();
        let items = ItemsDir::new(tmp.path().to_path_buf());

        let text = items
            .materialize(Content::Text("Hi: there\nmore".into()))
            .unwrap();
        assert_eq!(text.kind, ItemKind::Text);
        assert_eq!(text.title, "Hi: there");
        assert_eq!(file_name(&text.path), "Hi- there.txt");
        assert_eq!(fs::read_to_string(&text.path).unwrap(), "Hi: there\nmore");
        assert!(text.owned);

        let rich = items
            .materialize(Content::RichText {
                rtf: b"{\\rtf1 x}".to_vec(),
                plain: "x".into(),
            })
            .unwrap();
        assert_eq!(file_name(&rich.path), "x.rtf");

        let image = items
            .materialize(Content::Image {
                data: vec![1, 2, 3],
                extension: "png",
            })
            .unwrap();
        assert_eq!(file_name(&image.path), "Image.png");

        let url = items
            .materialize(Content::Url {
                url: "https://example.com/a".into(),
                title: None,
            })
            .unwrap();
        assert_eq!(url.title, "example.com/a");
        assert_eq!(file_name(&url.path), "example.com-a.webloc");
        assert_eq!(
            url.kind,
            ItemKind::Url {
                url: "https://example.com/a".into()
            }
        );

        // Every item lives in its own folder named after its id.
        assert_eq!(
            text.path.parent().unwrap(),
            fs::canonicalize(tmp.path())
                .unwrap()
                .join(text.id.to_string())
        );
    }

    #[test]
    fn adopt_moves_promised_file_and_cleans_incoming() {
        let tmp = tempfile::tempdir().unwrap();
        let items = ItemsDir::new(tmp.path().to_path_buf());
        let incoming = items.incoming_dir().unwrap();
        let received = incoming.join("Attachment.pdf");
        fs::write(&received, b"pdf").unwrap();

        let item = items.adopt(&received).unwrap();
        assert_eq!(item.kind, ItemKind::File);
        assert_eq!(item.title, "Attachment.pdf");
        assert!(item.owned);
        assert_eq!(fs::read(&item.path).unwrap(), b"pdf");
        assert!(!incoming.exists());
    }

    #[test]
    fn delete_only_touches_owned_items_inside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Items");
        let items = ItemsDir::new(root.clone());

        let owned = items.materialize(Content::Text("x".into())).unwrap();
        items.delete(&owned).unwrap();
        assert!(!owned.path.exists());
        assert!(root.exists());

        // A user's file is never deleted, even if it sits in the items folder.
        let user_path = tmp.path().join("mine.txt");
        fs::write(&user_path, "keep").unwrap();
        items
            .delete(&Item::user_file(user_path.clone(), None))
            .unwrap();
        assert!(user_path.exists());

        // An owned item whose path points outside the root is left alone.
        let mut stray = owned.clone();
        stray.path = user_path.clone();
        items.delete(&stray).unwrap();
        assert!(user_path.exists());
    }

    #[test]
    fn owns_only_direct_item_folders() {
        let root = PathBuf::from("/data/Items");
        let items = ItemsDir::new(root.clone());
        let id = Uuid::new_v4().to_string();
        assert!(items.owns(&root.join(&id).join("a.txt")));
        assert!(!items.owns(&root.join("a.txt")));
        assert!(!items.owns(&root.join(INCOMING).join("a.txt")));
        assert!(!items.owns(&root.join(&id).join("sub/a.txt")));
        assert!(!items.owns(Path::new("/Users/me/Desktop/a.txt")));
    }

    #[test]
    fn remove_orphans_keeps_live_items_and_other_files() {
        let tmp = tempfile::tempdir().unwrap();
        let items = ItemsDir::new(tmp.path().to_path_buf());
        let live = items.materialize(Content::Text("live".into())).unwrap();
        let orphan = items.materialize(Content::Text("orphan".into())).unwrap();
        fs::create_dir_all(tmp.path().join("Not An Item")).unwrap();

        items.remove_orphans(&HashSet::from([live.id]));
        assert!(live.path.exists());
        assert!(!orphan.path.parent().unwrap().exists());
        assert!(tmp.path().join("Not An Item").exists());
    }

    #[test]
    fn owns_matches_through_symlinked_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        fs::create_dir_all(&real).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        // Created through the symlink, checked with the resolved path (as a
        // bookmark would return it), and the other way around.
        let via_link = ItemsDir::new(link.clone());
        let item = via_link.materialize(Content::Text("x".into())).unwrap();
        let resolved = fs::canonicalize(&item.path).unwrap();
        assert!(via_link.owns(&resolved));
        let unresolved = link
            .join(item.id.to_string())
            .join(item.path.file_name().unwrap());
        assert!(via_link.owns(&unresolved));
    }
}
