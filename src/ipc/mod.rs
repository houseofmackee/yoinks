//! Talking to the running app: the socket used by the `yoinks` command line
//! and the `yoinks://` link scheme.

pub mod client;
pub mod protocol;
pub mod server;
pub mod url;

pub use protocol::{EntryInfo, ItemInfo, Request, Response};
