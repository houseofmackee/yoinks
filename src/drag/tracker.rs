//! Recognizes system-wide drags from global mouse events. Pure logic, no
//! AppKit, so it can be unit tested.
//!
//! A press followed by movement only counts as a drag once the system drag
//! pasteboard changes. That rules out moving windows, selecting text and
//! rubber-band selections, which never touch the drag pasteboard.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Idle,
    /// The mouse is down; `baseline` is the drag pasteboard's change count
    /// at that moment.
    Pressed {
        baseline: isize,
    },
    Dragging,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Update {
    /// Nothing to do.
    None,
    /// A drag just started.
    Started,
    /// An ongoing drag moved.
    Moved,
}

#[derive(Default)]
pub struct DragTracker {
    state: State,
}

impl DragTracker {
    pub fn mouse_down(&mut self, change_count: isize) {
        self.state = State::Pressed {
            baseline: change_count,
        };
    }

    /// Feed every global mouse-dragged event. `change_count` is only read
    /// while waiting for a drag to start.
    pub fn mouse_dragged(&mut self, change_count: impl FnOnce() -> isize) -> Update {
        match self.state {
            State::Pressed { baseline } if change_count() != baseline => {
                self.state = State::Dragging;
                Update::Started
            }
            State::Dragging => Update::Moved,
            State::Idle | State::Pressed { .. } => Update::None,
        }
    }

    /// The mouse button was released. Returns true if a drag just ended.
    pub fn mouse_released(&mut self) -> bool {
        let was_dragging = self.state == State::Dragging;
        self.state = State::Idle;
        was_dragging
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_starts_when_pasteboard_changes() {
        let mut tracker = DragTracker::default();
        tracker.mouse_down(7);
        assert_eq!(tracker.mouse_dragged(|| 7), Update::None);
        assert_eq!(tracker.mouse_dragged(|| 8), Update::Started);
        assert_eq!(tracker.mouse_dragged(|| unreachable!()), Update::Moved);
        assert!(tracker.mouse_released());
        assert_eq!(tracker.mouse_dragged(|| 9), Update::None, "released");
    }

    #[test]
    fn moving_a_window_is_not_a_drag() {
        let mut tracker = DragTracker::default();
        tracker.mouse_down(3);
        for _ in 0..10 {
            assert_eq!(tracker.mouse_dragged(|| 3), Update::None);
        }
        assert!(!tracker.mouse_released());
    }

    #[test]
    fn movement_without_a_press_is_ignored() {
        // E.g. the app launched while the button was already down.
        let mut tracker = DragTracker::default();
        assert_eq!(tracker.mouse_dragged(|| 99), Update::None);
    }

    #[test]
    fn each_press_takes_a_new_baseline() {
        let mut tracker = DragTracker::default();
        tracker.mouse_down(1);
        assert_eq!(tracker.mouse_dragged(|| 2), Update::Started);
        tracker.mouse_released();
        // The count stays at 2 for a plain click-and-move afterwards.
        tracker.mouse_down(2);
        assert_eq!(tracker.mouse_dragged(|| 2), Update::None);
    }
}
