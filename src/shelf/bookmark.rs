//! File bookmarks: let a shelf item follow its file across renames and moves.

use std::path::{Path, PathBuf};

use objc2::runtime::Bool;
use objc2_foundation::{
    NSData, NSURL, NSURLBookmarkCreationOptions, NSURLBookmarkResolutionOptions,
};

pub fn create(path: &Path) -> Option<Vec<u8>> {
    let url = NSURL::from_file_path(path)?;
    url.bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
        NSURLBookmarkCreationOptions::empty(),
        None,
        None,
    )
    .ok()
    .map(|data| data.to_vec())
}

pub struct Resolved {
    pub path: PathBuf,
    /// The bookmark still works but should be recreated.
    pub stale: bool,
}

pub fn resolve(bookmark: &[u8]) -> Option<Resolved> {
    let data = NSData::with_bytes(bookmark);
    let mut stale = Bool::NO;
    // SAFETY: `stale` is a valid pointer for the duration of the call.
    let url = unsafe {
        NSURL::URLByResolvingBookmarkData_options_relativeToURL_bookmarkDataIsStale_error(
            &data,
            // Never show UI or mount network volumes while launching.
            NSURLBookmarkResolutionOptions::WithoutUI
                | NSURLBookmarkResolutionOptions::WithoutMounting,
            None,
            &mut stale,
        )
    }
    .ok()?;
    Some(Resolved {
        path: url.to_file_path()?,
        stale: stale.as_bool(),
    })
}
