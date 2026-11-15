use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

pub trait ZTask: Sync {
    fn name(&self) -> &'static str;
    fn work(&self);
}

pub trait ZRestartableTask: ZTask {
    fn resize_workers(&self, _nworkers: usize) {}
}

pub trait ZThread: Send {
    fn name(&self) -> &'static str;
    fn run(&mut self);
    fn terminate(&mut self);
}

pub struct ZGCThread<T> {
    thread: Option<JoinHandle<()>>,
    state: Arc<Mutex<T>>,
    running: Arc<AtomicBool>,
    name: &'static str,
}

impl<T> ZGCThread<T> {
    pub fn spawn<F>(name: &'static str, state: T, mut run: F) -> Self
    where
        T: Send + 'static,
        F: FnMut(&mut T) + Send + 'static,
    {
        let state = Arc::new(Mutex::new(state));
        let running = Arc::new(AtomicBool::new(true));
        let thread_state = Arc::clone(&state);
        let thread_running = Arc::clone(&running);
        let thread = thread::spawn(move || {
            let mut guard = thread_state.lock().unwrap();
            while thread_running.load(Ordering::Acquire) {
                run(&mut guard);
                if thread_running.load(Ordering::Acquire) {
                    thread::yield_now();
                }
            }
        });
        Self { thread: Some(thread), state, running, name }
    }

    pub fn state(&self) -> &Arc<Mutex<T>> {
        &self.state
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    pub fn shutdown(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl<T> Drop for ZGCThread<T> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub struct ZWorkers {
    generation_name: &'static str,
    workers: AtomicUsize,
    requested_workers: AtomicUsize,
    active_workers: AtomicUsize,
    is_active: AtomicBool,
    resize_lock: Mutex<()>,
}

impl ZWorkers {
    pub fn new(generation_name: &'static str, nworkers: usize) -> Self {
        let nworkers = nworkers.max(1);
        Self {
            generation_name,
            workers: AtomicUsize::new(nworkers),
            requested_workers: AtomicUsize::new(nworkers),
            active_workers: AtomicUsize::new(0),
            is_active: AtomicBool::new(false),
            resize_lock: Mutex::new(()),
        }
    }

    pub fn generation_name(&self) -> &'static str {
        self.generation_name
    }

    pub fn nworkers(&self) -> usize {
        self.workers.load(Ordering::Acquire)
    }

    pub fn active_workers(&self) -> usize {
        self.active_workers.load(Ordering::Acquire)
    }

    pub fn is_active(&self) -> bool {
        self.is_active.load(Ordering::Acquire)
    }

    pub fn set_active(&self, active: bool) {
        self.is_active.store(active, Ordering::Release);
    }

    pub fn set_active_workers(&self, nworkers: usize) {
        self.active_workers.store(nworkers.min(self.nworkers()), Ordering::Release);
    }

    pub fn run<T: ZTask>(&self, task: &T) {
        let nworkers = self.active_workers().max(1).min(self.nworkers());
        self.set_active(true);
        thread::scope(|scope| {
            for worker_id in 1..nworkers {
                let _ = scope.spawn(|| {
                    let _ = worker_id;
                    task.work();
                });
            }
            task.work();
        });
        self.set_active(false);
        self.set_active_workers(0);
    }

    pub fn run_all<T: ZTask>(&self, task: &T) {
        self.run(task);
    }

    pub fn request_resize_workers(&self, nworkers: usize) {
        let _guard = self.resize_lock.lock().unwrap();
        self.requested_workers.store(nworkers.max(1), Ordering::Release);
    }

    pub fn should_worker_resize(&self) -> bool {
        self.requested_workers.load(Ordering::Acquire) != self.nworkers()
    }

    pub fn resize_workers(&self, nworkers: usize) {
        let _guard = self.resize_lock.lock().unwrap();
        let nworkers = nworkers.max(1);
        self.workers.store(nworkers, Ordering::Release);
        self.requested_workers.store(nworkers, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct CountTask<'a> {
        counter: &'a AtomicUsize,
    }

    impl ZTask for CountTask<'_> {
        fn name(&self) -> &'static str {
            "count"
        }
        fn work(&self) {
            self.counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn workers_run_task() {
        let workers = ZWorkers::new("young", 3);
        workers.set_active_workers(3);
        let counter = AtomicUsize::new(0);
        workers.run(&CountTask { counter: &counter });
        assert_eq!(counter.load(Ordering::Relaxed), 3);
    }
}
