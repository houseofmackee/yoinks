//! Quick Look thumbnails for shelf items, cached as PNGs in the data folder
//! (`Thumbnails/<item-id>.png`) and regenerated when the file changes.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Weak;
use std::sync::Arc;
use std::time::SystemTime;

use block2::RcBlock;
use dispatch2::{DispatchQueue, MainThreadBound};
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::NSScreen;
use objc2_foundation::{NSError, NSSize, NSURL};
use objc2_quick_look_thumbnailing::{
    QLThumbnailGenerationRequest, QLThumbnailGenerationRequestRepresentationTypes,
    QLThumbnailGenerator,
};
use objc2_uniform_type_identifiers::UTTypePNG;
use uuid::Uuid;

use super::item::{Item, ItemKind};
use super::model::ShelfModel;

/// Thumbnail edge length in points (the tile icon size).
pub const THUMBNAIL_POINTS: f64 = 56.0;

pub struct Thumbnails {
    dir: PathBuf,
    /// The file modification time each item was last requested for. A file
    /// version is requested at most once per session, so failures (or a
    /// file dated in the future) never cause a request loop.
    requested: RefCell<HashMap<Uuid, Option<SystemTime>>>,
}

impl Thumbnails {
    pub fn new(dir: PathBuf) -> Self {
        let _ = fs::create_dir_all(&dir);
        Self {
            dir,
            requested: RefCell::default(),
        }
    }

    fn path(&self, id: Uuid) -> PathBuf {
        self.dir.join(format!("{id}.png"))
    }

    /// The cached thumbnail for `item`, if there is one at least as new as
    /// the item's file.
    pub fn cached(&self, item: &Item) -> Option<PathBuf> {
        let path = self.path(item.id);
        is_fresh(&path, &item.path).then_some(path)
    }

    /// The thumbnail for `item` if cached; otherwise starts generating one
    /// (once per file version) and returns `None`. `model` is told when the
    /// request finishes.
    pub fn get_or_request(
        &self,
        mtm: MainThreadMarker,
        item: &Item,
        model: &Weak<ShelfModel>,
    ) -> Option<PathBuf> {
        if let Some(path) = self.cached(item) {
            return Some(path);
        }
        if matches!(item.kind, ItemKind::Pending | ItemKind::Url { .. }) {
            return None;
        }
        let version = modified(&item.path);
        let previous = self.requested.borrow_mut().insert(item.id, version);
        if previous != Some(version) {
            self.generate(mtm, item, model);
        }
        None
    }

    /// Deletes every cached thumbnail and forgets past requests.
    pub fn clear(&self) {
        self.requested.borrow_mut().clear();
        self.remove_orphans(&HashSet::new());
    }

    pub fn remove(&self, id: Uuid) {
        let _ = fs::remove_file(self.path(id));
    }

    /// Deletes thumbnails of items no longer on the shelf.
    pub fn remove_orphans(&self, live: &HashSet<Uuid>) {
        let Ok(files) = fs::read_dir(&self.dir) else {
            return;
        };
        for file in files.flatten() {
            let path = file.path();
            let id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| Uuid::parse_str(stem).ok());
            if id.is_some_and(|id| !live.contains(&id)) {
                let _ = fs::remove_file(path);
            }
        }
    }

    /// Asks Quick Look to write a thumbnail PNG. Its completion runs on a
    /// background queue and only sends the item id and outcome back to the
    /// main thread.
    fn generate(&self, mtm: MainThreadMarker, item: &Item, model: &Weak<ShelfModel>) {
        let id = item.id;
        let (Some(source), Some(destination)) = (
            NSURL::from_file_path(&item.path),
            NSURL::from_file_path(self.path(id)),
        ) else {
            return;
        };
        let scale = NSScreen::mainScreen(mtm).map_or(2.0, |screen| screen.backingScaleFactor());
        let model = Arc::new(MainThreadBound::new(model.clone(), mtm));
        let completion = RcBlock::new(move |error: *mut NSError| {
            let ok = error.is_null();
            let model = Arc::clone(&model);
            DispatchQueue::main().exec_async(move || {
                let mtm = MainThreadMarker::new().expect("main queue runs on the main thread");
                if let Some(model) = model.get(mtm).upgrade() {
                    model.thumbnail_finished(id, ok);
                }
            });
        });
        // SAFETY: Valid URLs and request; Quick Look calls the completion
        // handler exactly once, on a background queue, and it only touches
        // `Send` data before hopping to the main queue.
        unsafe {
            let request =
                QLThumbnailGenerationRequest::initWithFileAtURL_size_scale_representationTypes(
                    QLThumbnailGenerationRequest::alloc(),
                    &source,
                    NSSize::new(THUMBNAIL_POINTS, THUMBNAIL_POINTS),
                    scale,
                    QLThumbnailGenerationRequestRepresentationTypes::Thumbnail,
                );
            QLThumbnailGenerator::sharedGenerator()
                .saveBestRepresentationForRequest_toFileAtURL_asContentType_completionHandler(
                    &request,
                    &destination,
                    UTTypePNG,
                    &completion,
                );
        }
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// True if `thumbnail` exists and is not older than `source`.
fn is_fresh(thumbnail: &Path, source: &Path) -> bool {
    match (modified(thumbnail), modified(source)) {
        (Some(thumbnail), Some(source)) => thumbnail >= source,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;

    #[test]
    fn freshness_follows_modification_times() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("a.txt");
        let thumb = tmp.path().join("t.png");
        fs::write(&source, "x").unwrap();
        assert!(!is_fresh(&thumb, &source), "no thumbnail yet");

        fs::write(&thumb, "png").unwrap();
        let now = SystemTime::now();
        fs::File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_modified(now)
            .unwrap();
        fs::File::options()
            .write(true)
            .open(&thumb)
            .unwrap()
            .set_modified(now)
            .unwrap();
        assert!(is_fresh(&thumb, &source));

        let later = now + Duration::from_secs(5);
        fs::File::options()
            .write(true)
            .open(&source)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert!(
            !is_fresh(&thumb, &source),
            "source edited after the thumbnail"
        );
    }

    #[test]
    fn orphans_are_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let thumbnails = Thumbnails::new(tmp.path().to_path_buf());
        let (live, gone) = (Uuid::new_v4(), Uuid::new_v4());
        fs::write(thumbnails.path(live), "a").unwrap();
        fs::write(thumbnails.path(gone), "b").unwrap();
        fs::write(tmp.path().join("notes.txt"), "keep").unwrap();
        thumbnails.remove_orphans(&HashSet::from([live]));
        assert!(thumbnails.path(live).exists());
        assert!(!thumbnails.path(gone).exists());
        assert!(tmp.path().join("notes.txt").exists());
    }
}
