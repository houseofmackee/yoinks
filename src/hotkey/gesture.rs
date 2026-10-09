//! Turns hotkey presses and releases into gestures: a press, a long press
//! or a double press. Pure logic, so it can be unit tested; the caller
//! supplies the time and calls `tick` at `next_deadline`.

use std::time::{Duration, Instant};

/// Holding the shortcut at least this long is a long press.
pub const LONG_PRESS: Duration = Duration::from_millis(500);
/// A second press within this time after releasing is a double press. A
/// single press is only reported once this has passed.
pub const DOUBLE_PRESS: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    Press,
    LongPress,
    DoublePress,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Idle,
    /// Down since `since`, could still become a long press.
    Held { since: Instant },
    /// Released at `at`; a second press soon makes it a double press.
    Released { at: Instant },
    /// A gesture fired while the key is still down; wait for release.
    Spent,
}

#[derive(Default)]
pub struct GestureTracker {
    state: State,
}

impl GestureTracker {
    pub fn key_down(&mut self, now: Instant) -> Option<Gesture> {
        match self.state {
            State::Idle => {
                self.state = State::Held { since: now };
                None
            }
            State::Released { at } if now.duration_since(at) <= DOUBLE_PRESS => {
                self.state = State::Spent;
                Some(Gesture::DoublePress)
            }
            State::Released { .. } => {
                // The earlier press was never resolved by `tick`; report it
                // now and start over.
                self.state = State::Held { since: now };
                Some(Gesture::Press)
            }
            // Key repeat while held.
            State::Held { .. } | State::Spent => None,
        }
    }

    pub fn key_up(&mut self, now: Instant) -> Option<Gesture> {
        match self.state {
            State::Held { since } if now.duration_since(since) >= LONG_PRESS => {
                self.state = State::Idle;
                Some(Gesture::LongPress)
            }
            State::Held { .. } => {
                self.state = State::Released { at: now };
                None
            }
            State::Spent => {
                self.state = State::Idle;
                None
            }
            State::Idle | State::Released { .. } => None,
        }
    }

    pub fn tick(&mut self, now: Instant) -> Option<Gesture> {
        match self.state {
            State::Held { since } if now.duration_since(since) >= LONG_PRESS => {
                self.state = State::Spent;
                Some(Gesture::LongPress)
            }
            State::Released { at } if now.duration_since(at) > DOUBLE_PRESS => {
                self.state = State::Idle;
                Some(Gesture::Press)
            }
            _ => None,
        }
    }

    /// When `tick` should next be called, if a gesture is pending.
    pub fn next_deadline(&self) -> Option<Instant> {
        match self.state {
            State::Held { since } => Some(since + LONG_PRESS),
            State::Released { at } => Some(at + DOUBLE_PRESS + Duration::from_millis(1)),
            State::Idle | State::Spent => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(start: Instant, millis: u64) -> Instant {
        start + Duration::from_millis(millis)
    }

    #[test]
    fn single_press_resolves_after_double_press_window() {
        let t = Instant::now();
        let mut g = GestureTracker::default();
        assert_eq!(g.key_down(t), None);
        assert_eq!(g.key_up(ms(t, 80)), None);
        assert_eq!(
            g.tick(ms(t, 200)),
            None,
            "still within the double-press window"
        );
        let deadline = g.next_deadline().unwrap();
        assert_eq!(g.tick(deadline), Some(Gesture::Press));
        assert_eq!(g.next_deadline(), None);
    }

    #[test]
    fn double_press_fires_on_second_press() {
        let t = Instant::now();
        let mut g = GestureTracker::default();
        g.key_down(t);
        g.key_up(ms(t, 80));
        assert_eq!(g.key_down(ms(t, 250)), Some(Gesture::DoublePress));
        assert_eq!(g.tick(ms(t, 2000)), None, "no extra single press");
        assert_eq!(g.key_up(ms(t, 300)), None);
        assert_eq!(g.next_deadline(), None);
    }

    #[test]
    fn long_press_fires_while_held_and_once() {
        let t = Instant::now();
        let mut g = GestureTracker::default();
        g.key_down(t);
        assert_eq!(g.next_deadline(), Some(ms(t, 500)));
        assert_eq!(g.tick(ms(t, 499)), None);
        assert_eq!(g.tick(ms(t, 500)), Some(Gesture::LongPress));
        assert_eq!(g.key_down(ms(t, 600)), None, "key repeat is ignored");
        assert_eq!(g.key_up(ms(t, 900)), None);
        assert_eq!(g.tick(ms(t, 2000)), None);
    }

    #[test]
    fn long_press_detected_on_release_if_tick_was_late() {
        let t = Instant::now();
        let mut g = GestureTracker::default();
        g.key_down(t);
        assert_eq!(g.key_up(ms(t, 700)), Some(Gesture::LongPress));
    }

    #[test]
    fn slow_second_press_reports_the_first_then_starts_over() {
        let t = Instant::now();
        let mut g = GestureTracker::default();
        g.key_down(t);
        g.key_up(ms(t, 50));
        // `tick` never ran (e.g. a busy main thread).
        assert_eq!(g.key_down(ms(t, 900)), Some(Gesture::Press));
        g.key_up(ms(t, 950));
        assert_eq!(g.tick(ms(t, 1300)), Some(Gesture::Press));
    }
}
