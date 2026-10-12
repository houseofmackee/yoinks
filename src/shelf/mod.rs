//! The shelf: a floating panel that holds dropped items.

mod actions;
mod bookmark;
mod describe;
pub mod geometry;
mod item;
mod materialize;
mod menu_button;
mod model;
mod ocr;
mod pasteboard;
mod promise;
mod prompt;
mod recent;
mod search;
mod store;
mod thumbnail;
mod tile;
mod view;
mod window;

pub use describe::{format_bytes, tilde};
pub use geometry::ShelfEdge;
pub use item::{Entry, ItemKind, file_name, unix_now};
pub use materialize::{Content, text_title};
pub use model::ShelfModel;
pub use pasteboard::{Dropped, read as read_pasteboard};
pub use window::{HIDE_AFTER_ADD, Shelf};
