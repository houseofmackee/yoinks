//! Watches folders and reports what appears in them: the screenshot folder
//! (System Settings' choice, the Desktop by default) for new screenshots and
//! screen recordings, which macOS marks with `kMDItemIsScreenCapture`, and
//! the user's watched folders for anything new once it has stopped changing.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::ffi::{CString, c_char, c_int, c_void};
use std::fs::{self, Metadata};
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
/// A new file that is not ready yet (no screenshot marker, or a download
/// still in progress) is checked again for this long.
const GRACE: Duration = Duration::from_secs(30);
/// A file in a watched folder is taken once it has not changed for this
/// long, so half-copied files are not.
const SETTLE: Duration = Duration::from_secs(2);
const MARKER: &str = "com.apple.metadata:kMDItemIsScreenCapture";
/// Extensions of files that are still being downloaded or written.
const IN_PROGRESS: [&str; 6] = [
    "download",
    "crdownload",
    "part",
    "partial",
    "tmp",
    "opdownload",
];

/// A folder to watch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Folder {
    /// The screenshot folder, for screenshots and screen recordings only.
    Screenshots,
    /// Any folder; everything new in it counts.
    Path(PathBuf),
}

struct Watched {
    folder: Folder,
    dir: PathBuf,
    /// The folder's modification time at the last scan.
    modified: Option<SystemTime>,
    /// Recent files are not ready yet, so the folder is scanned again even
    /// if it did not change.
    waiting: bool,
}

pub struct FolderWatcher {
    watched: RefCell<Vec<Watched>>,
    /// Only files changed after this are new.
    since: SystemTime,
    /// Files already reported or ruled out.
    known: RefCell<HashSet<PathBuf>>,
    polls: Cell<u32>,
    on_new: Box<dyn Fn(Vec<PathBuf>)>,
    timer: RefCell<Option<Retained<NSTimer>>>,
}

impl FolderWatcher {
    /// Starts watching `folders`; what is already there is ignored. `on_new`
    /// gets new files, oldest first. Watching stops when the watcher is
    /// dropped.
    pub fn start(folders: Vec<Folder>, on_new: impl Fn(Vec<PathBuf>) + 'static) -> Rc<Self> {
        let paths: Vec<PathBuf> = folders
            .iter()
            .filter_map(|folder| match folder {
                Folder::Path(path) => Some(path.clone()),
                Folder::Screenshots => None,
            })
            .collect();
        let watched = folders
            .into_iter()
            .map(|folder| Watched {
                dir: resolve(&folder),
                folder,
                modified: None,
                waiting: false,
            })
            // A watched folder that is also the screenshot folder already
            // takes everything new in it.
            .filter(|w| w.folder != Folder::Screenshots || !paths.contains(&w.dir))
            .collect();
        let this = Rc::new(Self {
            watched: RefCell::new(watched),
            since: SystemTime::now(),
            known: RefCell::default(),
            polls: Cell::new(0),
            on_new: Box::new(on_new),
            timer: RefCell::new(None),
        });
        // Note what is there now, so only later changes trigger a scan.
        for watched in this.watched.borrow_mut().iter_mut() {
            watched.modified = dir_modified(&watched.dir);
        }
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
        if !crate::power::due(polls, crate::power::SCREENSHOT_SKIP) {
            return;
        }
        let refresh = polls.is_multiple_of(LOCATION_REFRESH);
        let now = SystemTime::now();
        let mut found = Vec::new();
        for watched in self.watched.borrow_mut().iter_mut() {
            if refresh && watched.folder == Folder::Screenshots {
                let dir = screenshot_dir();
                if watched.dir != dir {
                    watched.dir = dir;
                    watched.modified = None;
                }
            }
            let modified = dir_modified(&watched.dir);
            if modified == watched.modified && !watched.waiting {
                continue;
            }
            watched.modified = modified;
            let verdict = match watched.folder {
                Folder::Screenshots => screenshot_verdict,
                Folder::Path(_) => settled_verdict,
            };
            let scan = scan(
                &watched.dir,
                self.since,
                now,
                &mut self.known.borrow_mut(),
                verdict,
            );
            watched.waiting = scan.waiting;
            found.extend(scan.found);
        }
        if !found.is_empty() {
            (self.on_new)(found);
        }
    }
}

impl Drop for FolderWatcher {
    fn drop(&mut self) {
        if let Some(timer) = self.timer.take() {
            timer.invalidate();
        }
    }
}

/// What to do with a new file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Take,
    /// Not ready yet; look again later.
    Wait,
    Skip,
}

/// Screenshot folder: screenshots and recordings only.
fn screenshot_verdict(path: &Path, metadata: &Metadata, age: Duration) -> Verdict {
    if !metadata.is_file() {
        Verdict::Skip
    } else if is_screen_capture(path) {
        Verdict::Take
    } else if age < GRACE {
        // macOS may add the marker just after the file appears.
        Verdict::Wait
    } else {
        Verdict::Skip
    }
}

/// Watched folder: files and folders, once they have stopped changing and
/// are not downloads in progress.
fn settled_verdict(path: &Path, _metadata: &Metadata, age: Duration) -> Verdict {
    let in_progress = path
        .extension()
        .is_some_and(|ext| IN_PROGRESS.iter().any(|e| ext.eq_ignore_ascii_case(e)));
    if in_progress {
        // Renamed when finished, which makes it a new file.
        if age < GRACE {
            Verdict::Wait
        } else {
            Verdict::Skip
        }
    } else if age < SETTLE {
        Verdict::Wait
    } else {
        Verdict::Take
    }
}

struct Scan {
    found: Vec<PathBuf>,
    waiting: bool,
}

/// Finds entries in `dir` changed after `since` and not in `known` that
/// `verdict` takes (given their age); decided entries are added to `known`.
fn scan(
    dir: &Path,
    since: SystemTime,
    now: SystemTime,
    known: &mut HashSet<PathBuf>,
    verdict: impl Fn(&Path, &Metadata, Duration) -> Verdict,
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
        // macOS writes screenshots under a hidden name first; .DS_Store and
        // friends are hidden too.
        if entry.file_name().as_bytes().starts_with(b".") || known.contains(&path) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if modified < since {
            continue;
        }
        let age = now.duration_since(modified).unwrap_or_default();
        match verdict(&path, &metadata, age) {
            Verdict::Take => {
                known.insert(path.clone());
                found.push((modified, path));
            }
            Verdict::Wait => waiting = true,
            Verdict::Skip => {
                known.insert(path);
            }
        }
    }
    found.sort();
    Scan {
        found: found.into_iter().map(|(_, path)| path).collect(),
        waiting,
    }
}

fn resolve(folder: &Folder) -> PathBuf {
    match folder {
        Folder::Screenshots => screenshot_dir(),
        Folder::Path(path) => path.clone(),
    }
}

fn dir_modified(dir: &Path) -> Option<SystemTime> {
    fs::metadata(dir).and_then(|m| m.modified()).ok()
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

    /// Takes files whose names contain `name`, like the marker would.
    fn named(name: &str) -> impl Fn(&Path, &Metadata, Duration) -> Verdict {
        let name = name.to_owned();
        move |path: &Path, metadata: &Metadata, age: Duration| {
            let marked = path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().contains(&name));
            match (metadata.is_file(), marked) {
                (false, _) => Verdict::Skip,
                (true, true) => Verdict::Take,
                (true, false) if age < GRACE => Verdict::Wait,
                _ => Verdict::Skip,
            }
        }
    }

    fn names(paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
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
        assert_eq!(names(&scan1.found), ["Screenshot 1.png"]);
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

        let later = SystemTime::now() + GRACE + Duration::from_secs(1);
        let scan3 = scan(tmp.path(), since, later, &mut known, named("Screenshot"));
        assert!(
            !scan3.waiting,
            "notes.txt is ruled out after the grace period"
        );
        assert!(known.contains(&tmp.path().join("notes.txt")));
    }

    #[test]
    fn watched_folders_take_settled_files_and_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let since = SystemTime::now() - Duration::from_secs(5);
        fs::write(tmp.path().join("Report.pdf"), b"x").unwrap();
        fs::write(tmp.path().join("Movie.mov.download"), b"x").unwrap();
        fs::create_dir(tmp.path().join("Unzipped")).unwrap();
        fs::write(tmp.path().join(".DS_Store"), b"x").unwrap();
        let mut known = HashSet::new();

        let fresh = scan(
            tmp.path(),
            since,
            SystemTime::now(),
            &mut known,
            settled_verdict,
        );
        assert!(fresh.found.is_empty(), "nothing has settled yet");
        assert!(fresh.waiting);

        let settled = SystemTime::now() + SETTLE + Duration::from_secs(1);
        let scan = scan(tmp.path(), since, settled, &mut known, settled_verdict);
        let mut found = names(&scan.found);
        found.sort();
        assert_eq!(found, ["Report.pdf", "Unzipped"]);
        assert!(scan.waiting, "the download may still finish");
    }

    #[test]
    fn in_progress_downloads_are_dropped_after_the_grace_period() {
        let path = Path::new("/x/file.crdownload");
        let metadata = fs::metadata(std::env::temp_dir()).unwrap();
        assert_eq!(
            settled_verdict(path, &metadata, Duration::from_secs(5)),
            Verdict::Wait
        );
        assert_eq!(settled_verdict(path, &metadata, GRACE * 2), Verdict::Skip);
        assert_eq!(
            settled_verdict(Path::new("/x/file.pdf"), &metadata, SETTLE * 2),
            Verdict::Take
        );
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
            |_, _, _| Verdict::Take,
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
            |_, _, _| Verdict::Take,
        );
        assert!(scan.found.is_empty());
    }
}
