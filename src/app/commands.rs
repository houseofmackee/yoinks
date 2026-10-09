//! Answers requests from the command line and `yoinks://` links.

use crate::ipc::{EntryInfo, ItemInfo, Request, Response};
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
            if !model.accept(dropped) {
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
    }
    Response::ok(model.entries().iter().map(entry_info).collect())
}

fn entry_info(entry: &Entry) -> EntryInfo {
    EntryInfo {
        id: entry.id.to_string(),
        title: entry.title(),
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
