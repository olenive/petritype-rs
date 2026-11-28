//! [`CheckpointCache`]: the fold of a sequence to any index from the
//! nearest state kept, which the players of a record and of a log share.

use std::collections::BTreeMap;

/// A fold of a sequence by absolute index, from the nearest of three
/// starting points: the state the last fold ended on, a checkpoint made
/// every so many items, or the state before the first item held. A jump
/// anywhere so costs at most `every` items, and stepping forward one item
/// at a time costs one.
///
/// The checkpoints are made on the first pass over the items. A
/// checkpoint at absolute index `k` is the state before the item at `k`,
/// so it survives the items before it being dropped.
#[derive(Debug, Clone)]
pub(crate) struct CheckpointCache<T> {
    /// How many items apart the checkpoints are, at least one.
    every: u64,
    /// The state before the item of each absolute index.
    checkpoints: BTreeMap<u64, T>,
    /// The state after the item of an absolute index, as the last fold
    /// left it.
    current: Option<(u64, T)>,
}

/// Where a fold begins.
enum Start {
    /// From the state after the item at this index.
    After(u64),
    /// From the state before the item at this index.
    Before(u64),
}

impl<T: Clone> CheckpointCache<T> {
    /// A cache with a checkpoint every `every` items, at least one, and
    /// nothing folded yet.
    pub(crate) fn new(every: usize) -> Self {
        Self {
            every: every.max(1) as u64,
            checkpoints: BTreeMap::new(),
            current: None,
        }
    }

    /// How many items apart the checkpoints are.
    pub(crate) fn every(&self) -> usize {
        usize::try_from(self.every).unwrap_or(usize::MAX)
    }

    /// The state after the item at `target`, clamped to the items held,
    /// whose absolute indices run from `first` to `last`. `before_first` is
    /// the state before the item at `first`, and `step` moves a state on by
    /// the item at an absolute index. The fold starts from the state the
    /// last one ended on when that stands at or before the target, from the
    /// nearest checkpoint when that is closer, and from `before_first`
    /// when neither is there.
    ///
    /// Every index is worked out without overflow, so a sequence whose
    /// indices end at `u64::MAX` folds to its last item.
    pub(crate) fn fold(
        &mut self,
        first: u64,
        last: u64,
        target: u64,
        before_first: &T,
        mut step: impl FnMut(&mut T, u64),
    ) -> &T {
        let target = target.clamp(first, last.max(first));
        // The last fold, if it stands at or before the target with every
        // item after it held: after `first - 1` is before `first`.
        let resume = self
            .current
            .as_ref()
            .map(|(after, _)| *after)
            .filter(|&after| after <= target && after >= first.saturating_sub(1));
        // The checkpoint at `target + 1` is the state after the target.
        let checkpoint = self
            .checkpoints
            .range(first..=target.saturating_add(1))
            .next_back()
            .map(|(&before, _)| before);
        let (start, mut state) = match (resume, checkpoint) {
            (Some(after), Some(before)) if before > after.saturating_add(1) => {
                (Start::Before(before), self.checkpoints[&before].clone())
            }
            (Some(after), _) => {
                let (_, state) = self.current.take().expect("the last fold is there");
                (Start::After(after), state)
            }
            (None, Some(before)) => (Start::Before(before), self.checkpoints[&before].clone()),
            (None, None) => (Start::Before(first), before_first.clone()),
        };
        let next = match start {
            Start::After(after) => after.checked_add(1),
            Start::Before(before) => Some(before),
        };
        if let Some(next) = next {
            for k in next..=target {
                step(&mut state, k);
                if let Some(before) = k.checked_add(1)
                    && before % self.every == 0
                    && !self.checkpoints.contains_key(&before)
                {
                    self.checkpoints.insert(before, state.clone());
                }
            }
        }
        &self.current.insert((target, state)).1
    }

    /// Forgets every checkpoint before `first`, and the last fold if it
    /// ended before `first - 1`: what is left stands on items still held.
    pub(crate) fn forget_before(&mut self, first: u64) {
        self.checkpoints.retain(|&before, _| before >= first);
        if self
            .current
            .as_ref()
            .is_some_and(|(after, _)| after.saturating_add(1) < first)
        {
            self.current = None;
        }
    }

    /// Forgets every fold made so far.
    pub(crate) fn clear(&mut self) {
        self.checkpoints.clear();
        self.current = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The items' indices summed, from 0 before the first.
    fn sum(cache: &mut CheckpointCache<u64>, first: u64, last: u64, target: u64) -> u64 {
        *cache.fold(first, last, target, &0, |s, k| *s = s.wrapping_add(k))
    }

    fn straight(first: u64, target: u64) -> u64 {
        (first..=target).fold(0, |s: u64, k| s.wrapping_add(k))
    }

    #[test]
    fn a_fold_from_any_start_equals_the_straight_one() {
        let mut cache = CheckpointCache::new(4);
        for target in [9, 3, 3, 4, 17, 0, 12, 11, 30, 8, 7] {
            assert_eq!(
                sum(&mut cache, 0, 30, target),
                straight(0, target),
                "{target}"
            );
        }
        assert_eq!(sum(&mut cache, 0, 30, 1000), straight(0, 30), "clamped");
        assert_eq!(cache.every(), 4);
        // The items before 10 dropped: what is kept still stands.
        cache.forget_before(10);
        assert!(cache.checkpoints.keys().all(|&k| k >= 10));
        let kept = sum(&mut cache, 10, 30, 20);
        let mut fresh = CheckpointCache::new(4);
        assert_eq!(
            kept,
            straight(0, 20),
            "a checkpoint folded before the drop keeps what the dropped items left"
        );
        assert_eq!(sum(&mut fresh, 10, 30, 20), straight(10, 20));
    }

    #[test]
    fn indices_that_end_at_the_largest_u64_fold_without_overflow() {
        let first = u64::MAX - 5;
        let mut cache = CheckpointCache::new(2);
        assert_eq!(
            sum(&mut cache, first, u64::MAX, u64::MAX),
            straight(first, u64::MAX)
        );
        // Again at the end, from the last fold, and back from a checkpoint.
        assert_eq!(
            sum(&mut cache, first, u64::MAX, u64::MAX),
            straight(first, u64::MAX)
        );
        assert_eq!(
            sum(&mut cache, first, u64::MAX, first + 2),
            straight(first, first + 2)
        );
        sum(&mut cache, first, u64::MAX, u64::MAX);
        cache.forget_before(u64::MAX);
        assert_eq!(
            sum(&mut cache, u64::MAX, u64::MAX, 0),
            straight(first, u64::MAX),
            "the last fold stands at the one item held, and is the answer"
        );
    }
}
