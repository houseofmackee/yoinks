//! Turns dropped non-file content (text, images, links) into real files inside
//! the Yoinks items folder, so every shelf item can later be dragged out as a
//! file. Also owns cleanup of those files.

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
    pub fn new(root: PathBuf) -> Self {
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

    /// Deletes the folder of an owned item. User files are never touched, and
    /// nothing outside the items folder is ever deleted.
    pub fn delete(&self, item: &Item) -> io::Result<()> {
        if !item.owned {
            return Ok(());
        }
        match item.path.parent() {
            Some(dir) if dir.parent() == Some(self.root.as_path()) && dir != self.root => {
                fs::remove_dir_all(dir)
            }
            _ => Ok(()),
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
            tmp.path().join(text.id.to_string())
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
}
