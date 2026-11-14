use std::sync::atomic::{AtomicU64, Ordering};

static nextId: AtomicU64 = AtomicU64::new(3);

pub struct ThreadIdentifier;

impl ThreadIdentifier {
    pub fn allocate() -> u64 {
        nextId
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .expect("Thread identifiers exhausted")
    }

    pub fn isIssued(id: u64) -> bool {
        (3..nextId.load(Ordering::Relaxed)).contains(&id)
    }
}
