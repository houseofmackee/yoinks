//! Recognizes a shake of the pointer during a drag: quick back-and-forth
//! horizontal movement. Pure logic, so it can be unit tested.

use std::collections::VecDeque;

/// Travel between two turns for them to count, in points.
const MIN_TRAVEL: f64 = 24.0;
/// Turns needed within `WINDOW`: four make two full back-and-forths.
const TURNS: usize = 4;
/// Seconds within which the turns must happen.
const WINDOW: f64 = 0.7;

#[derive(Default)]
pub struct ShakeDetector {
    /// Where the current stretch of movement started.
    anchor: Option<f64>,
    /// The furthest point reached in the current direction.
    extreme: f64,
    /// +1 moving right, -1 moving left, 0 not yet known.
    direction: i8,
    /// When the recent turns happened.
    turns: VecDeque<f64>,
}

impl ShakeDetector {
    /// Starts over, for a new drag.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Feeds a pointer position (`x` in points, `time` in seconds). Returns
    /// true once the movement amounts to a shake; then starts over.
    pub fn moved(&mut self, x: f64, time: f64) -> bool {
        let Some(anchor) = self.anchor else {
            self.anchor = Some(x);
            self.extreme = x;
            return false;
        };
        let direction = match self.direction {
            0 if (x - anchor).abs() >= MIN_TRAVEL => (x - anchor).signum() as i8,
            0 => return false,
            d => d,
        };
        if self.direction == 0 {
            self.direction = direction;
            self.extreme = x;
            return false;
        }
        let beyond = (x - self.extreme) * f64::from(direction) > 0.0;
        if beyond {
            self.extreme = x;
        } else if (self.extreme - x).abs() >= MIN_TRAVEL {
            // Turned around far enough: one turn.
            self.turns.push_back(time);
            self.direction = -direction;
            self.extreme = x;
        }
        while self.turns.front().is_some_and(|t| time - t > WINDOW) {
            self.turns.pop_front();
        }
        if self.turns.len() >= TURNS {
            self.reset();
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Moves between the points given, 10 points and 10 ms per step.
    fn path(detector: &mut ShakeDetector, points: &[f64], start: f64) -> (bool, f64) {
        let mut time = start;
        let mut x = points[0];
        let mut shook = detector.moved(x, time);
        for &target in &points[1..] {
            while (target - x).abs() > 0.5 {
                x += (target - x).clamp(-10.0, 10.0);
                time += 0.01;
                shook |= detector.moved(x, time);
            }
        }
        (shook, time)
    }

    #[test]
    fn quick_back_and_forth_is_a_shake() {
        let mut d = ShakeDetector::default();
        let (shook, _) = path(&mut d, &[0.0, 60.0, 0.0, 60.0, 0.0, 60.0], 0.0);
        assert!(shook);
    }

    #[test]
    fn a_straight_drag_is_not() {
        let mut d = ShakeDetector::default();
        assert!(!path(&mut d, &[0.0, 800.0], 0.0).0);
    }

    #[test]
    fn small_wiggles_are_not() {
        let mut d = ShakeDetector::default();
        let wiggle = [0.0, 15.0, 0.0, 15.0, 0.0, 15.0, 0.0, 15.0];
        assert!(!path(&mut d, &wiggle, 0.0).0);
    }

    #[test]
    fn slow_turns_are_not() {
        let mut d = ShakeDetector::default();
        let mut time = 0.0;
        let mut shook = false;
        // One turn per second: never four within the window.
        for (i, x) in [0.0, 100.0, 0.0, 100.0, 0.0, 100.0].into_iter().enumerate() {
            time = i as f64;
            shook |= d.moved(x, time);
        }
        assert!(!shook, "at {time}");
    }

    #[test]
    fn reset_forgets_earlier_turns() {
        let mut d = ShakeDetector::default();
        let (shook, time) = path(&mut d, &[0.0, 60.0, 0.0, 60.0], 0.0);
        assert!(!shook);
        d.reset();
        assert!(!path(&mut d, &[0.0, 60.0, 0.0], time).0);
    }
}
