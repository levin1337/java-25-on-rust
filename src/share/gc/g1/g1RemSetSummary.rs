/*
 * Port of OpenJDK 25 HotSpot G1RemSetSummary
 * (share/gc/g1/g1RemSetSummary.hpp).
 *
 * Per-thread remembered-set summaries accumulated during refinement.
 * In HotSpot each G1RemSetSummary belongs to a DirtyCardQueueSet
 * thread and is protected by a semaphore pair so refinement threads
 * can add while the pause iterates. The Rust port keeps the per-
 * thread summary records; synchronization is left to the caller.
 */

/// `G1RemSetSummary::DirtyCard` equivalent: a card plus optional
/// region grouping info.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemSetDirtyCard {
    /// Absolute card index.
    pub card_index: u32,
    /// Region index owning the card (for grouping/coarsening).
    pub region_index: u32,
    /// True if the card was added while its region was optional.
    pub is_optional: bool,
}

/// Per-thread summary counters.
#[derive(Debug, Default, Clone)]
pub struct RemSetThreadSummary {
    /// Total dirty cards added by this thread.
    pub num_cards: u64,
    /// Cards belonging to optional regions.
    pub num_optional_cards: u64,
    /// Cards coarsened into full regions during refinement.
    pub num_coarsened: u64,
}

impl RemSetThreadSummary {
    pub fn add_card(&mut self, is_optional: bool) {
        self.num_cards += 1;
        if is_optional {
            self.num_optional_cards += 1;
        }
    }

    pub fn add_coarsened(&mut self) {
        self.num_coarsened += 1;
    }
}

/// `G1RemSetSummary`: collection of per-thread summaries.
#[derive(Debug, Default)]
pub struct G1RemSetSummary {
    /// One entry per refinement/GC worker thread.
    summaries: Vec<RemSetThreadSummary>,
    /// Total cards currently recorded (sum over summaries).
    total_cards: u64,
}

impl G1RemSetSummary {
    pub fn new(num_threads: usize) -> Self {
        Self {
            summaries: vec![RemSetThreadSummary::default(); num_threads],
            total_cards: 0,
        }
    }

    #[inline]
    pub fn summary(&self, thread: usize) -> Option<&RemSetThreadSummary> {
        self.summaries.get(thread)
    }

    #[inline]
    pub fn summary_mut(&mut self, thread: usize) -> Option<&mut RemSetThreadSummary> {
        self.summaries.get_mut(thread)
    }

    /// `add_card`: record a dirty card for `thread`.
    pub fn add_card(&mut self, thread: usize, is_optional: bool) {
        if let Some(s) = self.summaries.get_mut(thread) {
            s.add_card(is_optional);
            self.total_cards += 1;
        }
    }

    /// `merge`: fold another summary into this one (used when a
    /// refinement thread finishes).
    pub fn merge(&mut self, other: &G1RemSetSummary) {
        for (i, s) in other.summaries.iter().enumerate() {
            if let Some(dst) = self.summaries.get_mut(i) {
                dst.num_cards += s.num_cards;
                dst.num_optional_cards += s.num_optional_cards;
                dst.num_coarsened += s.num_coarsened;
            }
        }
        self.total_cards += other.total_cards;
    }

    /// `reset`: clears all summaries after a GC cycle.
    pub fn reset(&mut self) {
        for s in &mut self.summaries {
            *s = RemSetThreadSummary::default();
        }
        self.total_cards = 0;
    }

    /// `total_cards` / `total_occured`.
    #[inline]
    pub fn total_cards(&self) -> u64 {
        self.total_cards
    }

    /// `num_cards` across all threads.
    pub fn num_cards(&self) -> u64 {
        self.summaries.iter().map(|s| s.num_cards).sum()
    }

    /// `num_optional_cards`.
    pub fn num_optional_cards(&self) -> u64 {
        self.summaries.iter().map(|s| s.num_optional_cards).sum()
    }

    /// `is_empty`.
    pub fn is_empty(&self) -> bool {
        self.total_cards == 0
    }

    pub fn num_threads(&self) -> usize {
        self.summaries.len()
    }

    /// Iterate per-thread summaries.
    pub fn iter(&self) -> impl Iterator<Item = &RemSetThreadSummary> {
        self.summaries.iter()
    }
}
