

use crate::share::classfile::classLoader::ClassLoader;
use crate::share::jfr::leakprofiler::{
    checkpoint::object_sample_writer, sampling::object_sampler::ObjectSampler,
};

pub fn emit<'a>(
    loader: &ClassLoader<'a>,
    cutoff_ticks: i64,
    emit_all: bool,
    skip_bfs: bool,
) -> usize {
    let mut checkpoint =
        crate::share::jfr::leakprofiler::checkpoint::object_sample_checkpoint::ObjectSampleCheckpoint::new();
    let mut scavenge = false;
    ObjectSampler::with_sampler(|sampler| {
        sampler.for_each_sample(|_index, sample| {
            checkpoint.add(sample.clone());
        });
        scavenge = true;
    });
    if scavenge {
        ObjectSampler::with_sampler(|sampler| sampler.scavenge(loader));
    }
    object_sample_writer::write_samples(
        loader,
        &mut checkpoint,
        cutoff_ticks,
        emit_all,
        skip_bfs,
    )
}
