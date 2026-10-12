//! Answers requests from the command line and `yoinks://` links.

use std::path::PathBuf;

use crate::ipc::{EntryInfo, ItemInfo, Request, Response};

/// The Share extension's sandbox container, relative to the home folder.
const SHARE_CONTAINER: &str = "Library/Containers/io.github.yoinks.share";
use crate::shelf::{Content, Dropped, Entry, HIDE_AFTER_ADD, ItemKind, Shelf};

pub fn handle(shelf: &Shelf, request: Request) -> Response {
    let model = shelf.model();
    match request {
        Request::Add {
            paths,
            text,
            url,
            title,
        } => {
            if let Some(missing) = paths.iter().find(|path| !path.exists()) {
                return Response::error(format!("no such file: {}", missing.display()));
            }
            // Files the Share extension wrote into its own container would
            // be cleaned up by macOS, so Yoinks takes them over.
            let share_dir = std::env::home_dir().map(|home| home.join(SHARE_CONTAINER));
            let (shared, paths): (Vec<PathBuf>, Vec<PathBuf>) = paths
                .into_iter()
                .partition(|path| share_dir.as_ref().is_some_and(|dir| path.starts_with(dir)));
            let adopted = !shared.is_empty() && model.adopt_files(&shared);
            let mut dropped = Dropped {
                files: paths,
                ..Dropped::default()
            };
            if let Some(text) = text {
                dropped.contents.push(Content::Text(text));
            }
            if let Some(url) = url {
                dropped.contents.push(Content::Url { url, title });
            }
            let nothing_else = dropped.files.is_empty() && dropped.contents.is_empty();
            let accepted = !nothing_else && model.accept(dropped);
            if !adopted && !accepted {
                return Response::error("nothing to add");
            }
            // Show what was added without taking keyboard focus from the
            // terminal or browser the request came from, then slide away
            // again if it is left alone.
            shelf.appear_at_edge();
            shelf.arm_auto_hide(HIDE_AFTER_ADD);
        }
        Request::List => {}
        Request::Clear => model.clear(),
        Request::Show => shelf.appear_at_edge(),
        Request::Hide => shelf.hide(),
        Request::Recall => {
            if !model.recall() {
                return Response::error("nothing to recall");
            }
            shelf.appear_at_edge();
            shelf.arm_auto_hide(HIDE_AFTER_ADD);
        }
        Request::Shelf { name: Some(name) } => {
            let found = model
                .shelves()
                .into_iter()
                .find(|s| s.name.eq_ignore_ascii_case(name.trim()));
            let Some(found) = found else {
                return Response::error(format!("no shelf called \"{}\"", name.trim()));
            };
            model.switch_shelf(&found.id);
            shelf.appear_at_edge();
            shelf.arm_auto_hide(HIDE_AFTER_ADD);
        }
        Request::Shelf { name: None } => {}
    }
    let mut response = Response::ok(model.entries().iter().map(entry_info).collect());
    response.shelf = Some(model.current_shelf().name);
    response.shelves = model.shelves().into_iter().map(|s| s.name).collect();
    response
}

fn entry_info(entry: &Entry) -> EntryInfo {
    EntryInfo {
        id: entry.id.to_string(),
        title: entry.title(),
        pinned: entry.pinned,
        items: entry
            .items
            .iter()
            .map(|item| {
                let (kind, url) = match &item.kind {
                    ItemKind::File => ("file", None),
                    ItemKind::Text => ("text", None),
                    ItemKind::RichText => ("rich_text", None),
                    ItemKind::Image => ("image", None),
                    ItemKind::Url { url } => ("url", Some(url.clone())),
                    ItemKind::Pending => ("pending", None),
                };
                ItemInfo {
                    title: item.title.clone(),
                    kind: kind.to_owned(),
                    path: item.path.clone(),
                    url,
                }
            })
            .collect(),
    }
}
