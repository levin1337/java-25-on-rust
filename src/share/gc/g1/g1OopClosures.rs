/*
 * Port of OpenJDK 25 HotSpot G1 oop closure framework
 * (share/gc/g1/g1OopClosures.hpp) and evacuation metadata
 * (g1EvacInfo.hpp, g1EvacStats.hpp, g1RegionToSpaceMapper.hpp).
 *
 * Closures are traits: the marking/evacuation algorithms in
 * g1ParScan/g1ConcurrentMark call `do_oop`/`scan_task` and the
 * concrete heap (`memory/heap.rs`) supplies the implementation.
 */

use super::g1ParScan::G1ParScanThreadState;
use super::heapRegion::{G1HeapRegionAttr, G1HeapRegionAttrType};

/// `G1OopClosures::ScanTask` equivalents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanTask {
    /// A plain oop address to scan.
    Oop(usize),
    /// Partial array state: (array base, index, length) for long
    /// arrays scanned incrementally.
    PartialArray { base: usize, index: usize, length: usize },
    /// An nmethod/code root handle.
    NMethod(usize),
    /// A card to scan in a region (region_index, card_index).
    Card { region: usize, card: u32 },
}

/// Region claim used during evacuation: source region -> target
/// region allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionClaim {
    pub source_region: usize,
    /// Target region index in the target space, `usize::MAX` if not
    /// yet claimed.
    pub target_region: usize,
    /// Offset within target region where evacuation writes.
    pub target_top: usize,
}

/// `G1RegionToSpaceMapper`: maps source regions to target (to-space)
/// regions during evacuation. Target regions are allocated linearly.
#[derive(Debug)]
pub struct G1RegionToSpaceMapper {
    /// source region -> claim
    claims: Vec<RegionClaim>,
    /// Next target region index to hand out.
    next_target: usize,
    target_region_size: usize,
}

impl G1RegionToSpaceMapper {
    pub fn new(num_source_regions: usize, first_target_region: usize, region_size: usize) -> Self {
        Self {
            claims: (0..num_source_regions)
                .map(|i| RegionClaim {
                    source_region: i,
                    target_region: usize::MAX,
                    target_top: 0,
                })
                .collect(),
            next_target: first_target_region,
            target_region_size: region_size,
        }
    }

    /// `claim`/`forward` a source region to a fresh target region.
    /// Returns the claim.
    pub fn claim(&mut self, source_region: usize) -> RegionClaim {
        if let Some(c) = self.claims.get_mut(source_region) {
            if c.target_region == usize::MAX {
                c.target_region = self.next_target;
                c.target_top = 0;
                self.next_target += 1;
            }
            *c
        } else {
            RegionClaim { source_region, target_region: usize::MAX, target_top: 0 }
        }
    }

    /// `target_region` lookup.
    pub fn target_region(&self, source_region: usize) -> Option<usize> {
        self.claims.get(source_region).and_then(|c| {
            if c.target_region == usize::MAX { None } else { Some(c.target_region) }
        })
    }

    /// `forward` an address inside a claimed source region to the
    /// corresponding address in the target region (used by the
    /// forwarding-pointer scheme during evacuation).
    pub fn forward_addr(&self, source_region: usize, offset_in_region: usize) -> Option<usize> {
        let claim = self.claims.get(source_region)?;
        if claim.target_region == usize::MAX {
            return None;
        }
        // target_top tracks writes; forwarding uses same offset for
        // compacted regions (HotSpot uses PLAB allocation order, so
        // this is an approximation for non-moving analysis).
        Some(claim.target_region * self.target_region_size + offset_in_region)
    }

    pub fn num_claims(&self) -> usize {
        self.claims.iter().filter(|c| c.target_region != usize::MAX).count()
    }

    pub fn claims(&self) -> &[RegionClaim] {
        &self.claims
    }
}

/// `G1EvacStats`: evacuation statistics for GC logging / analytics.
#[derive(Debug, Default, Clone)]
pub struct G1EvacStats {
    /// Regions successfully evacuated.
    pub evacuated_regions: u64,
    /// Regions that failed evacuation.
    pub failed_regions: u64,
    /// Bytes evacuated (live bytes copied).
    pub evacuated_bytes: u64,
    /// Bytes reclaimed by releasing failed regions back.
    pub reclaimed_bytes_failed: u64,
    /// Humongous objects reclaimed without evacuation.
    pub humongous_reclaimed: u64,
    /// Optional regions that were not used and got retired.
    pub unused_optional_regions: u64,
    /// Worker-id -> evacuated byte count.
    pub per_worker_bytes: Vec<u64>,
}

impl G1EvacStats {
    pub fn new(num_workers: usize) -> Self {
        Self { per_worker_bytes: vec![0; num_workers], ..Default::default() }
    }

    pub fn add_evacuated(&mut self, bytes: u64, worker: usize) {
        self.evacuated_regions += 1;
        self.evacuated_bytes += bytes;
        if worker < self.per_worker_bytes.len() {
            self.per_worker_bytes[worker] += bytes;
        }
    }

    pub fn add_failed(&mut self, bytes: u64) {
        self.failed_regions += 1;
        self.reclaimed_bytes_failed += bytes;
    }

    pub fn total_evacuated_bytes(&self) -> u64 {
        self.evacuated_bytes
    }
}

/// `G1EvacuationFailureInfo`-style record kept per failed region.
#[derive(Debug, Clone, Copy)]
pub struct EvacuationFailure {
    pub region_index: usize,
    /// Address that failed to evacuate.
    pub failed_addr: usize,
    /// Whether the failure was caused by a pinned object.
    pub cause_pinned: bool,
    pub worker_id: usize,
}

/// Collection of evacuation failures for the current pause.
#[derive(Debug, Default)]
pub struct EvacuationFailures {
    failures: Vec<EvacuationFailure>,
}

impl EvacuationFailures {
    pub fn record(&mut self, region_index: usize, failed_addr: usize, cause_pinned: bool, worker_id: usize) {
        self.failures.push(EvacuationFailure { region_index, failed_addr, cause_pinned, worker_id });
    }

    pub fn failures(&self) -> &[EvacuationFailure] {
        &self.failures
    }

    pub fn is_empty(&self) -> bool {
        self.failures.is_empty()
    }

    pub fn clear(&mut self) {
        self.failures.clear();
    }
}

/// `G1OopClosure`: the closure interface used by parallel marking and
/// evacuation. `state` gives access to the worker's stacks so the
/// closure can push discovered oops.
pub trait G1OopClosure {
    /// Visit a single oop (mark or evacuate depending on phase).
    fn do_oop(&mut self, oop: usize, state: &mut G1ParScanThreadState);

    /// Dispatch a scanner task.
    fn scan_task(&mut self, task: ScanTask, state: &mut G1ParScanThreadState) {
        match task {
            ScanTask::Oop(oop) => self.do_oop(oop, state),
            ScanTask::PartialArray { base, index, length } => {
                self.do_partial_array(base, index, length, state)
            }
            ScanTask::NMethod(nm) => self.scan_nmethod(nm, state),
            ScanTask::Card { region, card } => self.scan_card(region, card, state),
        }
    }

    /// `do_partial_array`: scan `length` slots starting at `index`.
    fn do_partial_array(&mut self, _base: usize, _index: usize, _length: usize, _state: &mut G1ParScanThreadState) {}

    /// `scan_nmethod`: scan an nmethod/code root handle.
    fn scan_nmethod(&mut self, _nm: usize, _state: &mut G1ParScanThreadState) {}

    /// `scan_card`: scan the given card of a region.
    fn scan_card(&mut self, _region: usize, _card: u32, _state: &mut G1ParScanThreadState) {}

    /// `region_attr` lookup hook: returns the GC attribute for a
    /// region index. Implementations consult the heap's attr table.
    fn region_attr(&self, _region: usize) -> G1HeapRegionAttr {
        G1HeapRegionAttr::new()
    }
}

/// `G1ScanClosures` helper: decides whether a discovered oop should
/// be pushed onto the scan stack, given its region's CSet attribute.
/// Mirrors `G1ParScanThreadState::should_be_scanned` logic:
/// young CSet regions always; old CSet regions when marking;
/// optional regions recorded specially.
#[inline]
pub fn should_scan_region(attr: G1HeapRegionAttr, marking_old: bool) -> bool {
    if attr.is_young() {
        return true;
    }
    if attr.is_old() {
        return marking_old;
    }
    if attr.is_in_cset_or_humongous_candidate() {
        return true;
    }
    false
}

/// `G1EvacuationRootClosures` bookkeeping: which root categories were
/// already scanned (dedup for repeated root iteration).
#[derive(Debug, Default)]
pub struct G1EvacuationRootClosures {
    scanned_threads: bool,
    scanned_jni: bool,
    scanned_jvmti: bool,
    scanned_system_dict: bool,
    scanned_cm: bool,
}

impl G1EvacuationRootClosures {
    pub fn all_scanned(&self) -> bool {
        self.scanned_threads
            && self.scanned_jni
            && self.scanned_jvmti
            && self.scanned_system_dict
            && self.scanned_cm
    }

    pub fn mark_scanned(&mut self, which: &str) {
        match which {
            "threads" => self.scanned_threads = true,
            "jni" => self.scanned_jni = true,
            "jvmti" => self.scanned_jvmti = true,
            "system_dict" => self.scanned_system_dict = true,
            "cm" => self.scanned_cm = true,
            _ => {}
        }
    }
}
