use crate::share::memory::heap::HeapGcMode;
use std::sync::OnceLock;

/// Maps the `-XX:+Use*GC` launcher flags onto the heap engine that
/// `memory::heap` actually executes. The last GC flag on the command
/// line wins, matching HotSpot ergonomics. `JVM25_GC` (`serial`,
/// `parallel`, `epsilon`) overrides everything for tests.
///
/// G1/ZGC/Shenandoah select `SerialMarkSweep` here: their region
/// machinery, remembered sets and concurrent barriers live in
/// `share::gc` but are not yet wired into the slot-arena heap, so the
/// engine falls back to the only complete non-moving collector.
pub fn heap_gc_mode() -> HeapGcMode {
    static MODE: OnceLock<HeapGcMode> = OnceLock::new();
    *MODE.get_or_init(|| {
        if let Ok(forced) = std::env::var("JVM25_GC") {
            return match forced.as_str() {
                "parallel" | "throughput" => HeapGcMode::ParallelMarkSweep,
                "epsilon" | "nogc" => HeapGcMode::NoCollect,
                _ => HeapGcMode::SerialMarkSweep,
            };
        }
        let mut mode = None;
        for arg in std::env::args_os() {
            let Some(arg) = arg.to_str() else {
                continue;
            };
            mode = Some(match arg {
                "-XX:+UseParallelGC" => HeapGcMode::ParallelMarkSweep,
                "-XX:+UseEpsilonGC" => HeapGcMode::NoCollect,
                "-XX:+UseSerialGC"
                | "-XX:+UseG1GC"
                | "-XX:+UseZGC"
                | "-XX:+UseShenandoahGC" => HeapGcMode::SerialMarkSweep,
                _ => continue,
            });
        }
        mode.unwrap_or(HeapGcMode::SerialMarkSweep)
    })
}

/// Worker count for the parallel mark-sweep engine (ParallelGC
/// `-XX:ParallelGCThreads=N`; last occurrence wins). Falls back to the
/// hardware parallelism, then 4 — same default HotSpot uses for small
/// heaps.
pub fn parallel_gc_workers() -> usize {
    let mut workers = None;
    for arg in std::env::args_os() {
        let Some(arg) = arg.to_str() else {
            continue;
        };
        if let Some(value) = arg.strip_prefix("-XX:ParallelGCThreads=") {
            workers = value.parse::<usize>().ok();
        }
    }
    workers
        .filter(|count| *count > 0)
        .or_else(|| std::thread::available_parallelism().ok().map(|n| n.get()))
        .unwrap_or(4)
}
