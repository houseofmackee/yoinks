//! Yoinks: a drag-and-drop shelf for macOS.
//!
//! Launches as a menu bar (accessory) app. See PLAN.md for the roadmap.

#[cfg(not(target_os = "macos"))]
compile_error!("yoinks only supports macOS");

mod app;
mod drag;
mod paths;
mod settings;
mod shelf;
mod ui;

use objc2::MainThreadMarker;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

use app::AppDelegate;

fn main() {
    let mtm = MainThreadMarker::new().expect("must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    // Accessory: no Dock icon, no main menu bar takeover.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    // NSApplication holds its delegate weakly; `delegate` lives until `run`
    // returns, which is for the lifetime of the process.
    let delegate = AppDelegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    app.run();
}
