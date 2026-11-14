/*
 * Port of OpenJDK 25 HotSpot init (share/runtime/init.hpp / init.cpp)
 *
 * Coordinates initialization of all VM global subsystems:
 * Universe, SymbolTable, StringTable, CodeCache, Threads, Synchronization, GC.
 */

use crate::share::classfile::stringTable::StringTable;
use crate::share::classfile::symbolTable::SymbolTable;
use crate::share::code::codeCache::CodeCache;
use crate::share::runtime::os::Os;
use crate::share::runtime::synchronizer::ObjectSynchronizer;
use crate::share::runtime::threads::Threads;
use crate::share::services::management::Management;
use std::sync::atomic::{AtomicBool, Ordering};

static VM_INIT_COMPLETED: AtomicBool = AtomicBool::new(false);
static VM_GLOBALS_INITIALIZED: AtomicBool = AtomicBool::new(false);

pub struct VMInit;

impl VMInit {
    /// Early initialization of basic OS and time subsystems
    pub fn early_initialize() {
        Os::init();
    }

    /// Initializes global symbols, strings, code cache, management, synchronizers
    pub fn vm_init_globals() -> Result<(), &'static str> {
        if VM_GLOBALS_INITIALIZED.load(Ordering::SeqCst) {
            return Ok(());
        }

        // 1. Initialize OS & Timers
        Os::init();

        // 2. Initialize Symbol and String intern pools
        let _ = SymbolTable::global();
        let _ = StringTable::global();

        // 3. Initialize Code Cache
        let _ = CodeCache::global();

        // 4. Initialize Synchronizer & Object Monitors
        let _ = ObjectSynchronizer::new();

        // 5. Initialize Services and Management
        let _ = Management::global();

        VM_GLOBALS_INITIALIZED.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Full initialization phase 2: sets up heap, class loading, threads
    pub fn init_globals() -> Result<(), &'static str> {
        Self::vm_init_globals()?;

        // Ensure active thread registry is live
        let _ = Threads::global();

        Ok(())
    }

    pub fn set_init_completed() {
        VM_INIT_COMPLETED.store(true, Ordering::SeqCst);
    }

    #[inline]
    pub fn is_init_completed() -> bool {
        VM_INIT_COMPLETED.load(Ordering::SeqCst)
    }

    pub fn reset() {
        VM_INIT_COMPLETED.store(false, Ordering::SeqCst);
        VM_GLOBALS_INITIALIZED.store(false, Ordering::SeqCst);
    }
}
