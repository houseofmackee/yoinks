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

pub use geometry::ShelfEdge;
pub use model::ShelfModel;
pub use window::Shelf;
