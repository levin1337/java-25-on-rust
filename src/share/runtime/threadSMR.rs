use super::javaThread::{JavaThread, JavaThreadState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadSnapshot {
    pub id: u64,
    pub name: String,
    pub state: JavaThreadState,
    pub daemon: bool,
}

impl From<&JavaThread> for ThreadSnapshot {
    fn from(thread: &JavaThread) -> Self {
        Self {
            id: thread.id(),
            name: thread.name().to_owned(),
            state: thread.thread_state(),
            daemon: thread.is_daemon(),
        }
    }
}
