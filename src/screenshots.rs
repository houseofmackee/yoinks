//! Watches the screenshot folder (System Settings' choice, the Desktop by
//! default) and reports new screenshots and screen recordings, which macOS
//! marks with the `kMDItemIsScreenCapture` attribute.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::ffi::{CString, c_char, c_int, c_void};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::rc::{Rc, Weak};
use std::time::{Duration, SystemTime};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_foundation::{NSString, NSTimer, NSUserDefaults, ns_string};

const POLL_INTERVAL: f64 = 1.0;
/// How often the screenshot folder setting is read again, in polls.
const LOCATION_REFRESH: u32 = 30;
/// A new file without the screenshot marker is checked again for this long,
/// in case macOS adds the marker just after the file appears.
const MARKER_GRACE: Duration = Duration::from_secs(30);
const MARKER: &str = "com.apple.metadata:kMDItemIsScreenCapture";

pub struct ScreenshotWatcher {
    dir: RefCell<PathBuf>,
    /// Only files changed after this are new.
    since: SystemTime,
    /// The folder's modification time at the last scan.
    dir_modified: Cell<Option<SystemTime>>,
    /// Files already reported or ruled out.
    known: RefCell<HashSet<PathBuf>>,
    /// Recent files without the marker yet, so the folder is scanned again
    /// even if it did not change.
    waiting: Cell<bool>,
    polls: Cell<u32>,
    on_new: Box<dyn Fn(Vec<PathBuf>)>,
    timer: RefCell<Option<Retained<NSTimer>>>,
}

impl ScreenshotWatcher {
    /// Starts watching; files already there are ignored. `on_new` gets the
    /// new screenshots, oldest first. Watching stops when the watcher is
    /// dropped.
    pub fn start(on_new: impl Fn(Vec<PathBuf>) + 'static) -> Rc<Self> {
        let this = Rc::new(Self {
            dir: RefCell::new(screenshot_dir()),
            since: SystemTime::now(),
            dir_modified: Cell::new(None),
            known: RefCell::default(),
            waiting: Cell::new(false),
            polls: Cell::new(0),
            on_new: Box::new(on_new),
            timer: RefCell::new(None),
        });
        let weak: Weak<Self> = Rc::downgrade(&this);
        let block = RcBlock::new(move |_timer: NonNull<NSTimer>| {
            if let Some(watcher) = weak.upgrade() {
                watcher.poll();
            }
        });
        // SAFETY: Scheduled on the main run loop; the block only touches
        // main-thread state.
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_repeats_block(POLL_INTERVAL, true, &block)
        };
        *this.timer.borrow_mut() = Some(timer);
        this
    }

    fn poll(&self) {
        let polls = self.polls.get().wrapping_add(1);
        self.polls.set(polls);
        if polls.is_multiple_of(LOCATION_REFRESH) {
            let dir = screenshot_dir();
            if *self.dir.borrow() != dir {
                *self.dir.borrow_mut() = dir;
                self.dir_modified.set(None);
            }
        }
        let dir = self.dir.borrow().clone();
        let modified = fs::metadata(&dir).and_then(|m| m.modified()).ok();
        if modified == self.dir_modified.get() && !self.waiting.get() {
            return;
        }
        self.dir_modified.set(modified);
        let scan = scan(
            &dir,
            self.since,
            SystemTime::now(),
            &mut self.known.borrow_mut(),
            is_screen_capture,
        );
        self.waiting.set(scan.waiting);
        if !scan.found.is_empty() {
            (self.on_new)(scan.found);
        }
    }
}

impl Drop for ScreenshotWatcher {
    fn drop(&mut self) {
        if let Some(timer) = self.timer.take() {
            timer.invalidate();
        }
    }
}

struct Scan {
    found: Vec<PathBuf>,
    /// Some recent files may still get the marker.
    waiting: bool,
}

/// Finds files in `dir` changed after `since` that `is_capture` accepts and
/// that are not in `known`; decided files are added to `known`.
fn scan(
    dir: &Path,
    since: SystemTime,
    now: SystemTime,
    known: &mut HashSet<PathBuf>,
    is_capture: impl Fn(&Path) -> bool,
) -> Scan {
    let mut found: Vec<(SystemTime, PathBuf)> = Vec::new();
    let mut waiting = false;
    let Ok(entries) = fs::read_dir(dir) else {
        return Scan {
            found: Vec::new(),
            waiting,
        };
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // macOS writes screenshots under a hidden name first.
        if entry.file_name().as_bytes().starts_with(b".") || known.contains(&path) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if !metadata.is_file() || modified < since {
            continue;
        }
        if is_capture(&path) {
            known.insert(path.clone());
            found.push((modified, path));
        } else if now.duration_since(modified).unwrap_or_default() < MARKER_GRACE {
            waiting = true;
        } else {
            known.insert(path);
        }
    }
    found.sort();
    Scan {
        found: found.into_iter().map(|(_, path)| path).collect(),
        waiting,
    }
}

/// The folder screenshots are saved to (`defaults read
/// com.apple.screencapture location`), the Desktop if unset.
fn screenshot_dir() -> PathBuf {
    let home = std::env::home_dir().unwrap_or_default();
    let location = NSUserDefaults::standardUserDefaults()
        .persistentDomainForName(ns_string!("com.apple.screencapture"))
        .and_then(|domain| domain.objectForKey(ns_string!("location")))
        .and_then(|value| value.downcast::<NSString>().ok())
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty());
    match location {
        Some(path) => match path.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(path),
        },
        None => home.join("Desktop"),
    }
}

unsafe extern "C" {
    fn getxattr(
        path: *const c_char,
        name: *const c_char,
        value: *mut c_void,
        size: usize,
        position: u32,
        options: c_int,
    ) -> isize;
}

/// Whether macOS marked the file as a screenshot or screen recording.
fn is_screen_capture(path: &Path) -> bool {
    let (Ok(path), Ok(name)) = (
        CString::new(path.as_os_str().as_bytes()),
        CString::new(MARKER),
    ) else {
        return false;
    };
    // SAFETY: Valid C strings; a null buffer asks only for the size.
    unsafe { getxattr(path.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0, 0, 0) >= 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(name: &str) -> impl Fn(&Path) -> bool {
        let name = name.to_owned();
        move |path: &Path| {
            path.file_name()
                .is_some_and(|n| n.to_string_lossy().contains(&name))
        }
    }

    #[test]
    fn finds_new_captures_once() {
        let tmp = tempfile::tempdir().unwrap();
        let since = SystemTime::now() - Duration::from_secs(5);
        fs::write(tmp.path().join("Screenshot 1.png"), b"x").unwrap();
        fs::write(tmp.path().join("notes.txt"), b"x").unwrap();
        fs::write(tmp.path().join(".Screenshot 2.png"), b"x").unwrap();
        fs::create_dir(tmp.path().join("Screenshot folder")).unwrap();
        let mut known = HashSet::new();

        let scan1 = scan(
            tmp.path(),
            since,
            SystemTime::now(),
            &mut known,
            named("Screenshot"),
        );
        let names: Vec<_> = scan1
            .found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["Screenshot 1.png"]);
        assert!(
            scan1.waiting,
            "notes.txt is recent and may still get the marker"
        );

        let scan2 = scan(
            tmp.path(),
            since,
            SystemTime::now(),
            &mut known,
            named("Screenshot"),
        );
        assert!(scan2.found.is_empty(), "reported once");

        let later = SystemTime::now() + MARKER_GRACE + Duration::from_secs(1);
        let scan3 = scan(tmp.path(), since, later, &mut known, named("Screenshot"));
        assert!(
            !scan3.waiting,
            "notes.txt is ruled out after the grace period"
        );
        assert!(known.contains(&tmp.path().join("notes.txt")));
    }

    #[test]
    fn ignores_files_from_before_watching() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("Screenshot old.png"), b"x").unwrap();
        let since = SystemTime::now() + Duration::from_secs(60);
        let scan = scan(
            tmp.path(),
            since,
            SystemTime::now(),
            &mut HashSet::new(),
            |_| true,
        );
        assert!(scan.found.is_empty());
    }

    #[test]
    fn missing_folder_finds_nothing() {
        let scan = scan(
            Path::new("/nonexistent/yoinks"),
            SystemTime::UNIX_EPOCH,
            SystemTime::now(),
            &mut HashSet::new(),
            |_| true,
        );
        assert!(scan.found.is_empty());
    }
}
