//! Tooltip text for shelf tiles. Pure formatting; the caller looks up file
//! sizes and kinds.

use std::path::Path;

use super::item::{Entry, Item, ItemKind};

/// Facts about an item's file gathered by the caller.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileFacts {
    /// Size in bytes; `None` for folders or unreadable files.
    pub size: Option<u64>,
    /// Localized kind, e.g. "PDF document" or "Folder".
    pub kind: Option<String>,
    /// The first part of a text item's contents.
    pub excerpt: Option<String>,
}

const EXCERPT_LINES: usize = 4;
const EXCERPT_CHARS: usize = 200;

/// The tooltip for an entry: one block per item, separated by blank lines.
/// `facts` must have one element per item.
pub fn tooltip(entry: &Entry, facts: &[FileFacts], home: Option<&Path>) -> String {
    let blocks: Vec<String> = entry
        .items
        .iter()
        .zip(facts)
        .map(|(item, facts)| item_tooltip(item, facts, home))
        .collect();
    if entry.is_stack() {
        format!("{} items\n\n{}", entry.items.len(), blocks.join("\n\n"))
    } else {
        blocks.join("\n\n")
    }
}

fn item_tooltip(item: &Item, facts: &FileFacts, home: Option<&Path>) -> String {
    let mut lines = vec![item.title.clone()];
    let details: Vec<String> = [facts.kind.clone(), facts.size.map(format_bytes)]
        .into_iter()
        .flatten()
        .collect();
    if !details.is_empty() {
        lines.push(details.join(", "));
    }
    match &item.kind {
        ItemKind::Url { url } => lines.push(url.clone()),
        ItemKind::Pending => lines.push("Still being received".to_owned()),
        _ if item.owned => {}
        // Where the user's own file lives.
        _ => {
            if let Some(parent) = item.path.parent() {
                lines.push(format!("In {}", tilde(parent, home)));
            }
        }
    }
    if let Some(excerpt) = &facts.excerpt {
        lines.push(String::new());
        lines.push(excerpt.clone());
    }
    lines.join("\n")
}

/// The start of `text`: a few lines, shortened with an ellipsis.
pub fn excerpt(text: &str) -> String {
    let lines: Vec<&str> = text.lines().take(EXCERPT_LINES).collect();
    let mut joined = lines.join("\n");
    let truncated_lines = text.lines().count() > EXCERPT_LINES;
    if joined.chars().count() > EXCERPT_CHARS {
        joined = joined.chars().take(EXCERPT_CHARS - 1).collect::<String>();
        joined = format!("{}…", joined.trim_end());
    } else if truncated_lines {
        joined.push('…');
    }
    joined
}

/// Byte counts the way Finder shows them (decimal units).
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if bytes < 1000 {
        return if bytes == 1 {
            "1 byte".to_owned()
        } else {
            format!("{bytes} bytes")
        };
    }
    let mut value = bytes as f64;
    let mut unit = "";
    for name in UNITS {
        value /= 1000.0;
        unit = name;
        if value < 999.95 {
            break;
        }
    }
    if value < 10.0 {
        format!("{value:.1} {unit}")
    } else {
        format!("{value:.0} {unit}")
    }
}

/// `path` with the home folder shown as `~`.
pub fn tilde(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn bytes() {
        assert_eq!(format_bytes(0), "0 bytes");
        assert_eq!(format_bytes(1), "1 byte");
        assert_eq!(format_bytes(999), "999 bytes");
        assert_eq!(format_bytes(1_000), "1.0 KB");
        assert_eq!(format_bytes(12_345), "12 KB");
        assert_eq!(format_bytes(3_400_000), "3.4 MB");
        assert_eq!(format_bytes(999_960), "1.0 MB");
        assert_eq!(format_bytes(5_000_000_000), "5.0 GB");
    }

    #[test]
    fn tilde_paths() {
        let home = PathBuf::from("/Users/me");
        assert_eq!(
            tilde(Path::new("/Users/me/Desktop"), Some(&home)),
            "~/Desktop"
        );
        assert_eq!(tilde(Path::new("/Users/me"), Some(&home)), "~");
        assert_eq!(
            tilde(Path::new("/Volumes/USB"), Some(&home)),
            "/Volumes/USB"
        );
        assert_eq!(tilde(Path::new("/tmp"), None), "/tmp");
    }

    #[test]
    fn excerpts() {
        assert_eq!(excerpt("one\ntwo"), "one\ntwo");
        assert_eq!(excerpt("1\n2\n3\n4\n5\n6"), "1\n2\n3\n4…");
        let long = "x".repeat(300);
        assert_eq!(excerpt(&long).chars().count(), EXCERPT_CHARS);
    }

    #[test]
    fn user_file_tooltip() {
        let home = PathBuf::from("/Users/me");
        let entry = Entry::new(vec![Item::user_file(
            "/Users/me/Desktop/a.pdf".into(),
            None,
        )]);
        let facts = FileFacts {
            size: Some(12_345),
            kind: Some("PDF document".into()),
            excerpt: None,
        };
        assert_eq!(
            tooltip(&entry, &[facts], Some(&home)),
            "a.pdf\nPDF document, 12 KB\nIn ~/Desktop"
        );
    }

    #[test]
    fn link_and_stack_tooltips() {
        let mut link = Item::user_file("/data/Items/x/Docs.webloc".into(), None);
        link.title = "Docs".into();
        link.owned = true;
        link.kind = ItemKind::Url {
            url: "https://example.com".into(),
        };
        let mut text = Item::user_file("/data/Items/y/Hello.txt".into(), None);
        text.title = "Hello".into();
        text.owned = true;
        text.kind = ItemKind::Text;
        let entry = Entry::new(vec![link, text]);
        let facts = [
            FileFacts::default(),
            FileFacts {
                size: Some(5),
                kind: Some("Plain Text".into()),
                excerpt: Some("Hello".into()),
            },
        ];
        assert_eq!(
            tooltip(&entry, &facts, None),
            "2 items\n\nDocs\nhttps://example.com\n\nHello\nPlain Text, 5 bytes\n\nHello"
        );
    }
}
