/*
 * Port of OpenJDK 25 HotSpot nmtUsage (share/nmt/nmtUsage.hpp/.cpp)
 *
 * Lightweight usage counters for NMT-tracked operations, exposed for
 * tuning/diagnostics (VM.native_memory statistics and os::NMTUsage).
 */

use std::sync::atomic::{AtomicUsize, Ordering};

/// NMTUsage — counts of tracked operations by category.
#[derive(Default)]
pub struct NmtUsage {
    mallocs: AtomicUsize,
    frees: AtomicUsize,
    malloc_bytes: AtomicUsize,
    free_bytes: AtomicUsize,
    reserves: AtomicUsize,
    commits: AtomicUsize,
    uncommits: AtomicUsize,
    releases: AtomicUsize,
    reserved_bytes: AtomicUsize,
    committed_bytes: AtomicUsize,
}

static USAGE: NmtUsage = NmtUsage {
    mallocs: AtomicUsize::new(0),
    frees: AtomicUsize::new(0),
    malloc_bytes: AtomicUsize::new(0),
    free_bytes: AtomicUsize::new(0),
    reserves: AtomicUsize::new(0),
    commits: AtomicUsize::new(0),
    uncommits: AtomicUsize::new(0),
    releases: AtomicUsize::new(0),
    reserved_bytes: AtomicUsize::new(0),
    committed_bytes: AtomicUsize::new(0),
};

pub struct NmtUsageStats;

impl NmtUsageStats {
    #[inline]
    pub fn record_malloc(size: usize) {
        USAGE.mallocs.fetch_add(1, Ordering::Relaxed);
        USAGE.malloc_bytes.fetch_add(size, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_free(size: usize) {
        USAGE.frees.fetch_add(1, Ordering::Relaxed);
        USAGE.free_bytes.fetch_add(size, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_reserve(size: usize) {
        USAGE.reserves.fetch_add(1, Ordering::Relaxed);
        USAGE.reserved_bytes.fetch_add(size, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_commit(size: usize) {
        USAGE.commits.fetch_add(1, Ordering::Relaxed);
        USAGE.committed_bytes.fetch_add(size, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_uncommit(size: usize) {
        USAGE.uncommits.fetch_add(1, Ordering::Relaxed);
        USAGE.committed_bytes.fetch_sub(size, Ordering::Relaxed);
    }

    #[inline]
    pub fn record_release(size: usize) {
        USAGE.releases.fetch_add(1, Ordering::Relaxed);
        USAGE.reserved_bytes.fetch_sub(size, Ordering::Relaxed);
    }

    /// Render a statistics block for reports.
    pub fn report() -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "mallocs: {} calls, {} bytes\n",
            USAGE.mallocs.load(Ordering::Relaxed),
            USAGE.malloc_bytes.load(Ordering::Relaxed)
        ));
        out.push_str(&format!(
            "frees:   {} calls, {} bytes\n",
            USAGE.frees.load(Ordering::Relaxed),
            USAGE.free_bytes.load(Ordering::Relaxed)
        ));
        out.push_str(&format!(
            "reserves: {} calls, {} bytes; releases: {} calls\n",
            USAGE.reserves.load(Ordering::Relaxed),
            USAGE.reserved_bytes.load(Ordering::Relaxed),
            USAGE.releases.load(Ordering::Relaxed)
        ));
        out.push_str(&format!(
            "commits: {} calls, {} bytes; uncommits: {} calls\n",
            USAGE.commits.load(Ordering::Relaxed),
            USAGE.committed_bytes.load(Ordering::Relaxed),
            USAGE.uncommits.load(Ordering::Relaxed)
        ));
        out
    }

    /// Reset all counters (tests).
    pub fn reset() {
        for c in [
            &USAGE.mallocs, &USAGE.frees, &USAGE.malloc_bytes, &USAGE.free_bytes,
            &USAGE.reserves, &USAGE.commits, &USAGE.uncommits, &USAGE.releases,
            &USAGE.reserved_bytes, &USAGE.committed_bytes,
        ] {
            c.store(0, Ordering::Relaxed);
        }
    }
}
