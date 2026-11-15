/*
 * Port of OpenJDK 25 HotSpot G1CollectionSet (share/gc/g1/g1CollectionSet.hpp)
 * and G1CollectionSetChooser candidate selection.
 *
 * The collection set holds the regions selected for evacuation in the
 * current young GC. Candidates are ranked by GC efficiency
 * (reclaimed bytes per allocated byte) — the same metric used by
 * `G1CollectionSetChooser` — and optional regions may be added when
 * the young pause has spare time.
 */

use super::g1RegionMarkStatsCache::{G1RegionMarkStats, G1RegionMarkStatsCache};

/// `G1CollectionSetCandidates`-driven candidate: a region index plus
/// its mark stats used for ranking.
#[derive(Debug, Clone)]
pub struct G1CSetCandidate {
    pub region_index: usize,
    pub stats: G1RegionMarkStats,
    /// Humongous objects are candidates too (reclaimed without copy).
    pub is_humongous: bool,
    /// Old regions may be optional (added only if pause allows).
    pub is_optional: bool,
}

/// `G1CollectionSet`: the selected set of regions for this GC.
#[derive(Debug, Default)]
pub struct G1CollectionSet {
    /// Region indexes in evacuation order.
    regions: Vec<usize>,
    /// Fast membership check.
    membership: Vec<bool>,
    /// Old regions in the CSet (scanned differently).
    old_regions: Vec<usize>,
    /// Humongous candidate regions.
    humongous_regions: Vec<usize>,
    /// Optional old regions added late.
    optional_regions: Vec<usize>,
}

impl G1CollectionSet {
    pub fn new(num_regions: usize) -> Self {
        Self {
            regions: Vec::new(),
            membership: vec![false; num_regions],
            old_regions: Vec::new(),
            humongous_regions: Vec::new(),
            optional_regions: Vec::new(),
        }
    }

    #[inline]
    pub fn num_regions(&self) -> usize {
        self.regions.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    #[inline]
    pub fn contains(&self, region: usize) -> bool {
        self.membership.get(region).copied().unwrap_or(false)
    }

    #[inline]
    pub fn regions(&self) -> &[usize] {
        &self.regions
    }

    #[inline]
    pub fn old_regions(&self) -> &[usize] {
        &self.old_regions
    }

    #[inline]
    pub fn humongous_regions(&self) -> &[usize] {
        &self.humongous_regions
    }

    #[inline]
    pub fn optional_regions(&self) -> &[usize] {
        &self.optional_regions
    }

    /// `add_region`/`add_old_region`/`add_humongous_candidate`/
    /// `add_optional_region`.
    pub fn add_region(&mut self, region: usize, kind: CSetRegionKind) {
        if self.contains(region) {
            return;
        }
        if region < self.membership.len() {
            self.membership[region] = true;
        }
        self.regions.push(region);
        match kind {
            CSetRegionKind::Young => {}
            CSetRegionKind::Old => self.old_regions.push(region),
            CSetRegionKind::Humongous => self.humongous_regions.push(region),
            CSetRegionKind::OptionalOld => self.optional_regions.push(region),
        }
    }

    /// `remove_region` (used when evacuation fails and the region is
    /// reverted).
    pub fn remove_region(&mut self, region: usize) {
        if region < self.membership.len() {
            self.membership[region] = false;
        }
        self.regions.retain(|&r| r != region);
        self.old_regions.retain(|&r| r != region);
        self.humongous_regions.retain(|&r| r != region);
        self.optional_regions.retain(|&r| r != region);
    }

    /// `clear` for the next GC.
    pub fn clear(&mut self) {
        self.regions.clear();
        self.old_regions.clear();
        self.humongous_regions.clear();
        self.optional_regions.clear();
        self.membership.fill(false);
    }

    /// `young_regions`: regions that are young (eden+survivor) — the
    /// caller distinguishes via region types; we keep the split by
    /// counting old/humongous/optional.
    pub fn young_regions(&self) -> Vec<usize> {
        let mut v = self.regions.clone();
        for r in self
            .old_regions
            .iter()
            .chain(self.humongous_regions.iter())
            .chain(self.optional_regions.iter())
        {
            if let Some(pos) = v.iter().position(|&x| x == *r) {
                v.swap_remove(pos);
            }
        }
        v
    }
}

/// Kind of region added to the collection set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CSetRegionKind {
    Young,
    Old,
    Humongous,
    OptionalOld,
}

/// `G1CollectionSetChooser`: ranks candidates by GC efficiency and
/// selects up to `max_old_regions` old regions plus all humongous
/// candidates. Mirrors `G1CollectionSetChooser::select_old_regions`.
pub struct G1CollectionSetChooser<'a> {
    stats_cache: &'a G1RegionMarkStatsCache,
}

impl<'a> G1CollectionSetChooser<'a> {
    pub fn new(stats_cache: &'a G1RegionMarkStatsCache) -> Self {
        Self { stats_cache }
    }

    /// Rank candidates: descending `gc_efficiency`, ties by region
    /// index for determinism (same ordering as the scaffold's
    /// `select_collection_set` but driven by mark-stats cache).
    pub fn rank_candidates(&self, candidates: &[usize]) -> Vec<usize> {
        let mut ranked: Vec<(usize, f64)> = candidates
            .iter()
            .map(|&i| (i, self.stats_cache.efficiency_of(i)))
            .collect();
        ranked.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        ranked.into_iter().map(|(i, _)| i).collect()
    }

    /// `select_old_regions`: pick top `max_regions` by efficiency.
    pub fn select_old_regions(&self, candidates: &[usize], max_regions: usize) -> Vec<usize> {
        self.rank_candidates(candidates).into_iter().take(max_regions).collect()
    }
}
