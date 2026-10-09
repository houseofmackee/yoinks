//! Disk usage of the data folder, for the Storage tab.

use std::fs;
use std::path::Path;

/// Total size of the files under `path` (0 if it does not exist). Symbolic
/// links are not followed.
pub fn dir_size(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if !metadata.is_dir() {
        return metadata.len();
    }
    fs::read_dir(path)
        .map(|entries| entries.flatten().map(|entry| dir_size(&entry.path())).sum())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_nested_files() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("a/b")).unwrap();
        fs::write(tmp.path().join("one"), [0u8; 10]).unwrap();
        fs::write(tmp.path().join("a/b/two"), [0u8; 32]).unwrap();
        assert_eq!(dir_size(tmp.path()), 42);
        assert_eq!(dir_size(&tmp.path().join("missing")), 0);
    }
}
