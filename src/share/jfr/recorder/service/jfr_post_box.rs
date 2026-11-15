/*
 * OpenJDK 25 HotSpot jfr/recorder/service/jfrPostBox.hpp + .cpp port
 *
 * Lock-free (mutex) MPSC-ish message queue between the VM and the JFR
 * recorder thread. Rust: Mutex<VecDeque<JfrPostBoxMsg>>.
 */

use std::collections::VecDeque;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JfrPostBoxMsg {
    MsgChunkRotation = 0,
    MsgCheckpointRequest = 1,
    MsgFlushPoint = 2,
    MsgShutdown = 3,
    MsgStartRecording = 4,
    MsgStopRecording = 5,
    MsgDump = 6,
    MsgDumpOnExit = 7,
    MsgDumpForFullRepository = 8,
    MsgReleaseBuffers = 9,
    MsgMetadataEvent = 10,
    MsgHeartbeat = 11,
    MsgNewDestination = 12,
}

static POST_BOX: Mutex<VecDeque<JfrPostBoxMsg>> = Mutex::new(VecDeque::new());
static RECORDING: Mutex<bool> = Mutex::new(false);
static ENABLED: Mutex<bool> = Mutex::new(false);

pub struct JfrPostBox;

impl JfrPostBox {
    pub fn enqueue(msg: JfrPostBoxMsg) {
        POST_BOX.lock().unwrap().push_back(msg);
    }

    pub fn peek(&self) -> Option<JfrPostBoxMsg> {
        POST_BOX.lock().unwrap().front().copied()
    }

    pub fn dequeue(&self) -> Option<JfrPostBoxMsg> {
        POST_BOX.lock().unwrap().pop_front()
    }

    pub fn is_empty(&self) -> bool {
        POST_BOX.lock().unwrap().is_empty()
    }

    pub fn set_is_recording(v: bool) {
        *RECORDING.lock().unwrap() = v;
    }

    pub fn is_recording() -> bool {
        *RECORDING.lock().unwrap()
    }

    pub fn set_is_enabled(v: bool) {
        *ENABLED.lock().unwrap() = v;
    }

    pub fn is_enabled() -> bool {
        *ENABLED.lock().unwrap()
    }
}
