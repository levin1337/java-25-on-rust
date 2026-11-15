use super::z_generation_id::{ZGenerationId, ZGenerationIdOptional};
use super::z_mark::ZMark;
use super::z_page::ZPage;
use super::z_relocation_set::{ZRelocationSet, ZRelocationSetSelector};
use super::z_statistics::ZStatistics;
use super::z_workers::ZWorkers;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZGenerationPhase {
    Mark,
    MarkComplete,
    Relocate,
}

impl ZGenerationPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mark => "Mark",
            Self::MarkComplete => "Mark Complete",
            Self::Relocate => "Relocate",
        }
    }
}

pub struct ZGeneration {
    id: ZGenerationId,
    seqnum: AtomicU32,
    phase: Mutex<ZGenerationPhase>,
    freed: AtomicU64,
    promoted: AtomicU64,
    compacted: AtomicU64,
    workers: ZWorkers,
    mark: Mutex<ZMark>,
    relocation_set: Mutex<ZRelocationSet>,
    statistics: ZStatistics,
}

impl ZGeneration {
    pub fn new(id: ZGenerationId, nworkers: usize, max_offset: u64) -> Self {
        Self {
            id,
            seqnum: AtomicU32::new(1),
            phase: Mutex::new(ZGenerationPhase::Mark),
            freed: AtomicU64::new(0),
            promoted: AtomicU64::new(0),
            compacted: AtomicU64::new(0),
            workers: ZWorkers::new(if id.is_young() { "young" } else { "old" }, nworkers),
            mark: Mutex::new(ZMark::new(nworkers)),
            relocation_set: Mutex::new(ZRelocationSet::new(max_offset)),
            statistics: ZStatistics::new(),
        }
    }

    pub fn id(&self) -> ZGenerationId {
        self.id
    }

    pub fn id_optional(&self) -> ZGenerationIdOptional {
        self.id.into()
    }

    pub fn is_young(&self) -> bool {
        self.id.is_young()
    }

    pub fn is_old(&self) -> bool {
        self.id.is_old()
    }

    pub fn seqnum(&self) -> u32 {
        self.seqnum.load(Ordering::Acquire)
    }

    pub fn next_seqnum(&self) -> u32 {
        self.seqnum.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
    }

    pub fn phase(&self) -> ZGenerationPhase {
        *self.phase.lock().unwrap()
    }

    pub fn set_phase(&self, phase: ZGenerationPhase) {
        *self.phase.lock().unwrap() = phase;
    }

    pub fn is_phase_mark(&self) -> bool {
        self.phase() == ZGenerationPhase::Mark
    }

    pub fn is_phase_mark_complete(&self) -> bool {
        self.phase() == ZGenerationPhase::MarkComplete
    }

    pub fn is_phase_relocate(&self) -> bool {
        self.phase() == ZGenerationPhase::Relocate
    }

    pub fn freed(&self) -> u64 {
        self.freed.load(Ordering::Acquire)
    }

    pub fn increase_freed(&self, size: usize) {
        self.freed.fetch_add(size as u64, Ordering::AcqRel);
    }

    pub fn promoted(&self) -> u64 {
        self.promoted.load(Ordering::Acquire)
    }

    pub fn increase_promoted(&self, size: usize) {
        self.promoted.fetch_add(size as u64, Ordering::AcqRel);
    }

    pub fn compacted(&self) -> u64 {
        self.compacted.load(Ordering::Acquire)
    }

    pub fn increase_compacted(&self, size: usize) {
        self.compacted.fetch_add(size as u64, Ordering::AcqRel);
    }

    pub fn workers(&self) -> &ZWorkers {
        &self.workers
    }

    pub fn mark(&self) -> &Mutex<ZMark> {
        &self.mark
    }

    pub fn relocation_set(&self) -> &Mutex<ZRelocationSet> {
        &self.relocation_set
    }

    pub fn statistics(&self) -> &ZStatistics {
        &self.statistics
    }

    pub fn reset_statistics(&self) {
        self.freed.store(0, Ordering::Release);
        self.promoted.store(0, Ordering::Release);
        self.compacted.store(0, Ordering::Release);
    }

    pub fn select_relocation_set(&self, pages: &[ZPage], tenuring_threshold: u8, max_selected: usize) -> ZRelocationSetSelector {
        ZRelocationSetSelector::select(pages, self.seqnum(), tenuring_threshold, max_selected, Some(self.id))
    }

    pub fn install_relocation_set(&self, pages: &[ZPage], selector: &ZRelocationSetSelector, tenuring_threshold: u8) {
        self.relocation_set().lock().unwrap().install(pages, selector, tenuring_threshold);
    }
}

pub struct ZGenerationYoung {
    generation: ZGeneration,
    tenuring_threshold: AtomicU32,
    remembered_pages: Mutex<Vec<usize>>,
}

impl ZGenerationYoung {
    pub fn new(nworkers: usize, max_offset: u64, tenuring_threshold: u8) -> Self {
        Self {
            generation: ZGeneration::new(ZGenerationId::Young, nworkers, max_offset),
            tenuring_threshold: AtomicU32::new(tenuring_threshold as u32),
            remembered_pages: Mutex::new(Vec::new()),
        }
    }

    pub fn generation(&self) -> &ZGeneration {
        &self.generation
    }

    pub fn tenuring_threshold(&self) -> u8 {
        self.tenuring_threshold.load(Ordering::Acquire) as u8
    }

    pub fn set_tenuring_threshold(&self, threshold: u8) {
        self.tenuring_threshold.store(threshold as u32, Ordering::Release);
    }

    pub fn select_tenuring_threshold(&self, stats: super::z_relocation_set::ZRelocationSetSelectorStats, promote_all: bool) {
        if promote_all {
            self.set_tenuring_threshold(0);
            return;
        }
        let density = if stats.total_bytes == 0 { 0 } else { stats.live_bytes * 100 / stats.total_bytes };
        self.set_tenuring_threshold(if density > 50 { 10 } else { 4 });
    }

    pub fn register_remembered_page(&self, page: usize) {
        let mut pages = self.remembered_pages.lock().unwrap();
        if !pages.contains(&page) {
            pages.push(page);
        }
    }

    pub fn remembered_pages(&self) -> Vec<usize> {
        self.remembered_pages.lock().unwrap().clone()
    }
}

pub struct ZGenerationOld {
    generation: ZGeneration,
    total_collections_at_start: AtomicU64,
}

impl ZGenerationOld {
    pub fn new(nworkers: usize, max_offset: u64) -> Self {
        Self {
            generation: ZGeneration::new(ZGenerationId::Old, nworkers, max_offset),
            total_collections_at_start: AtomicU64::new(0),
        }
    }

    pub fn generation(&self) -> &ZGeneration {
        &self.generation
    }

    pub fn total_collections_at_start(&self) -> u64 {
        self.total_collections_at_start.load(Ordering::Acquire)
    }

    pub fn set_total_collections_at_start(&self, value: u64) {
        self.total_collections_at_start.store(value, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_seqnum_and_phase() {
        let young = ZGenerationYoung::new(1, 1 << 32, 10);
        assert_eq!(young.generation().id(), ZGenerationId::Young);
        assert!(young.generation().is_phase_mark());
        let seqnum = young.generation().next_seqnum();
        assert!(seqnum > young.generation().seqnum() - 1);
    }
}
