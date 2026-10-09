//! Pure layout math for the shelf window. Kept free of AppKit calls so it can
//! be unit tested.

use objc2_foundation::{NSPoint, NSRect, NSSize};
use serde::{Deserialize, Serialize};

/// Which side of the screen the shelf slides in from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

/// Gap between the cursor and the shelf when it appears at the cursor.
pub const CURSOR_GAP: f64 = 24.0;
/// How close (in points) the cursor must get to the shelf's screen edge in
/// "near edge" mode.
pub const NEAR_EDGE_DISTANCE: f64 = 40.0;

/// Frames for showing the shelf beside the cursor: to its right, or to its
/// left when there is no room, vertically centered on it and kept inside the
/// visible frame. It fades rather than slides, so both frames are the same.
pub fn cursor_frames(visible: NSRect, cursor: NSPoint, edge: ShelfEdge) -> ShelfFrames {
    let size = shelf_frames(visible, edge).shown.size;
    let min_x = visible.origin.x + EDGE_MARGIN;
    let max_x = visible.origin.x + visible.size.width - EDGE_MARGIN - size.width;
    let min_y = visible.origin.y + EDGE_MARGIN;
    let max_y = visible.origin.y + visible.size.height - EDGE_MARGIN - size.height;

    let right = cursor.x + CURSOR_GAP;
    let x = if right <= max_x {
        right
    } else {
        cursor.x - CURSOR_GAP - size.width
    };
    let y = cursor.y - size.height / 2.0;
    let frame = NSRect::new(
        NSPoint::new(
            x.clamp(min_x, max_x.max(min_x)),
            y.clamp(min_y, max_y.max(min_y)),
        ),
        size,
    );
    ShelfFrames {
        shown: frame,
        hidden: frame,
    }
}

/// True when `point` is within `NEAR_EDGE_DISTANCE` of the `edge` side of
/// `screen` (the full screen frame, so the Dock area counts too).
pub fn near_edge(screen: NSRect, point: NSPoint, edge: ShelfEdge) -> bool {
    match edge {
        ShelfEdge::Left => point.x <= screen.origin.x + NEAR_EDGE_DISTANCE,
        ShelfEdge::Right => point.x >= screen.origin.x + screen.size.width - NEAR_EDGE_DISTANCE,
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

    #[test]
    fn cursor_frames_sit_beside_the_cursor() {
        let visible = rect(0.0, 0.0, 1440.0, 875.0);
        let frames = cursor_frames(visible, NSPoint::new(500.0, 400.0), ShelfEdge::Right);
        assert_eq!(frames.shown, frames.hidden);
        assert_eq!(frames.shown.origin.x, 500.0 + CURSOR_GAP);
        assert_eq!(frames.shown.origin.y, 400.0 - SHELF_MAX_HEIGHT / 2.0);
    }

    #[test]
    fn cursor_frames_flip_left_and_clamp_near_screen_edges() {
        let visible = rect(0.0, 0.0, 1440.0, 875.0);
        let frames = cursor_frames(visible, NSPoint::new(1430.0, 5.0), ShelfEdge::Right);
        assert_eq!(frames.shown.origin.x, 1430.0 - CURSOR_GAP - SHELF_WIDTH);
        assert_eq!(frames.shown.origin.y, EDGE_MARGIN);

        let frames = cursor_frames(visible, NSPoint::new(1.0, 870.0), ShelfEdge::Left);
        assert_eq!(frames.shown.origin.x, 1.0 + CURSOR_GAP);
        assert_eq!(
            frames.shown.origin.y,
            875.0 - EDGE_MARGIN - SHELF_MAX_HEIGHT
        );
    }

    #[test]
    fn near_edge_checks_the_shelf_side() {
        let screen = rect(1440.0, 0.0, 1920.0, 1080.0);
        assert!(near_edge(
            screen,
            NSPoint::new(3350.0, 10.0),
            ShelfEdge::Right
        ));
        assert!(!near_edge(
            screen,
            NSPoint::new(3300.0, 10.0),
            ShelfEdge::Right
        ));
        assert!(near_edge(
            screen,
            NSPoint::new(1460.0, 10.0),
            ShelfEdge::Left
        ));
        assert!(!near_edge(
            screen,
            NSPoint::new(1500.0, 10.0),
            ShelfEdge::Left
        ));
    }
}
