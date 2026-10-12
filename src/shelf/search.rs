//! Matching shelf entries against a search: every word must appear in the
//! entry's names, links, kinds or text. Plain Rust, so it can be tested.

use super::item::{Entry, Item, ItemKind};

/// A parsed search: lowercase words, all of which must match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    words: Vec<String>,
}

impl Query {
    /// None for a blank search, which shows everything.
    pub fn parse(text: &str) -> Option<Self> {
        let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
        (!words.is_empty()).then_some(Self { words })
    }

    /// Whether every word appears somewhere in the entry. `text_of` gives
    /// the contents of text items (None when there are none to search).
    pub fn matches(&self, entry: &Entry, text_of: &dyn Fn(&Item) -> Option<String>) -> bool {
        let mut haystack = entry.title().to_lowercase();
        for item in &entry.items {
            haystack.push('\n');
            haystack.push_str(&describe(item));
            if let Some(text) = text_of(item) {
                haystack.push('\n');
                haystack.push_str(&text.to_lowercase());
            }
        }
        self.words
            .iter()
            .all(|word| haystack.contains(word.as_str()))
    }
}

/// An item's title, file name, link and kind words, lowercase.
fn describe(item: &Item) -> String {
    let mut parts = vec![item.title.clone()];
    if let Some(name) = item.path.file_name() {
        parts.push(name.to_string_lossy().into_owned());
    }
    let kind = match &item.kind {
        ItemKind::File => "file",
        ItemKind::Text => "text",
        ItemKind::RichText => "text rich",
        ItemKind::Image => "image",
        ItemKind::Url { url } => {
            parts.push(url.clone());
            "link url"
        }
        ItemKind::Pending => "",
    };
    parts.push(kind.to_owned());
    parts.join("\n").to_lowercase()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn item(kind: ItemKind, title: &str, path: &str) -> Item {
        let mut item = Item::pending();
        item.kind = kind;
        item.title = title.to_owned();
        item.path = PathBuf::from(path);
        item
    }

    fn no_text(_: &Item) -> Option<String> {
        None
    }

    #[test]
    fn blank_search_is_none() {
        assert_eq!(Query::parse("   "), None);
        assert!(Query::parse(" a ").is_some());
    }

    #[test]
    fn matches_names_kinds_and_links() {
        let file = Entry::new(vec![item(
            ItemKind::File,
            "Report.pdf",
            "/Users/me/Report.pdf",
        )]);
        let link = Entry::new(vec![item(
            ItemKind::Url {
                url: "https://example.com/docs".to_owned(),
            },
            "Docs",
            "/data/Docs.webloc",
        )]);
        let q = |text: &str| Query::parse(text).unwrap();

        assert!(q("report").matches(&file, &no_text));
        assert!(
            q("REPORT pdf").matches(&file, &no_text),
            "case and every word"
        );
        assert!(q("file").matches(&file, &no_text), "kind word");
        assert!(
            !q("report link").matches(&file, &no_text),
            "all words must match"
        );
        assert!(q("example.com").matches(&link, &no_text));
        assert!(q("link").matches(&link, &no_text));
        assert!(!q("invoice").matches(&link, &no_text));
    }

    #[test]
    fn matches_stacks_and_text_contents() {
        let stack = Entry::new(vec![
            item(ItemKind::File, "a.png", "/x/a.png"),
            item(ItemKind::Text, "Meeting notes", "/data/Meeting notes.txt"),
        ]);
        let text_of = |item: &Item| {
            (item.kind == ItemKind::Text).then(|| "Agenda: ship the Invoice fix".to_owned())
        };
        let q = |text: &str| Query::parse(text).unwrap();

        assert!(q("2 items").matches(&stack, &no_text), "the stack's title");
        assert!(
            q("a.png").matches(&stack, &no_text),
            "any item in the stack"
        );
        assert!(q("invoice").matches(&stack, &text_of), "text contents");
        assert!(!q("invoice").matches(&stack, &no_text));
        assert!(
            q("png meeting").matches(&stack, &no_text),
            "words across items"
        );
    }
}
