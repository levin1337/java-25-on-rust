use crate::share::oops::oop::Oop;
use std::{
    collections::HashMap,
    io::{self, Error},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread::{self, ThreadId},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JniReference(usize);

impl JniReference {
    pub fn asRaw(self) -> *mut std::ffi::c_void {
        std::ptr::without_provenance_mut(self.0)
    }

    pub(crate) fn fromRaw(value: *mut std::ffi::c_void) -> Option<Self> {
        let id = value.addr();
        (id != 0).then_some(Self(id))
    }

    pub(crate) fn intoRaw(value: Option<Self>) -> *mut std::ffi::c_void {
        std::ptr::without_provenance_mut(value.map_or(0, |reference| reference.0))
    }
}

/// Object a JNI reference points to. `Class` ids index the per-VM class
/// registry kept by `jniInvocation` (java.lang.Class mirrors are lightweight
/// `Value::ClassMirror` tokens in this VM, so classes need their own slot).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JniRefTarget {
    Object(Oop),
    Class(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JniRefType {
    Invalid,
    Local,
    Global,
    WeakGlobal,
}

#[derive(Clone, Copy)]
struct Entry {
    target: JniRefTarget,
    owner: Option<ThreadId>,
    weak: bool,
}

#[derive(Default)]
struct State {
    entries: HashMap<JniReference, Entry>,
    frames: HashMap<ThreadId, Vec<Vec<JniReference>>>,
}

pub struct JniReferences {
    next: AtomicUsize,
    state: Mutex<State>,
}

impl Default for JniReferences {
    fn default() -> Self {
        Self {
            next: AtomicUsize::new(1),
            state: Mutex::new(State::default()),
        }
    }
}


thread_local! {
    /// Thread identity captured on first use, while the thread's TLS is
    /// still alive. `std::thread::current()` panics after TLS destruction,
    /// which happens when native shutdown callbacks (e.g. JNA disposers
    /// during `LdrShutdownThread`) call back into the VM.
    static THREAD_ID: std::cell::Cell<Option<ThreadId>> = const { std::cell::Cell::new(None) };
}

static SHUTDOWN_THREAD_ID: std::sync::OnceLock<ThreadId> = std::sync::OnceLock::new();

/// Captures a dedicated fallback thread identity for callbacks that arrive
/// after a thread's TLS was destroyed. The owner thread parks forever, so
/// the bucket never collides with live thread state.
pub fn init_shutdown_thread_id() {
    let _ = SHUTDOWN_THREAD_ID.get_or_init(|| {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(thread::current().id());
            loop {
                std::thread::park();
            }
        });
        receiver.recv().unwrap()
    });
}

/// Panic-free `std::thread::current().id()`.
pub fn current_thread_id() -> ThreadId {
    match THREAD_ID.try_with(|cell| {
        if let Some(id) = cell.get() {
            return id;
        }
        let id = thread::current().id();
        cell.set(Some(id));
        id
    }) {
        Ok(id) => id,
        Err(_) => *SHUTDOWN_THREAD_ID.get_or_init(|| {
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = sender.send(thread::current().id());
                loop {
                    std::thread::park();
                }
            });
            receiver.recv().unwrap()
        }),
    }
}

impl JniReferences {
    pub fn new() -> Self {
        Self::default()
    }

    fn insert(
        &self,
        state: &mut State,
        target: JniRefTarget,
        owner: Option<ThreadId>,
        weak: bool,
    ) -> io::Result<Option<JniReference>> {
        if matches!(target, JniRefTarget::Object(Oop::Null)) {
            return Ok(None);
        }
        let id = self
            .next
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| Error::other("JNI reference identifiers exhausted"))?;
        let reference = JniReference(id);
        state.entries.insert(
            reference,
            Entry {
                target,
                owner,
                weak,
            },
        );
        if let Some(thread) = owner {
            state
                .frames
                .entry(thread)
                .or_insert_with(|| vec![Vec::new()])
                .last_mut()
                .unwrap()
                .push(reference);
        }
        Ok(Some(reference))
    }

    pub fn newLocal(&self, object: Oop) -> io::Result<Option<JniReference>> {
        self.newLocalTarget(JniRefTarget::Object(object))
    }

    pub fn newLocalTarget(&self, target: JniRefTarget) -> io::Result<Option<JniReference>> {
        self.insert(
            &mut self.state.lock().unwrap(),
            target,
            Some(current_thread_id()),
            false,
        )
    }

    pub fn newGlobal(&self, object: Oop) -> io::Result<Option<JniReference>> {
        self.newGlobalTarget(JniRefTarget::Object(object))
    }

    pub fn newGlobalTarget(&self, target: JniRefTarget) -> io::Result<Option<JniReference>> {
        self.insert(&mut self.state.lock().unwrap(), target, None, false)
    }

    pub fn newWeakGlobal(&self, target: JniRefTarget) -> io::Result<Option<JniReference>> {
        self.insert(&mut self.state.lock().unwrap(), target, None, true)
    }

    pub fn resolveTarget(&self, reference: Option<JniReference>) -> io::Result<JniRefTarget> {
        let Some(reference) = reference else {
            return Ok(JniRefTarget::Object(Oop::Null));
        };
        let state = self.state.lock().unwrap();
        let entry = state
            .entries
            .get(&reference)
            .ok_or_else(|| Error::other("Invalid JNI reference"))?;
        if entry
            .owner
            .is_some_and(|owner| owner != current_thread_id())
        {
            return Err(Error::other(
                "JNI local reference belongs to another thread",
            ));
        }
        Ok(entry.target)
    }

    /// All strong JNI references as GC roots (local and global).
    /// Weak globals and `Class` targets are not object roots.
    pub fn referenced_oops(&self) -> Vec<Oop> {
        let state = self.state.lock().unwrap();
        state
            .entries
            .values()
            .filter(|entry| !entry.weak)
            .filter_map(|entry| match entry.target {
                JniRefTarget::Object(oop) => Some(oop),
                JniRefTarget::Class(_) => None,
            })
            .collect()
    }

    pub fn resolve(&self, reference: Option<JniReference>) -> io::Result<Oop> {
        match self.resolveTarget(reference)? {
            JniRefTarget::Object(object) => Ok(object),
            JniRefTarget::Class(_) => Err(Error::other("JNI reference is a class")),
        }
    }

    pub fn refType(&self, reference: Option<JniReference>) -> JniRefType {
        let Some(reference) = reference else {
            return JniRefType::Invalid;
        };
        let state = self.state.lock().unwrap();
        match state.entries.get(&reference) {
            Some(entry) if entry.owner.is_none() && entry.weak => JniRefType::WeakGlobal,
            Some(entry) if entry.owner.is_none() => JniRefType::Global,
            Some(_) => JniRefType::Local,
            None => JniRefType::Invalid,
        }
    }

    /// Deletes a global or weak-global reference. `weak` selects which kind;
    /// local references must go through `deleteLocal`-style paths (frames).
    pub fn delete(&self, reference: Option<JniReference>, global: bool) -> io::Result<()> {
        self.deleteKind(reference, global, false)
    }

    pub fn deleteWeak(&self, reference: Option<JniReference>) -> io::Result<()> {
        self.deleteKind(reference, true, true)
    }

    fn deleteKind(
        &self,
        reference: Option<JniReference>,
        global: bool,
        weak: bool,
    ) -> io::Result<()> {
        let Some(reference) = reference else {
            return Ok(());
        };
        let mut state = self.state.lock().unwrap();
        let entry = state
            .entries
            .get(&reference)
            .ok_or_else(|| Error::other("Invalid JNI reference"))?;
        if entry.owner.is_none() != global
            || entry.weak != weak
            || entry
                .owner
                .is_some_and(|owner| owner != current_thread_id())
        {
            return Err(Error::other("Invalid JNI reference kind or owner"));
        }
        if let Some(owner) = entry.owner
            && let Some(frames) = state.frames.get_mut(&owner)
        {
            for frame in frames {
                if let Some(index) = frame.iter().position(|item| *item == reference) {
                    frame.swap_remove(index);
                    break;
                }
            }
        }
        state.entries.remove(&reference);
        Ok(())
    }

    /// Deletes a single local reference outside of frame bookkeeping.
    pub fn deleteLocal(&self, reference: Option<JniReference>) -> io::Result<()> {
        self.deleteKind(reference, false, false)
    }

    pub fn pushFrame(&self) {
        self.state
            .lock()
            .unwrap()
            .frames
            .entry(current_thread_id())
            .or_insert_with(|| vec![Vec::new()])
            .push(Vec::new());
    }

    pub fn ensureCapacity(&self, capacity: usize) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.entries.try_reserve(capacity).map_err(Error::other)?;
        state
            .frames
            .entry(current_thread_id())
            .or_insert_with(|| vec![Vec::new()])
            .last_mut()
            .unwrap()
            .try_reserve(capacity)
            .map_err(Error::other)
    }

    pub fn popFrame(&self, result: Option<JniReference>) -> io::Result<Option<JniReference>> {
        let thread = current_thread_id();
        let mut state = self.state.lock().unwrap();
        let target = if let Some(reference) = result {
            let entry = state
                .entries
                .get(&reference)
                .ok_or_else(|| Error::other("Invalid JNI reference"))?;
            if entry.owner.is_some_and(|owner| owner != thread) {
                return Err(Error::other(
                    "JNI local reference belongs to another thread",
                ));
            }
            entry.target
        } else {
            JniRefTarget::Object(Oop::Null)
        };
        let frames = state
            .frames
            .get_mut(&thread)
            .ok_or_else(|| Error::other("Missing JNI local frame"))?;
        if frames.len() <= 1 {
            return Err(Error::other("Cannot pop base JNI local frame"));
        }
        let frame = frames.pop().unwrap();
        for reference in frame {
            state.entries.remove(&reference);
        }
        self.insert(&mut state, target, Some(thread), false)
    }

    /// Pops the top local frame without preserving a result object.
    pub fn popFrameVoid(&self) -> io::Result<()> {
        let thread = current_thread_id();
        let mut state = self.state.lock().unwrap();
        let frames = state
            .frames
            .get_mut(&thread)
            .ok_or_else(|| Error::other("Missing JNI local frame"))?;
        if frames.len() <= 1 {
            return Err(Error::other("Cannot pop base JNI local frame"));
        }
        let frame = frames.pop().unwrap();
        for reference in frame {
            state.entries.remove(&reference);
        }
        Ok(())
    }

    /// Ensures a base local frame exists for the current thread.
    pub fn ensureBaseFrame(&self) {
        self.state
            .lock()
            .unwrap()
            .frames
            .entry(current_thread_id())
            .or_insert_with(|| vec![Vec::new()]);
    }

    pub fn detach(&self) {
        let thread = current_thread_id();
        let mut state = self.state.lock().unwrap();
        if let Some(frames) = state.frames.remove(&thread) {
            for reference in frames.into_iter().flatten() {
                state.entries.remove(&reference);
            }
        }
    }
}
