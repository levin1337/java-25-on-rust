

use super::{
    checkpoint::event_emitter,
    sampling::object_sampler::{ObjectSampler, SampledKind, SampledRef},
    start_operation::StartOperation,
    stop_operation::StopOperation,
    utilities::vm_operation::execute_operation,
};
use crate::share::{
    classfile::classLoader::ClassLoader,
    interpreter::bytecodeInterpreter::Value,
    runtime::threading,
};
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Collector {
    Supported,
    Shenandoah,
    Z,
}

static COLLECTOR: AtomicU8 = AtomicU8::new(Collector::Supported as u8);

pub fn set_collector(kind: CollectorKind) {
    let value = match kind {
        CollectorKind::Supported => Collector::Supported,
        CollectorKind::Shenandoah => Collector::Shenandoah,
        CollectorKind::Z => Collector::Z,
    };
    COLLECTOR.store(value as u8, Ordering::Relaxed);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollectorKind {
    Supported,
    Shenandoah,
    Z,
}

fn collector() -> Collector {
    match COLLECTOR.load(Ordering::Relaxed) {
        1 => Collector::Shenandoah,
        2 => Collector::Z,
        _ => Collector::Supported,
    }
}

pub fn is_supported() -> bool {
    !matches!(collector(), Collector::Shenandoah | Collector::Z)
}

pub fn is_running() -> bool {
    ObjectSampler::is_created()
}

pub fn start(sample_count: usize) -> bool {
    if is_running() {
        return true;
    }
    if sample_count == 0 || !is_supported() {
        return false;
    }
    let mut op = StartOperation::new(sample_count);
    execute_operation(&mut op);
    is_running()
}

pub fn stop() -> bool {
    if !is_running() {
        return false;
    }
    let mut op = StopOperation;
    execute_operation(&mut op);
    !is_running()
}

pub fn emit_events(cutoff_ticks: i64, emit_all: bool, skip_bfs: bool) {
    if !is_running() {
        return;
    }
    let Some(loader) = threading::jvm_loader() else {
        return;
    };
    let _ = event_emitter::emit(&loader, cutoff_ticks, emit_all, skip_bfs);
}

pub fn emit_events_with<'a>(
    loader: &ClassLoader<'a>,
    cutoff_ticks: i64,
    emit_all: bool,
    skip_bfs: bool,
) -> usize {
    if !is_running() {
        return 0;
    }
    event_emitter::emit(loader, cutoff_ticks, emit_all, skip_bfs)
}

pub fn sample<'a>(
    loader: &ClassLoader<'a>,
    object: Value<'a>,
    size: usize,
    thread: Option<usize>,
) {
    if !is_running() {
        return;
    }
    let reference = match object {
        Value::Object(handle) => SampledRef {
            kind: SampledKind::Object,
            handle,
        },
        Value::ObjectArray(handle) => SampledRef {
            kind: SampledKind::ObjectArray,
            handle,
        },
        _ => return,
    };
    let thread_id = thread
        .map(|handle| java_thread_id(loader, handle))
        .unwrap_or(0);
    if thread_id == 0 {
        return;
    }
    if let Some(handle) = thread
        && crate::share::jfr::jni::java_support::is_thread_excluded(
            loader,
            Value::Object(handle),
        )
        .unwrap_or(false)
    {
        return;
    }
    let stack_trace_id = 0i64;
    let stack_trace_hash = 0i64;
    ObjectSampler::sample(
        loader,
        reference,
        size,
        thread_id,
        false,
        stack_trace_id,
        stack_trace_hash,
    );
}

fn java_thread_id(loader: &ClassLoader<'_>, handle: usize) -> i64 {
    let objects = loader.borrow_objects();
    let Some(instance) = objects.get(handle) else {
        return 0;
    };
    if let Ok(slot) = instance.klass.instanceField(b"tid", b"J")
        && let Some(Value::Long(tid)) = instance.fields.get(slot).copied()
    {
        return tid;
    }
    handle as i64
}
