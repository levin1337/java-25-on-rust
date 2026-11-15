use super::z_address::{ZAddress, ZPointer, load_good, store_good};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

pub trait ZBarrierBackend: Send + Sync {
    fn load_barrier_slow(&self, field: &AtomicU64, ptr: ZPointer) -> ZAddress;
    fn store_barrier_slow(&self, field: &AtomicU64, ptr: ZPointer) -> ZAddress;
    fn mark_barrier_slow(&self, address: ZAddress, finalizable: bool) -> ZAddress;
    fn remember_slow(&self, field: &AtomicU64, ptr: ZPointer);
    fn relocate_or_remap_slow(&self, address: ZAddress) -> ZAddress;
    fn remap_slow(&self, address: ZAddress) -> ZAddress;
}

pub struct ZNoopBarrierBackend;

impl ZBarrierBackend for ZNoopBarrierBackend {
    fn load_barrier_slow(&self, _field: &AtomicU64, ptr: ZPointer) -> ZAddress {
        ptr.uncolor()
    }

    fn store_barrier_slow(&self, _field: &AtomicU64, ptr: ZPointer) -> ZAddress {
        ptr.uncolor()
    }

    fn mark_barrier_slow(&self, address: ZAddress, _finalizable: bool) -> ZAddress {
        address
    }

    fn remember_slow(&self, _field: &AtomicU64, _ptr: ZPointer) {}

    fn relocate_or_remap_slow(&self, address: ZAddress) -> ZAddress {
        address
    }

    fn remap_slow(&self, address: ZAddress) -> ZAddress {
        address
    }
}

pub struct ZBarrierSet {
    backend: Arc<dyn ZBarrierBackend>,
}

impl ZBarrierSet {
    pub fn new(backend: Arc<dyn ZBarrierBackend>) -> Self {
        Self { backend }
    }

    pub fn noop() -> Self {
        Self::new(Arc::new(ZNoopBarrierBackend))
    }

    pub fn backend(&self) -> &dyn ZBarrierBackend {
        self.backend.as_ref()
    }

    pub fn load_atomic(&self, field: &AtomicU64) -> ZPointer {
        ZPointer::from_raw(field.load(Ordering::Acquire))
    }

    pub fn load_barrier(&self, ptr: ZPointer) -> ZAddress {
        if ptr.is_null_any() {
            0
        } else if ptr.is_load_good() {
            ptr.uncolor()
        } else {
            ptr.uncolor_unsafe()
        }
    }

    pub fn load_barrier_or_null(&self, ptr: ZPointer) -> ZAddress {
        if ptr.is_null_any() {
            0
        } else {
            self.load_barrier(ptr)
        }
    }

    pub fn load_barrier_on_oop_field(&self, field: &AtomicU64) -> ZAddress {
        let ptr = self.load_atomic(field);
        if ptr.is_load_good_or_null() {
            self.load_barrier(ptr)
        } else {
            self.backend.load_barrier_slow(field, ptr)
        }
    }

    pub fn load_barrier_on_oop_field_preloaded(&self, field: &AtomicU64, ptr: ZPointer) -> ZAddress {
        if ptr.is_load_good_or_null() {
            self.load_barrier(ptr)
        } else {
            self.backend.load_barrier_slow(field, ptr)
        }
    }

    pub fn store_barrier_on_heap_oop_field(&self, field: &AtomicU64, heal: bool) {
        let ptr = self.load_atomic(field);
        if heal && !ptr.is_store_good_or_null() {
            self.backend.store_barrier_slow(field, ptr);
        } else if !ptr.is_store_good_or_null() {
            self.backend.remember_slow(field, ptr);
        }
    }

    pub fn store_barrier_on_heap_oop_field_value(&self, field: &AtomicU64, value: ZAddress) {
        field.store(store_good(value).raw(), Ordering::Release);
    }

    pub fn store_barrier_on_native_oop_field(&self, field: &AtomicU64, heal: bool) {
        self.store_barrier_on_heap_oop_field(field, heal);
    }

    pub fn no_keep_alive_store_barrier_on_heap_oop_field(&self, field: &AtomicU64) {
        let ptr = self.load_atomic(field);
        if !ptr.is_store_good_or_null() {
            self.backend.remember_slow(field, ptr);
        }
    }

    pub fn mark_barrier_on_oop_field(&self, field: &AtomicU64, finalizable: bool) {
        let ptr = self.load_atomic(field);
        if ptr.is_null_any() {
            return;
        }
        let address = ptr.uncolor();
        if ptr.is_mark_good() {
            return;
        }
        let healed = self.backend.mark_barrier_slow(address, finalizable);
        let _ = healed;
        let _ = field.compare_exchange(
            ptr.raw(),
            load_good(address, ZPointer::NULL).raw(),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    pub fn mark_barrier_on_young_oop_field(&self, field: &AtomicU64) {
        self.mark_barrier_on_oop_field(field, false);
    }

    pub fn mark_barrier_on_old_oop_field(&self, field: &AtomicU64, finalizable: bool) {
        self.mark_barrier_on_oop_field(field, finalizable);
    }

    pub fn remset_barrier_on_oop_field(&self, field: &AtomicU64) -> ZAddress {
        let ptr = self.load_atomic(field);
        if ptr.is_store_good() {
            ptr.uncolor()
        } else {
            self.backend.remember_slow(field, ptr);
            ptr.uncolor()
        }
    }

    pub fn promote_barrier_on_young_oop_field(&self, field: &AtomicU64) {
        let ptr = self.load_atomic(field);
        if !ptr.is_store_good_or_null() {
            self.backend.store_barrier_slow(field, ptr);
        }
    }

    pub fn relocate_or_remap(&self, address: ZAddress) -> ZAddress {
        if address == 0 {
            0
        } else {
            self.backend.relocate_or_remap_slow(address)
        }
    }

    pub fn remap(&self, address: ZAddress) -> ZAddress {
        if address == 0 {
            0
        } else {
            self.backend.remap_slow(address)
        }
    }
}

impl Default for ZBarrierSet {
    fn default() -> Self {
        Self::noop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::gc::z::z_address::{offset_to_address, store_good};

    #[test]
    fn load_barrier_fast_path() {
        let barriers = ZBarrierSet::noop();
        let address = offset_to_address(0x1000);
        let field = AtomicU64::new(store_good(address).raw());
        assert_eq!(barriers.load_barrier_on_oop_field(&field), address);
    }

    #[test]
    fn store_barrier_writes_store_good() {
        let barriers = ZBarrierSet::noop();
        let field = AtomicU64::new(0);
        let address = offset_to_address(0x2000);
        barriers.store_barrier_on_heap_oop_field_value(&field, address);
        assert!(ZPointer::from_raw(field.load(Ordering::Acquire)).is_store_good());
    }
}
