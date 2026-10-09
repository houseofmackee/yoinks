//! User settings, stored as `settings.json` in the data folder. A
//! preferences window arrives in Phase 8; until then the file can be edited
//! by hand (Yoinks writes the defaults on first launch).

use std::fs;
use std::io;
use std::path::Path;

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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub shelf_edge: ShelfEdge,
    pub appear_on_drag: AppearMode,
    /// Keep entries on the shelf after dragging them out.
    pub keep_after_drag_out: bool,
    /// Bundle identifiers of apps whose drags never bring up the shelf.
    pub ignored_apps: Vec<String>,
    /// Global shortcut, e.g. `"control+option+Y"`; `null` turns it off.
    /// Press: show or hide the shelf. Long press: recall the last dragged
    /// item. Double press: add the clipboard to the shelf.
    pub hotkey: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            shelf_edge: ShelfEdge::default(),
            appear_on_drag: AppearMode::default(),
            keep_after_drag_out: false,
            ignored_apps: Vec::new(),
            hotkey: Some(DEFAULT_HOTKEY.to_owned()),
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
    fn invalid_file_falls_back_to_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        fs::write(&path, "{ not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
    }
}
