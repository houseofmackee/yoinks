//! Requests and responses shared by the command line, the `yoinks://` link
//! scheme and the app. Sent over the socket as one JSON object per line.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// Adds one entry: the files (a stack if several), or text, or a link.
    Add {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        paths: Vec<PathBuf>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    List,
    Clear,
    Show,
    Hide,
    Recall,
    /// Switch to the named shelf (any case); without a name, just report
    /// the shelves.
    Shelf {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

impl Request {
    pub fn add_paths(paths: Vec<PathBuf>) -> Self {
        Self::Add {
            paths,
            text: None,
            url: None,
            title: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The shelf after the request, newest first.
    #[serde(default)]
    pub entries: Vec<EntryInfo>,
    /// The shelf the entries are on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shelf: Option<String>,
    /// Every shelf's name, in menu order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shelves: Vec<String>,
}

impl Response {
    pub fn ok(entries: Vec<EntryInfo>) -> Self {
        Self {
            ok: true,
            error: None,
            entries,
            shelf: None,
            shelves: Vec::new(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(message.into()),
            entries: Vec::new(),
            shelf: None,
            shelves: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryInfo {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub pinned: bool,
    pub items: Vec<ItemInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemInfo {
    pub title: String,
    /// `file`, `text`, `rich_text`, `image`, `url` or `pending`.
    pub kind: String,
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip_as_tagged_json() {
        let add = Request::add_paths(vec!["/tmp/a.txt".into()]);
        let json = serde_json::to_string(&add).unwrap();
        assert_eq!(json, r#"{"cmd":"add","paths":["/tmp/a.txt"]}"#);
        assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), add);
        assert_eq!(
            serde_json::from_str::<Request>(r#"{"cmd":"list"}"#).unwrap(),
            Request::List
        );
        let text: Request = serde_json::from_str(r#"{"cmd":"add","text":"hi"}"#).unwrap();
        assert!(matches!(text, Request::Add { text: Some(t), .. } if t == "hi"));
        assert!(serde_json::from_str::<Request>(r#"{"cmd":"format_disk"}"#).is_err());
    }

    #[test]
    fn error_responses_omit_entries_noise() {
        let json = serde_json::to_string(&Response::error("nope")).unwrap();
        assert_eq!(json, r#"{"ok":false,"error":"nope","entries":[]}"#);
    }
}
