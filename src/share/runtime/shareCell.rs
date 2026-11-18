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

struct Inner<T> {
    value: UnsafeCell<T>,
    /// Number of outstanding shared borrows; `-1` while mutably borrowed.
    borrows: Cell<isize>,
}

pub struct Share<T>(ReentrantMutex<Inner<T>>);

impl<T> Share<T> {
    pub fn new(value: T) -> Self {
        Self(ReentrantMutex::new(Inner {
            value: UnsafeCell::new(value),
            borrows: Cell::new(0),
        }))
    }

    pub fn borrow(&self) -> ShareRef<'_, T> {
        let guard = match self.0.try_lock() {
            Some(guard) => guard,
            None => {
                if std::env::var_os("JVM25_BLOCKTRACE").is_some() {
                    eprintln!("[block] share-borrow-wait tid={:?}", std::thread::current().id());
                }
                self.0.lock()
            }
        };
        if guard.borrows.get() < 0 {
            panic!("already mutably borrowed");
        }
        guard.borrows.set(guard.borrows.get() + 1);
        // SAFETY: the mutex is held for the guard's lifetime, so no other
        // thread can touch `value`; the borrow counter was raised above.
        let ptr = unsafe { &*guard.value.get() } as *const T;
        ShareRef {
            _guard: guard,
            ptr,
            _marker: PhantomData,
        }
    }

    pub fn borrow_mut(&self) -> ShareMut<'_, T> {
        let guard = match self.0.try_lock() {
            Some(guard) => guard,
            None => {
                if std::env::var_os("JVM25_BLOCKTRACE").is_some() {
                    eprintln!("[block] share-borrowmut-wait tid={:?}", std::thread::current().id());
                }
                self.0.lock()
            }
        };
        if guard.borrows.get() != 0 {
            panic!(
                "already borrowed: Share<{}> borrows={}",
                std::any::type_name::<T>(),
                guard.borrows.get()
            );
        }
        guard.borrows.set(-1);
        // SAFETY: same as `borrow`, plus the borrow counter marks exclusive
        // access for the guard's lifetime.
        let ptr = unsafe { &mut *guard.value.get() } as *mut T;
        ShareMut {
            _guard: guard,
            ptr,
            _marker: PhantomData,
        }
    }

    pub fn try_borrow(&self) -> Option<ShareRef<'_, T>> {
        let guard = self.0.try_lock()?;
        if guard.borrows.get() < 0 {
            return None;
        }
        guard.borrows.set(guard.borrows.get() + 1);
        let ptr = unsafe { &*guard.value.get() } as *const T;
        Some(ShareRef {
            _guard: guard,
            ptr,
            _marker: PhantomData,
        })
    }

    pub fn try_borrow_mut(&self) -> Option<ShareMut<'_, T>> {
        let guard = self.0.try_lock()?;
        if guard.borrows.get() != 0 {
            return None;
        }
        guard.borrows.set(-1);
        let ptr = unsafe { &mut *guard.value.get() } as *mut T;
        Some(ShareMut {
            _guard: guard,
            ptr,
            _marker: PhantomData,
        })
    }

    /// Runs `f` with a shared borrow of the contents.
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.borrow())
    }

    /// Runs `f` with an exclusive borrow of the contents.
    pub fn with_mut<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        f(&mut self.borrow_mut())
    }
}

impl<T: Copy> Share<T> {
    pub fn get(&self) -> T {
        *self.borrow()
    }

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
        self._guard.borrows.set(self._guard.borrows.get() - 1);
    }
}

pub struct ShareMut<'a, T> {
    _guard: ReentrantMutexGuard<'a, Inner<T>>,
    ptr: *mut T,
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
    }
}
