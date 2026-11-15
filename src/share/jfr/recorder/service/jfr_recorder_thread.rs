/*
 * OpenJDK 25 HotSpot jfr/recorder/service/jfrRecorderThread.hpp + .cpp port
 *
 * JfrRecorderThread — dedicated thread draining the post box and running
 * recorder-service loop iterations (checkpoints, rotations, metadata).
 */

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::share::jfr::recorder::service::jfr_post_box::{JfrPostBox, JfrPostBoxMsg};

static RUNNING: AtomicBool = AtomicBool::new(false);
static THREAD: std::sync::Mutex<Option<Arc<JoinHandle<()>>>> = std::sync::Mutex::new(None);

pub struct JfrRecorderThread;

impl JfrRecorderThread {
    // Starts the recorder thread. `service_step` is invoked for every
    // drained message and on each idle iteration.
    pub fn start<F>(mut service_step: F) -> bool
    where
        F: FnMut(JfrPostBoxMsg) + Send + 'static,
    {
        if RUNNING.swap(true, Ordering::SeqCst) {
            return true;
        }
        let handle = std::thread::Builder::new()
            .name("JFR Recorder Thread".to_string())
            .spawn(move || {
                let post_box = JfrPostBox;
                while RUNNING.load(Ordering::SeqCst) {
                    match post_box.dequeue() {
                        Some(msg) => service_step(msg),
                        None => {
                            std::thread::sleep(std::time::Duration::from_millis(1));
                        }
                    }
                }
            })
            .expect("failed to spawn JFR recorder thread");
        *THREAD.lock().unwrap() = Some(Arc::new(handle));
        true
    }

    pub fn stop() {
        RUNNING.store(false, Ordering::SeqCst);
        if let Some(h) = THREAD.lock().unwrap().take() {
            let _ = Arc::try_unwrap(h).map(|h| h.join());
        }
    }

    pub fn is_running() -> bool {
        RUNNING.load(Ordering::SeqCst)
    }

    pub fn enqueue(msg: JfrPostBoxMsg) {
        JfrPostBox::enqueue(msg);
    }

    pub fn request_checkpoint() {
        Self::enqueue(JfrPostBoxMsg::MsgCheckpointRequest);
    }

    pub fn request_flushpoint() {
        Self::enqueue(JfrPostBoxMsg::MsgFlushPoint);
    }

    pub fn request_chunk_rotation() {
        Self::enqueue(JfrPostBoxMsg::MsgChunkRotation);
    }
}
