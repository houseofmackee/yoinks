//! Energy saving: with `save_energy_on_battery` on, Yoinks polls the
//! clipboard and the screenshot folder less often and makes no new
//! thumbnails while the Mac runs on battery or in Low Power Mode.

use std::cell::Cell;
use std::ffi::c_void;

use objc2_foundation::{NSProcessInfo, NSString};

/// The clipboard is read every 0.5 s; while saving energy, every 2 s.
pub const CLIPBOARD_SKIP: u32 = 4;
/// The screenshot folder is checked every second; while saving, every 5 s.
pub const SCREENSHOT_SKIP: u32 = 5;

thread_local! {
    static SAVING: Cell<bool> = const { Cell::new(false) };
}

/// Whether Yoinks is saving energy right now.
pub fn saving() -> bool {
    SAVING.with(Cell::get)
}

/// Re-reads the power state; `enabled` is the setting. Returns true if
/// saving turned on or off.
pub fn update(enabled: bool) -> bool {
    let now = enabled && (on_battery() || NSProcessInfo::processInfo().isLowPowerModeEnabled());
    SAVING.with(|saving| saving.replace(now) != now)
}

/// Whether a timer tick should do its work: every tick normally, every
/// `skip`th tick while saving energy.
pub fn due(tick: u32, skip: u32) -> bool {
    !saving() || tick.is_multiple_of(skip)
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPSCopyPowerSourcesInfo() -> *const c_void;
    fn IOPSGetProvidingPowerSourceType(snapshot: *const c_void) -> *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: *const c_void);
}

/// Whether the Mac is running on battery power.
fn on_battery() -> bool {
    // SAFETY: IOKit returns an owned snapshot (released below) and a
    // CFString borrowed from it, toll-free bridged to NSString.
    unsafe {
        let snapshot = IOPSCopyPowerSourcesInfo();
        if snapshot.is_null() {
            return false;
        }
        let kind = IOPSGetProvidingPowerSourceType(snapshot);
        let battery = !kind.is_null() && (*kind.cast::<NSString>()).to_string() == "Battery Power";
        CFRelease(snapshot);
        battery
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_skips_ticks_only_while_saving() {
        SAVING.with(|s| s.set(false));
        assert!((1..10).all(|tick| due(tick, 4)));
        SAVING.with(|s| s.set(true));
        let done: Vec<u32> = (1..10).filter(|tick| due(*tick, 4)).collect();
        assert_eq!(done, [4, 8]);
        SAVING.with(|s| s.set(false));
    }

    #[test]
    fn power_source_matches_pmset() {
        let output = std::process::Command::new("/usr/bin/pmset")
            .args(["-g", "batt"])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        assert_eq!(on_battery(), text.contains("'Battery Power'"), "{text}");
    }

    #[test]
    fn off_means_not_saving() {
        SAVING.with(|s| s.set(true));
        assert!(update(false), "turning the setting off stops saving");
        assert!(!saving());
        assert!(!update(false), "no change the second time");
    }
}
