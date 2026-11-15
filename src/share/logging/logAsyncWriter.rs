/*
 * Port of OpenJDK 25 HotSpot LogAsyncWriter (share/logging/logAsyncWriter.cpp)
 *
 * Asynchronous logging (-Xlog:async[:drop|:stall]): log records are
 * rendered at the log site and enqueued as byte buffers; a dedicated
 * writer thread drains the queue to the configured LogOutputs.
 * Drop mode discards records when the queue is full; stall mode blocks
 * the log site until space is available.
 */

use super::logDecorations::{Decorations, LogDecorations};
use super::logOutput::LogOutput;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, OnceLock};

/// LogConfiguration::AsyncMode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsyncMode {
    Off,
    Drop,
    Stall,
}

/// Maximum queued records before drop/stall kicks in.
const QUEUE_CAPACITY: usize = 1 << 16;

struct AsyncItem {
    output: Arc<dyn LogOutput>,
    decorations: LogDecorations,
    decorators: Decorations,
    msg: String,
}

struct FlushRequest {
    done: Arc<(Mutex<bool>, Condvar)>,
}

enum QueueItem {
    Record(AsyncItem),
    Flush(FlushRequest),
}

struct WriterState {
    queue: VecDeque<QueueItem>,
    shutdown: bool,
}

struct AsyncLogWriter {
    state: Mutex<WriterState>,
    not_empty: Condvar,
    not_full: Condvar,
}

fn writer() -> Arc<AsyncLogWriter> {
    static WRITER: OnceLock<Arc<AsyncLogWriter>> = OnceLock::new();
    WRITER
        .get_or_init(|| {
            let w = Arc::new(AsyncLogWriter {
                state: Mutex::new(WriterState {
                    queue: VecDeque::new(),
                    shutdown: false,
                }),
                not_empty: Condvar::new(),
                not_full: Condvar::new(),
            });
            let w2 = w.clone();
            std::thread::Builder::new()
                .name("async-log-writer".to_string())
                .spawn(move || writer_thread(&w2))
                .expect("failed to spawn async log writer thread");
            w
        })
        .clone()
}

fn writer_thread(w: &AsyncLogWriter) {
    loop {
        let item = {
            let mut state = w.state.lock().unwrap();
            loop {
                if let Some(item) = state.queue.pop_front() {
                    break item;
                }
                if state.shutdown {
                    return;
                }
                state = w.not_empty.wait(state).unwrap();
            }
        };
        match item {
            QueueItem::Record(rec) => {
                let _ = rec
                    .output
                    .write_record(&rec.decorations, &rec.decorators, &rec.msg);
                let _ = rec.output.flush();
            }
            QueueItem::Flush(req) => {
                {
                    let mut done = req.done.0.lock().unwrap();
                    *done = true;
                }
                req.done.1.notify_all();
            }
        }
        w.not_full.notify_all();
    }
}

impl AsyncLogWriter {
    /// AsyncLogWriter::enqueue — returns true if the record was queued
    /// (false in drop mode when the queue is full).
    fn enqueue(
        &self,
        output: Arc<dyn LogOutput>,
        decorations: LogDecorations,
        decorators: Decorations,
        msg: String,
        mode: AsyncMode,
    ) -> bool {
        let mut state = self.state.lock().unwrap();
        loop {
            if state.queue.len() < QUEUE_CAPACITY {
                state.queue.push_back(QueueItem::Record(AsyncItem {
                    output,
                    decorations,
                    decorators,
                    msg,
                }));
                self.not_empty.notify_one();
                return true;
            }
            match mode {
                AsyncMode::Drop => return false,
                AsyncMode::Stall => {
                    state = self.not_full.wait(state).unwrap();
                }
                AsyncMode::Off => return false,
            }
        }
    }

    /// AsyncLogWriter::flush — wait until all queued records are written.
    fn flush(&self) {
        let done = Arc::new((Mutex::new(false), Condvar::new()));
        {
            let mut state = self.state.lock().unwrap();
            state.queue.push_back(QueueItem::Flush(FlushRequest {
                done: done.clone(),
            }));
            self.not_empty.notify_one();
        }
        let mut flag = done.0.lock().unwrap();
        while !*flag {
            flag = done.1.wait(flag).unwrap();
        }
    }
}

/// Entry point used by Log::write when async logging is enabled.
pub fn enqueue_record(
    output: Arc<dyn LogOutput>,
    decorations: LogDecorations,
    decorators: Decorations,
    msg: String,
    mode: AsyncMode,
) {
    if mode == AsyncMode::Off {
        return;
    }
    writer().enqueue(output, decorations, decorators, msg, mode);
}

pub fn flush() {
    writer().flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::logging::logOutput::NullOutput;
    use crate::share::logging::logTag::LogTag;
    use crate::share::logging::logTagSet::LogTagSet;

    #[test]
    fn async_enqueue_and_flush() {
        let ts = LogTagSet::new(&[LogTag::Logging]);
        let deco = LogDecorations::new(
            crate::share::logging::logLevel::LogLevel::Info,
            &ts,
            &Decorations::none(),
        );
        let out: Arc<dyn LogOutput> = Arc::new(NullOutput);
        for _ in 0..100 {
            enqueue_record(
                out.clone(),
                deco.clone(),
                Decorations::none(),
                "async record".to_string(),
                AsyncMode::Drop,
            );
        }
        flush();
    }
}
