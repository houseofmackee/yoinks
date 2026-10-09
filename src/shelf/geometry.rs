//! Pure layout math for the shelf window. Kept free of AppKit calls so it can
//! be unit tested.

use objc2_foundation::{NSPoint, NSRect, NSSize};

/// Which side of the screen the shelf slides in from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShelfEdge {
    Left,
    #[default]
    Right,
}

pub const SHELF_WIDTH: f64 = 112.0;
pub const SHELF_MIN_HEIGHT: f64 = 160.0;
pub const SHELF_MAX_HEIGHT: f64 = 440.0;
/// Gap between the shelf and the edge of the visible screen area.
pub const EDGE_MARGIN: f64 = 8.0;
/// Fraction of the visible screen height the shelf tries to occupy.
const HEIGHT_FRACTION: f64 = 0.6;

/// Where the shelf sits when shown, and where it slides to when hidden.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShelfFrames {
    pub shown: NSRect,
    pub hidden: NSRect,
}

/// Computes the shelf frames for a screen's visible frame (the area excluding
/// the menu bar, notch and Dock).
pub fn shelf_frames(visible: NSRect, edge: ShelfEdge) -> ShelfFrames {
    let available = (visible.size.height - 2.0 * EDGE_MARGIN).max(0.0);
    let height = (visible.size.height * HEIGHT_FRACTION)
        .clamp(SHELF_MIN_HEIGHT, SHELF_MAX_HEIGHT)
        .min(available);
    let y = visible.origin.y + (visible.size.height - height) / 2.0;
    let size = NSSize::new(SHELF_WIDTH, height);

    let min_x = visible.origin.x;
    let max_x = visible.origin.x + visible.size.width;
    let (shown_x, hidden_x) = match edge {
        ShelfEdge::Left => (min_x + EDGE_MARGIN, min_x - SHELF_WIDTH),
        ShelfEdge::Right => (max_x - EDGE_MARGIN - SHELF_WIDTH, max_x),
    };

    ShelfFrames {
        shown: NSRect::new(NSPoint::new(shown_x, y), size),
        hidden: NSRect::new(NSPoint::new(hidden_x, y), size),
    }
}

/// Returns the index of the first screen frame containing `point`. Edges are
/// inclusive so the cursor pinned to the top or right of a screen still counts.
pub fn screen_containing(screens: &[NSRect], point: NSPoint) -> Option<usize> {
    screens.iter().position(|frame| {
        point.x >= frame.origin.x
            && point.x <= frame.origin.x + frame.size.width
            && point.y >= frame.origin.y
            && point.y <= frame.origin.y + frame.size.height
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
    }

    #[test]
    fn right_edge_frames() {
        let frames = shelf_frames(rect(0.0, 0.0, 1440.0, 875.0), ShelfEdge::Right);
        assert_eq!(
            frames.shown.size,
            NSSize::new(SHELF_WIDTH, SHELF_MAX_HEIGHT)
        );
        assert_eq!(frames.shown.origin.x, 1440.0 - EDGE_MARGIN - SHELF_WIDTH);
        assert_eq!(frames.hidden.origin.x, 1440.0);
        assert_eq!(frames.shown.origin.y, (875.0 - SHELF_MAX_HEIGHT) / 2.0);
        assert_eq!(frames.shown.origin.y, frames.hidden.origin.y);
    }

    #[test]
    fn left_edge_frames_on_offset_screen() {
        // A secondary display to the left of the main one.
        let frames = shelf_frames(rect(-1920.0, 100.0, 1920.0, 1000.0), ShelfEdge::Left);
        assert_eq!(frames.shown.origin.x, -1920.0 + EDGE_MARGIN);
        assert_eq!(frames.hidden.origin.x, -1920.0 - SHELF_WIDTH);
        assert_eq!(
            frames.shown.origin.y,
            100.0 + (1000.0 - SHELF_MAX_HEIGHT) / 2.0
        );
    }

    #[test]
    fn height_scales_with_screen() {
        let frames = shelf_frames(rect(0.0, 0.0, 800.0, 500.0), ShelfEdge::Right);
        assert_eq!(frames.shown.size.height, 300.0);
    }

    #[test]
    fn height_never_exceeds_visible_area() {
        let frames = shelf_frames(rect(0.0, 0.0, 800.0, 100.0), ShelfEdge::Right);
        assert_eq!(frames.shown.size.height, 100.0 - 2.0 * EDGE_MARGIN);
    }

    #[test]
    fn finds_screen_under_point() {
        let screens = [
            rect(0.0, 0.0, 1440.0, 900.0),
            rect(1440.0, 0.0, 1920.0, 1080.0),
        ];
        assert_eq!(
            screen_containing(&screens, NSPoint::new(100.0, 100.0)),
            Some(0)
        );
        assert_eq!(
            screen_containing(&screens, NSPoint::new(2000.0, 1000.0)),
            Some(1)
        );
        assert_eq!(
            screen_containing(&screens, NSPoint::new(100.0, 900.0)),
            Some(0)
        );
        assert_eq!(screen_containing(&screens, NSPoint::new(-5.0, 10.0)), None);
    }
}
