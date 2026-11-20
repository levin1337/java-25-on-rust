//! Thread-safe cell used for all shared JVM state.
//!
//! `Share<T>` combines a re-entrant mutex with RefCell-style borrow
//! tracking:
//! - borrows on the same OS thread behave like `RefCell` (shared borrows
//!   coexist, mutable borrow is exclusive, conflicts panic),
//! - different Java threads (each on its own OS thread) serialize on the
//!   mutex instead of racing.
//!
//! `parking_lot::ReentrantMutexGuard` only implements `Deref`, so the
//! payload lives in an `UnsafeCell` and borrow flags in a `Cell`; the
//! mutex guarantees no concurrent access across threads and the manual
//! counters reproduce `RefCell` exclusivity on the owning thread.

use parking_lot::{ReentrantMutex, ReentrantMutexGuard};
use std::cell::Cell;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

static NEXT_OS_TID: AtomicU64 = AtomicU64::new(1);
thread_local! {
    static OS_TID: u64 = NEXT_OS_TID.fetch_add(1, Ordering::Relaxed);
}

fn os_tid() -> u64 {
    OS_TID.with(|t| *t)
}

struct Inner<T> {
    value: UnsafeCell<T>,
    /// Number of outstanding shared borrows; `-1` while mutably borrowed.
    borrows: Cell<isize>,
}

pub struct Share<T> {
    inner: ReentrantMutex<Inner<T>>,
    /// OS-thread id currently holding the lock (0 = free). Debug aid for
    /// `JVM25_BLOCKTRACE`; only meaningful while the mutex is contended.
    holder: AtomicU64,
    /// `&'static Location` of the callsite that acquired the lock,
    /// stored as a raw pointer for debug printing.
    holder_site: AtomicUsize,
    /// Leaked `Backtrace` captured at the last 0->1 borrow when
    /// `JVM25_SHARE_TRACE` is set; printed by `borrow_mut` conflicts.
    holder_bt: AtomicUsize,
}

impl<T> Share<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: ReentrantMutex::new(Inner {
                value: UnsafeCell::new(value),
                borrows: Cell::new(0),
            }),
            holder: AtomicU64::new(0),
            holder_site: AtomicUsize::new(0),
            holder_bt: AtomicUsize::new(0),
        }
    }

    #[track_caller]
    fn mark_holder(&self) {
        self.holder.store(os_tid(), Ordering::Relaxed);
        self.holder_site
            .store(std::panic::Location::caller() as *const _ as usize, Ordering::Relaxed);
        if std::env::var_os("JVM25_SHARE_TRACE").is_some()
            && std::any::type_name::<T>().contains("ObjArrayOop")
        {
            let bt = Box::new(std::backtrace::Backtrace::force_capture());
            self.holder_bt.store(Box::into_raw(bt) as usize, Ordering::Relaxed);
        }
    }

    fn holder_desc(&self) -> String {
        if self.holder.load(Ordering::Relaxed) == 0 {
            return String::new();
        }
        let site = self.holder_site.load(Ordering::Relaxed);
        if site == 0 {
            return String::new();
        }
        let loc = unsafe { &*(site as *const std::panic::Location<'static>) };
        format!(" at {loc}")
    }

    #[track_caller]
    pub fn borrow(&self) -> ShareRef<'_, T> {
        let guard = match self.inner.try_lock() {
            Some(guard) => guard,
            None => {
                if std::env::var_os("JVM25_BLOCKTRACE").is_some() {
                    eprintln!(
                        "[block] share-borrow-wait tid={} holder={} share={:p}{} frame={}",
                        os_tid(),
                        self.holder.load(Ordering::Relaxed),
                        self,
                        self.holder_desc(),
                        crate::share::interpreter::bytecodeInterpreter::current_frame()
                    );
                }
                // Parked on a VM-internal mutex: register as GC-blocked so
                // a pending collection neither deadlocks nor misses roots.
                crate::share::memory::heap::quiesce();
                let guard = self.inner.lock();
                crate::share::memory::heap::unquiesce();
                guard
            }
        };
        if guard.borrows.get() < 0 {
            panic!("already mutably borrowed");
        }
        if guard.borrows.get() == 0 {
            self.mark_holder();
        }
        guard.borrows.set(guard.borrows.get() + 1);
        // SAFETY: the mutex is held for the guard's lifetime, so no other
        // thread can touch `value`; the borrow counter was raised above.
        let ptr = unsafe { &*guard.value.get() } as *const T;
        ShareRef {
            _guard: guard,
            ptr,
            holder: &self.holder,
            _marker: PhantomData,
        }
    }

    #[track_caller]
    pub fn borrow_mut(&self) -> ShareMut<'_, T> {
        let guard = match self.inner.try_lock() {
            Some(guard) => guard,
            None => {
                if std::env::var_os("JVM25_BLOCKTRACE").is_some() {
                    eprintln!(
                        "[block] share-borrowmut-wait tid={} holder={} share={:p}{} frame={}",
                        os_tid(),
                        self.holder.load(Ordering::Relaxed),
                        self,
                        self.holder_desc(),
                        crate::share::interpreter::bytecodeInterpreter::current_frame()
                    );
                }
                crate::share::memory::heap::quiesce();
                let guard = self.inner.lock();
                crate::share::memory::heap::unquiesce();
                guard
            }
        };
        if guard.borrows.get() != 0 {
            if std::env::var_os("JVM25_SHARE_TRACE").is_some() {
                let bt = self.holder_bt.load(Ordering::Relaxed);
                let bt_txt = if bt != 0 {
                    format!("\n{}", unsafe { &*(bt as *const std::backtrace::Backtrace) })
                } else {
                        String::new()
                };
                eprintln!(
                    "[share-busy] Share<{}> borrows={}{}{}\n{}",
                    std::any::type_name::<T>(),
                    guard.borrows.get(),
                    self.holder_desc(),
                    bt_txt,
                    std::backtrace::Backtrace::force_capture()
                );
            }
            panic!(
                "already borrowed: Share<{}> borrows={}{}",
                std::any::type_name::<T>(),
                guard.borrows.get(),
                self.holder_desc()
            );
        }
        self.mark_holder();
        guard.borrows.set(-1);
        // SAFETY: same as `borrow`, plus the borrow counter marks exclusive
        // access for the guard's lifetime.
        let ptr = unsafe { &mut *guard.value.get() } as *mut T;
        ShareMut {
            _guard: guard,
            ptr,
            holder: &self.holder,
            _marker: PhantomData,
        }
    }

    #[track_caller]
    pub fn try_borrow(&self) -> Option<ShareRef<'_, T>> {
        let guard = self.inner.try_lock()?;
        if guard.borrows.get() < 0 {
            return None;
        }
        if guard.borrows.get() == 0 {
            self.mark_holder();
        }
        guard.borrows.set(guard.borrows.get() + 1);
        let ptr = unsafe { &*guard.value.get() } as *const T;
        Some(ShareRef {
            _guard: guard,
            ptr,
            holder: &self.holder,
            _marker: PhantomData,
        })
    }

    #[track_caller]
    pub fn try_borrow_mut(&self) -> Option<ShareMut<'_, T>> {
        let guard = self.inner.try_lock()?;
        if guard.borrows.get() != 0 {
            return None;
        }
        self.mark_holder();
        guard.borrows.set(-1);
        let ptr = unsafe { &mut *guard.value.get() } as *mut T;
        Some(ShareMut {
            _guard: guard,
            ptr,
            holder: &self.holder,
            _marker: PhantomData,
        })
    }

    /// Runs `f` with a shared borrow of the contents.
    #[track_caller]
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.borrow())
    }

    /// Runs `f` with an exclusive borrow of the contents.
    #[track_caller]
    pub fn with_mut<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        f(&mut self.borrow_mut())
    }
}

impl<T: Copy> Share<T> {
    #[track_caller]
    pub fn get(&self) -> T {
        *self.borrow()
    }

    #[track_caller]
    pub fn set(&self, value: T) {
        *self.borrow_mut() = value;
    }
}

impl<T> Default for Share<T>
where
    T: Default,
{
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> UnwindSafe for Share<T> {}
impl<T> RefUnwindSafe for Share<T> {}

pub struct ShareRef<'a, T> {
    _guard: ReentrantMutexGuard<'a, Inner<T>>,
    ptr: *const T,
    holder: &'a AtomicU64,
    _marker: PhantomData<&'a T>,
}

impl<T> Deref for ShareRef<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: `ptr` points at `Inner.value`, kept alive by `_guard`;
        // the shared-borrow count was raised for the guard's lifetime.
        unsafe { &*self.ptr }
    }
}

impl<T> Drop for ShareRef<'_, T> {
    fn drop(&mut self) {
        let borrows = self._guard.borrows.get() - 1;
        self._guard.borrows.set(borrows);
        if borrows == 0 {
            self.holder.store(0, Ordering::Relaxed);
        }
    }
}

pub struct ShareMut<'a, T> {
    _guard: ReentrantMutexGuard<'a, Inner<T>>,
    ptr: *mut T,
    holder: &'a AtomicU64,
    _marker: PhantomData<&'a mut T>,
}

impl<T> Deref for ShareMut<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.ptr }
    }
}

impl<T> DerefMut for ShareMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.ptr }
    }
}

impl<T> Drop for ShareMut<'_, T> {
    fn drop(&mut self) {
        self._guard.borrows.set(0);
        self.holder.store(0, Ordering::Relaxed);
    }
}
