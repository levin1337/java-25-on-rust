/*
 * Port of OpenJDK 25 HotSpot generational Shenandoah structures
 * (share/gc/shenandoah/shenandoahGeneration.hpp,
 *  shenandoahYoungGeneration.hpp, shenandoahOldGeneration.hpp,
 *  shenandoahGlobalGeneration.hpp)
 *
 * Generational bookkeeping: per-generation used/max capacity,
 * evacuation and promotion reserves, humongous waste, affiliated
 * region counts, and marking completeness flags.
 */

use super::shenandoahGenerationType::ShenandoahGenerationType;
use super::shenandoahHeapRegion::{ShenandoahAffiliation, ShenandoahHeapRegion};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub struct ShenandoahGeneration {
    generation_type: ShenandoahGenerationType,
    max_capacity: usize,
    used: AtomicUsize,
    bytes_allocated_since_gc_start: AtomicUsize,
    affiliated_region_count: AtomicUsize,
    humongous_waste: usize,
    evacuation_reserve: usize,
    is_marking_complete: AtomicBool,
    max_task_queues: usize,
}

impl ShenandoahGeneration {
    pub fn new(generation_type: ShenandoahGenerationType, max_task_queues: usize, max_capacity: usize) -> Self {
        Self {
            generation_type,
            max_capacity,
            used: AtomicUsize::new(0),
            bytes_allocated_since_gc_start: AtomicUsize::new(0),
            affiliated_region_count: AtomicUsize::new(0),
            humongous_waste: 0,
            evacuation_reserve: 0,
            is_marking_complete: AtomicBool::new(false),
            max_task_queues: max_task_queues,
        }
    }

    pub fn generation_type(&self) -> ShenandoahGenerationType {
        self.generation_type
    }

    pub fn name(&self) -> &'static str {
        self.generation_type.name()
    }

    pub fn is_young(&self) -> bool {
        self.generation_type.is_young()
    }

    pub fn is_old(&self) -> bool {
        self.generation_type.is_old()
    }

    pub fn is_global(&self) -> bool {
        self.generation_type.is_global()
    }

    pub fn max_capacity(&self) -> usize {
        self.max_capacity
    }

    pub fn used(&self) -> usize {
        self.used.load(Ordering::Relaxed)
    }

    pub fn set_used(&self, used: usize) {
        self.used.store(used, Ordering::Relaxed);
    }

    pub fn add_used(&self, delta: usize) {
        self.used.fetch_add(delta, Ordering::Relaxed);
    }

    pub fn bytes_allocated_since_gc_start(&self) -> usize {
        self.bytes_allocated_since_gc_start.load(Ordering::Relaxed)
    }

    pub fn note_allocation(&self, bytes: usize) {
        self.bytes_allocated_since_gc_start
            .fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn reset_allocation_counter(&self) {
        self.bytes_allocated_since_gc_start.store(0, Ordering::Relaxed);
    }

    pub fn affiliated_region_count(&self) -> usize {
        self.affiliated_region_count.load(Ordering::Relaxed)
    }

    pub fn note_region_affiliated(&self) {
        self.affiliated_region_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_region_unaffiliated(&self) {
        self.affiliated_region_count.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn humongous_waste(&self) -> usize {
        self.humongous_waste
    }

    pub fn set_humongous_waste(&mut self, waste: usize) {
        self.humongous_waste = waste;
    }

    pub fn evacuation_reserve(&self) -> usize {
        self.evacuation_reserve
    }

    pub fn set_evacuation_reserve(&mut self, reserve: usize) {
        self.evacuation_reserve = reserve;
    }

    pub fn augment_evacuation_reserve(&mut self, increment: usize) {
        self.evacuation_reserve = self.evacuation_reserve.saturating_add(increment);
    }

    pub fn is_marking_complete(&self) -> bool {
        self.is_marking_complete.load(Ordering::Acquire)
    }

    pub fn set_mark_complete(&self) {
        self.is_marking_complete.store(true, Ordering::Release);
    }

    pub fn set_mark_incomplete(&self) {
        self.is_marking_complete.store(false, Ordering::Release);
    }

    pub fn max_task_queues(&self) -> usize {
        self.max_task_queues
    }

    pub fn contains_affiliation(&self, affiliation: ShenandoahAffiliation) -> bool {
        match self.generation_type {
            ShenandoahGenerationType::NonGen | ShenandoahGenerationType::Global => true,
            ShenandoahGenerationType::Young => affiliation == ShenandoahAffiliation::Young,
            ShenandoahGenerationType::Old => affiliation == ShenandoahAffiliation::Old,
        }
    }

    pub fn contains_region(&self, region: &ShenandoahHeapRegion) -> bool {
        match self.generation_type {
            ShenandoahGenerationType::NonGen | ShenandoahGenerationType::Global => true,
            _ => region.affiliation() == expected_affiliation(self.generation_type),
        }
    }

    pub fn available(&self) -> usize {
        self.max_capacity
            .saturating_sub(self.used())
            .saturating_sub(self.evacuation_reserve)
            .saturating_sub(self.humongous_waste)
    }

    pub fn soft_available(&self) -> usize {
        self.available()
    }

    pub fn prepare_gc(&mut self) {
        self.reset_allocation_counter();
    }
}

fn expected_affiliation(t: ShenandoahGenerationType) -> ShenandoahAffiliation {
    match t {
        ShenandoahGenerationType::Young => ShenandoahAffiliation::Young,
        ShenandoahGenerationType::Old => ShenandoahAffiliation::Old,
        _ => ShenandoahAffiliation::NonGen,
    }
}

pub struct ShenandoahYoungGeneration {
    base: ShenandoahGeneration,
    old_gen_task_queues: Option<usize>,
}

impl ShenandoahYoungGeneration {
    pub fn new(max_queues: usize, max_capacity: usize) -> Self {
        Self {
            base: ShenandoahGeneration::new(ShenandoahGenerationType::Young, max_queues, max_capacity),
            old_gen_task_queues: None,
        }
    }

    pub fn generation(&self) -> &ShenandoahGeneration {
        &self.base
    }

    pub fn set_old_gen_task_queues(&mut self, queues: Option<usize>) {
        self.old_gen_task_queues = queues;
    }

    pub fn is_bootstrap_cycle(&self) -> bool {
        self.old_gen_task_queues.is_some()
    }

    pub fn available(&self) -> usize {
        self.base.available()
    }

    pub fn soft_available(&self) -> usize {
        self.base.soft_available()
    }
}

pub struct ShenandoahOldGeneration {
    base: ShenandoahGeneration,
    promoted_reserve: usize,
    promoted_expended: AtomicUsize,
    promotion_potential: usize,
    pad_for_promote_in_place: usize,
    promotable_humongous_regions: usize,
    promotable_regular_regions: usize,
}

impl ShenandoahOldGeneration {
    pub fn new(max_queues: usize, max_capacity: usize) -> Self {
        Self {
            base: ShenandoahGeneration::new(ShenandoahGenerationType::Old, max_queues, max_capacity),
            promoted_reserve: 0,
            promoted_expended: AtomicUsize::new(0),
            promotion_potential: 0,
            pad_for_promote_in_place: 0,
            promotable_humongous_regions: 0,
            promotable_regular_regions: 0,
        }
    }

    pub fn generation(&self) -> &ShenandoahGeneration {
        &self.base
    }

    pub fn set_promoted_reserve(&mut self, reserve: usize) {
        self.promoted_reserve = reserve;
    }

    pub fn get_promoted_reserve(&self) -> usize {
        self.promoted_reserve
    }

    pub fn augment_promoted_reserve(&mut self, increment: usize) {
        self.promoted_reserve = self.promoted_reserve.saturating_add(increment);
    }

    pub fn reset_promoted_expended(&self) {
        self.promoted_expended.store(0, Ordering::Relaxed);
    }

    pub fn expend_promoted(&self, increment: usize) -> usize {
        self.promoted_expended.fetch_add(increment, Ordering::Relaxed) + increment
    }

    pub fn unexpend_promoted(&self, decrement: usize) -> usize {
        self.promoted_expended.fetch_sub(decrement, Ordering::Relaxed) - decrement
    }

    pub fn get_promoted_expended(&self) -> usize {
        self.promoted_expended.load(Ordering::Relaxed)
    }

    pub fn can_promote(&self, requested_bytes: usize) -> bool {
        self.get_promoted_expended() + requested_bytes <= self.get_promoted_reserve()
    }

    pub fn promotion_potential(&self) -> usize {
        self.promotion_potential
    }

    pub fn set_promotion_potential(&mut self, potential: usize) {
        self.promotion_potential = potential;
    }

    pub fn pad_for_promote_in_place(&self) -> usize {
        self.pad_for_promote_in_place
    }

    pub fn set_pad_for_promote_in_place(&mut self, pad: usize) {
        self.pad_for_promote_in_place = pad;
    }

    pub fn promotable_humongous_regions(&self) -> usize {
        self.promotable_humongous_regions
    }

    pub fn promotable_regular_regions(&self) -> usize {
        self.promotable_regular_regions
    }

    pub fn set_promotable_regions(&mut self, humongous: usize, regular: usize) {
        self.promotable_humongous_regions = humongous;
        self.promotable_regular_regions = regular;
    }

    pub fn coalesce_and_fill(&mut self) -> bool {
        self.promotable_humongous_regions > 0 || self.promotable_regular_regions > 0
    }
}

pub struct ShenandoahGlobalGeneration {
    base: ShenandoahGeneration,
    generational: bool,
}

impl ShenandoahGlobalGeneration {
    pub fn new(generational: bool, max_queues: usize, max_capacity: usize) -> Self {
        let t = if generational {
            ShenandoahGenerationType::Global
        } else {
            ShenandoahGenerationType::NonGen
        };
        Self {
            base: ShenandoahGeneration::new(t, max_queues, max_capacity),
            generational,
        }
    }

    pub fn generation(&self) -> &ShenandoahGeneration {
        &self.base
    }

    pub fn is_generational(&self) -> bool {
        self.generational
    }

    pub fn max_capacity(&self) -> usize {
        self.base.max_capacity()
    }

    pub fn contains_addr(&self, heap_base: usize, _region_size: usize, addr: usize) -> bool {
        addr >= heap_base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn young_generation_membership() {
        let mut young = ShenandoahYoungGeneration::new(4, 1 << 30);
        let mut region = ShenandoahHeapRegion::new_affiliated(
            0,
            0x1000,
            ShenandoahHeapRegion::DEFAULT_REGION_SIZE,
            ShenandoahAffiliation::Young,
            true,
        );
        assert!(young.generation().contains_region(&region));
        region.set_affiliation(ShenandoahAffiliation::Old);
        assert!(!young.generation().contains_region(&region));
        assert!(!young.is_bootstrap_cycle());
        young.set_old_gen_task_queues(Some(8));
        assert!(young.is_bootstrap_cycle());
    }

    #[test]
    fn old_generation_promotion_reserve() {
        let mut old = ShenandoahOldGeneration::new(4, 1 << 30);
        old.set_promoted_reserve(1000);
        assert!(old.can_promote(100));
        old.expend_promoted(400);
        assert!(old.can_promote(500));
        assert!(!old.can_promote(700));
        old.unexpend_promoted(200);
        assert!(old.can_promote(700));
    }

    #[test]
    fn available_accounts_reserves() {
        let mut g = ShenandoahGeneration::new(ShenandoahGenerationType::NonGen, 4, 1000);
        g.set_used(400);
        g.set_evacuation_reserve(100);
        g.set_humongous_waste(50);
        assert_eq!(g.available(), 450);
        g.prepare_gc();
        assert_eq!(g.bytes_allocated_since_gc_start(), 0);
    }
}
