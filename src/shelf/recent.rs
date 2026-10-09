//! Entries recently dragged out of the shelf, kept so they can be recalled.

use std::collections::VecDeque;

use super::item::Entry;

/// How many dragged-out entries can be recalled.
pub const RECALL_LIMIT: usize = 10;

#[derive(Default)]
pub struct Recent {
    entries: VecDeque<Entry>,
}

impl Recent {
    /// Remembers `entry`, returning whatever fell off the end.
    #[must_use]
    pub fn push(&mut self, entry: Entry) -> Option<Entry> {
        self.entries.push_front(entry);
        (self.entries.len() > RECALL_LIMIT)
            .then(|| self.entries.pop_back())
            .flatten()
    }

    pub fn pop(&mut self) -> Option<Entry> {
        self.entries.pop_front()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shelf::item::Item;

    fn entry(name: &str) -> Entry {
        Entry::new(vec![Item::user_file(format!("/tmp/{name}").into(), None)])
    }

    #[test]
    fn pops_most_recent_first() {
        let mut recent = Recent::default();
        assert!(recent.push(entry("a")).is_none());
        assert!(recent.push(entry("b")).is_none());
        assert_eq!(recent.pop().unwrap().title(), "b");
        assert_eq!(recent.pop().unwrap().title(), "a");
        assert!(recent.pop().is_none());
        assert!(recent.is_empty());
    }

    #[test]
    fn evicts_oldest_beyond_limit() {
        let mut recent = Recent::default();
        for i in 0..RECALL_LIMIT {
            assert!(recent.push(entry(&i.to_string())).is_none());
        }
        let evicted = recent.push(entry("new")).unwrap();
        assert_eq!(evicted.title(), "0");
        assert_eq!(std::iter::from_fn(|| recent.pop()).count(), RECALL_LIMIT);
    }
}
