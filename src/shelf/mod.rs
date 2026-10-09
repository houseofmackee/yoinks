//! The shelf: a floating panel that holds dropped items.

mod bookmark;
mod describe;
pub mod geometry;
mod item;
mod materialize;
mod model;
mod pasteboard;
mod promise;
mod recent;
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
pub use window::Shelf;
