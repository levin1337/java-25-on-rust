/*
 * Port of OpenJDK 25 HotSpot G1RegionMarkStatsCache / G1RegionMarkStats
 * (share/gc/g1/g1RegionMarkStatsCache.hpp) — per-region marking
 * statistics collected during concurrent marking and consumed by the
 * collection-set chooser and IHOP control.
 */

/// `G1RegionMarkStats`: per-region stats for one marking cycle.
#[derive(Debug, Default, Clone, Copy)]
pub struct G1RegionMarkStats {
    /// Bytes marked live in the region.
    pub live_bytes: u64,
    /// Total marked bytes including during-marking estimate.
    pub marked_bytes: u64,
    /// Region occupancy (used bytes) at marking time.
    pub allocated_bytes: u64,
    /// Predicted (or measured) marking time contribution.
    pub mark_bytes: u64,
    /// Number of references into the young generation found.
    pub young_refs: u64,
    /// GC efficiency: reclaimed bytes per allocated byte.
    pub gc_efficiency: f64,
}

impl G1RegionMarkStats {
    /// `set_live_bytes`, `set_allocation_bytes` etc. computed.
    pub fn compute_efficiency(&mut self) {
        if self.allocated_bytes == 0 {
            self.gc_efficiency = 0.0;
        } else {
            let reclaimed = self
                .allocated_bytes
                .saturating_sub(self.live_bytes);
            self.gc_efficiency = reclaimed as f64 / self.allocated_bytes as f64;
        }
    }

    /// `is_gc_cset_candidate`: region is worth collecting if it has
    /// enough reclaimable bytes (HotSpot uses a threshold on
    /// efficiency * live bytes).
    pub fn is_gc_cset_candidate(&self, min_efficiency: f64) -> bool {
        self.gc_efficiency >= min_efficiency && self.marked_bytes > 0
    }
}

/// `G1RegionMarkStatsCache`: per-region stats indexed by region.
#[derive(Debug)]
pub struct G1RegionMarkStatsCache {
    stats: Vec<G1RegionMarkStats>,
    /// Cached max live bytes across regions (for IHOP).
    max_live_bytes: u64,
}

impl G1RegionMarkStatsCache {
    pub fn new(num_regions: usize) -> Self {
        Self { stats: vec![G1RegionMarkStats::default(); num_regions], max_live_bytes: 0 }
    }

    #[inline]
    pub fn stats(&self, region: usize) -> G1RegionMarkStats {
        self.stats.get(region).copied().unwrap_or_default()
    }

    #[inline]
    pub fn stats_mut(&mut self, region: usize) -> Option<&mut G1RegionMarkStats> {
        self.stats.get_mut(region)
    }

    /// `add_live_bytes` accumulation during marking.
    pub fn add_marked_bytes(&mut self, region: usize, bytes: u64) {
        if let Some(s) = self.stats.get_mut(region) {
            s.marked_bytes += bytes;
            s.live_bytes = s.marked_bytes;
        }
    }

    /// `set_region_live_bytes` at end of marking.
    pub fn set_live_bytes(&mut self, region: usize, live: u64, allocated: u64) {
        if let Some(s) = self.stats.get_mut(region) {
            s.live_bytes = live;
            s.allocated_bytes = allocated;
            s.mark_bytes = live;
            s.compute_efficiency();
        }
        self.refresh_max_live_bytes();
    }

    /// `clear` after a GC cycle.
    pub fn clear(&mut self) {
        for s in &mut self.stats {
            *s = G1RegionMarkStats::default();
        }
        self.max_live_bytes = 0;
    }

    fn refresh_max_live_bytes(&mut self) {
        self.max_live_bytes = self.stats.iter().map(|s| s.live_bytes).max().unwrap_or(0);
    }

    /// `max_live_bytes` used by IHOP prediction.
    #[inline]
    pub fn max_live_bytes(&self) -> u64 {
        self.max_live_bytes
    }

    /// `efficiency_of` helper for the chooser.
    #[inline]
    pub fn efficiency_of(&self, region: usize) -> f64 {
        self.stats(region).gc_efficiency
    }

    /// Regions exceeding `min_efficiency`, sorted descending by
    /// efficiency (chooser pre-filter).
    pub fn candidate_regions(&self, min_efficiency: f64) -> Vec<usize> {
        let mut out: Vec<(usize, f64)> = self
            .stats
            .iter()
            .enumerate()
            .filter(|(_, s)| s.is_gc_cset_candidate(min_efficiency))
            .map(|(i, s)| (i, s.gc_efficiency))
            .collect();
        out.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        out.into_iter().map(|(i, _)| i).collect()
    }

    pub fn num_regions(&self) -> usize {
        self.stats.len()
    }
}
