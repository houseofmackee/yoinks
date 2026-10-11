//! Clipboard history: recording copies, privacy filters, and the history
//! panel.

mod filters;
mod history;
mod panel;
mod secrets;
mod watcher;

pub use panel::HistoryList;
pub use watcher::ClipboardHistory;
