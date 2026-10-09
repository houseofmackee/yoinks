//! Locations on disk.

use std::path::PathBuf;

/// `~/Library/Application Support/Yoinks`, or `$YOINKS_DATA_DIR` when set
/// (handy for testing without touching the real shelf).
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("YOINKS_DATA_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join("Library/Application Support/Yoinks")
}
