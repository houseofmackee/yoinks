//! User settings, stored as `settings.json` in the data folder. A
//! preferences window arrives in Phase 8; until then the file can be edited
//! by hand (Yoinks writes the defaults on first launch).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::hotkey::Shortcut;
use crate::shelf::ShelfEdge;

/// When the shelf appears on its own during a drag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppearMode {
    /// Only show the shelf from the menu bar or the keyboard shortcut.
    Off,
    /// Slide in at the screen edge as soon as a drag starts.
    #[default]
    AtEdge,
    /// Slide in when the cursor approaches the shelf's screen edge.
    NearEdge,
    /// Appear next to the cursor.
    AtCursor,
    /// Appear next to the cursor when the pointer is shaken mid-drag.
    OnShake,
}

/// Where the user dragged the shelf to: the bottom-left corner of the shelf
/// in screen coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShelfPosition {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub shelf_edge: ShelfEdge,
    /// Set when the user drags the shelf somewhere; it then appears there
    /// instead of at `shelf_edge`. `null` puts it back at the edge.
    pub shelf_position: Option<ShelfPosition>,
    /// Slide the shelf away once it is empty and the pointer leaves it. A
    /// shelf with items stays until the user hides it. (Saved as
    /// `hide_after_use` for compatibility with existing settings files.)
    pub hide_after_use: bool,
    pub appear_on_drag: AppearMode,
    /// Show the tray icon in the menu bar. When hidden, the same menu is
    /// available from the gear button at the bottom of the shelf.
    pub show_menu_bar_icon: bool,
    /// Keep entries on the shelf after dragging them out.
    pub keep_after_drag_out: bool,
    /// Bundle identifiers of apps whose drags never bring up the shelf.
    pub ignored_apps: Vec<String>,
    /// Global shortcut, e.g. `"control+option+Y"`; `null` turns it off.
    /// Press: show or hide the shelf. Long press: recall the last dragged
    /// item. Double press: add the clipboard to the shelf.
    pub hotkey: Option<String>,
    /// Record copied text, links, images and files.
    pub clipboard_history: bool,
    /// How many unpinned clips to keep.
    pub clipboard_history_size: usize,
    /// Bundle identifiers of apps whose copies are never recorded (password
    /// managers are always skipped).
    pub clipboard_ignored_apps: Vec<String>,
    /// Skip copies that look like passwords, API keys or tokens.
    pub clipboard_skip_secrets: bool,
    /// Remove unpinned entries this many hours after they were added;
    /// `null` keeps them until removed.
    pub shelf_expiry_hours: Option<u32>,
    /// Add new screenshots and screen recordings to the shelf.
    pub add_screenshots: bool,
    /// Tint the shelf's background with its color tag.
    pub tint_shelf: bool,
    /// Take drops on the notch (Macs with a notch).
    pub notch_drop: bool,
    /// Folders whose new files and folders are added to the shelf.
    pub watched_folders: Vec<PathBuf>,
    /// On battery or in Low Power Mode, poll less often and make no new
    /// thumbnails.
    pub save_energy_on_battery: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            shelf_edge: ShelfEdge::default(),
            shelf_position: None,
            hide_after_use: true,
            appear_on_drag: AppearMode::default(),
            show_menu_bar_icon: true,
            keep_after_drag_out: false,
            ignored_apps: Vec::new(),
            hotkey: Some(DEFAULT_HOTKEY.to_owned()),
            clipboard_history: true,
            clipboard_history_size: 50,
            clipboard_ignored_apps: Vec::new(),
            clipboard_skip_secrets: true,
            shelf_expiry_hours: None,
            add_screenshots: false,
            watched_folders: Vec::new(),
            tint_shelf: true,
            notch_drop: true,
            save_energy_on_battery: false,
        }
    }
}

pub const DEFAULT_HOTKEY: &str = "control+option+Y";

impl Settings {
    /// Reads `path`, filling missing fields with defaults. A missing file is
    /// created with the defaults; an unreadable one is reported and ignored.
    pub fn load(path: &Path) -> Self {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                eprintln!("yoinks: ignoring invalid {}: {error}", path.display());
                Self::default()
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let settings = Self::default();
                if let Err(error) = settings.save(path) {
                    eprintln!("yoinks: could not write {}: {error}", path.display());
                }
                settings
            }
            Err(error) => {
                eprintln!("yoinks: could not read {}: {error}", path.display());
                Self::default()
            }
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_vec_pretty(self)?)
    }

    /// The configured shortcut, if any. An invalid one is reported and
    /// treated as unset.
    pub fn shortcut(&self) -> Option<Shortcut> {
        let text = self.hotkey.as_deref()?;
        Shortcut::parse(text)
            .inspect_err(|error| eprintln!("yoinks: ignoring hotkey setting: {error}"))
            .ok()
    }

    /// How old an unpinned entry may get, if expiry is on.
    pub fn shelf_max_age(&self) -> Option<std::time::Duration> {
        self.shelf_expiry_hours
            .filter(|hours| *hours > 0)
            .map(|hours| std::time::Duration::from_secs(u64::from(hours) * 3600))
    }

    pub fn ignores_app(&self, bundle_id: &str) -> bool {
        self.ignored_apps.iter().any(|ignored| ignored == bundle_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_writes_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        assert_eq!(Settings::load(&path), Settings::default());
        let written = fs::read_to_string(&path).unwrap();
        assert!(written.contains("\"appear_on_drag\": \"at_edge\""));
        assert!(written.contains("\"shelf_edge\": \"right\""));
        assert!(written.contains("\"hotkey\": \"control+option+Y\""));
        assert!(written.contains("\"show_menu_bar_icon\": true"));
    }

    #[test]
    fn partial_file_keeps_defaults_for_the_rest() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        fs::write(
            &path,
            r#"{"appear_on_drag": "near_edge", "ignored_apps": ["com.apple.Safari"], "future": 1}"#,
        )
        .unwrap();
        let settings = Settings::load(&path);
        assert_eq!(settings.appear_on_drag, AppearMode::NearEdge);
        assert_eq!(settings.shelf_edge, ShelfEdge::Right);
        assert!(settings.ignores_app("com.apple.Safari"));
        assert!(!settings.ignores_app("com.apple.finder"));
    }

    #[test]
    fn hotkey_can_be_disabled_or_invalid() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        assert_eq!(
            Settings::load(&path).shortcut().unwrap().to_string(),
            DEFAULT_HOTKEY
        );
        fs::write(&path, r#"{"hotkey": null}"#).unwrap();
        assert_eq!(Settings::load(&path).shortcut(), None);
        fs::write(&path, r#"{"hotkey": "control+Nope"}"#).unwrap();
        let settings = Settings::load(&path);
        assert_eq!(settings.shortcut(), None, "invalid hotkey is ignored");
        assert_eq!(
            settings.appear_on_drag,
            AppearMode::AtEdge,
            "rest still loads"
        );
    }

    #[test]
    fn expiry_is_off_unless_set() {
        let mut settings = Settings::default();
        assert_eq!(settings.shelf_max_age(), None);
        settings.shelf_expiry_hours = Some(0);
        assert_eq!(settings.shelf_max_age(), None, "0 means off too");
        settings.shelf_expiry_hours = Some(24);
        assert_eq!(settings.shelf_max_age().unwrap().as_secs(), 86_400);
    }

    #[test]
    fn invalid_file_falls_back_to_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        fs::write(&path, "{ not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
    }
}
