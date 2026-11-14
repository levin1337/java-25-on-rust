/*
 * Port of OpenJDK 25 HotSpot StringTable (share/classfile/stringTable.hpp, stringTable.cpp)
 *
 * StringTable manages the interned Java String pool (backing String.intern()).
 */

use super::symbolTable::{TableStatistics, unicode_to_modified_utf8};
use crate::share::oops::symbol::Symbol;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock};

pub struct StringTable {
    pool: RwLock<HashSet<String>>,
    needs_rehashing: AtomicBool,
    has_work: AtomicBool,
}

static GLOBAL_STRING_TABLE: OnceLock<StringTable> = OnceLock::new();

impl StringTable {
    pub fn new() -> Self {
        Self {
            pool: RwLock::new(HashSet::new()),
            needs_rehashing: AtomicBool::new(false),
            has_work: AtomicBool::new(false),
        }
    }

    pub fn global() -> &'static StringTable {
        GLOBAL_STRING_TABLE.get_or_init(Self::new)
    }

    pub fn lookup(&self, text: &str) -> Option<String> {
        let read_guard = self.pool.read().unwrap();
        read_guard.get(text).cloned()
    }

    pub fn lookup_symbol(&self, sym: &Symbol) -> Option<String> {
        self.lookup(sym.as_str())
    }

    pub fn lookup_unicode(&self, chars: &[u16]) -> Option<String> {
        let s = String::from_utf16_lossy(chars);
        self.lookup(&s)
    }

    pub fn intern(&self, text: &str) -> String {
        {
            let read_guard = self.pool.read().unwrap();
            if let Some(existing) = read_guard.get(text) {
                return existing.clone();
            }
        }

        let mut write_guard = self.pool.write().unwrap();
        if let Some(existing) = write_guard.get(text) {
            return existing.clone();
        }

        let string = text.to_string();
        write_guard.insert(string.clone());
        string
    }

    pub fn intern_symbol(&self, sym: &Symbol) -> String {
        self.intern(sym.as_str())
    }

    pub fn intern_utf8(&self, bytes: &[u8]) -> String {
        let s = String::from_utf8_lossy(bytes);
        self.intern(&s)
    }

    pub fn intern_unicode(&self, chars: &[u16]) -> String {
        let utf8 = unicode_to_modified_utf8(chars);
        self.intern_utf8(&utf8)
    }

    pub fn contains(&self, text: &str) -> bool {
        self.pool.read().unwrap().contains(text)
    }

    pub fn intern_count(&self) -> usize {
        self.pool.read().unwrap().len()
    }

    pub fn table_size(&self) -> usize {
        let count = self.intern_count();
        if count == 0 {
            16
        } else {
            count.next_power_of_two()
        }
    }

    pub fn get_table_statistics(&self) -> TableStatistics {
        let guard = self.pool.read().unwrap();
        let entries = guard.len();
        let literal_bytes = guard.iter().map(|s| s.len()).sum();
        let size = if entries == 0 {
            16
        } else {
            entries.next_power_of_two()
        };
        let load_factor = if size > 0 {
            entries as f64 / size as f64
        } else {
            0.0
        };
        TableStatistics {
            table_size: size,
            number_of_entries: entries,
            load_factor,
            empty_buckets: size.saturating_sub(entries),
            literal_bytes,
        }
    }

    pub fn has_work(&self) -> bool {
        self.has_work.load(Ordering::Relaxed)
    }

    pub fn trigger_concurrent_work(&self) {
        self.has_work.store(true, Ordering::Relaxed);
    }

    pub fn do_concurrent_work(&self) {
        self.has_work.store(false, Ordering::Relaxed);
    }

    pub fn needs_rehashing(&self) -> bool {
        self.needs_rehashing.load(Ordering::Relaxed)
    }

    pub fn update_needs_rehash(&self, rehash: bool) {
        if rehash {
            self.needs_rehashing.store(true, Ordering::Relaxed);
            self.trigger_concurrent_work();
        }
    }

    pub fn rehash_table(&self) {
        self.needs_rehashing.store(false, Ordering::Relaxed);
    }

    pub fn strings_do<F: FnMut(&str)>(&self, mut f: F) {
        let guard = self.pool.read().unwrap();
        for s in guard.iter() {
            f(s);
        }
    }

    pub fn wrapped_string_equals(str1: &str, str2: &str) -> bool {
        str1 == str2
    }

    pub fn verify(&self) -> bool {
        let guard = self.pool.read().unwrap();
        for s in guard.iter() {
            if s.is_empty() && self.intern_count() > 1 && !guard.contains("") {
                return false;
            }
        }
        true
    }

    pub fn clear(&self) {
        self.pool.write().unwrap().clear();
        self.needs_rehashing.store(false, Ordering::Relaxed);
        self.has_work.store(false, Ordering::Relaxed);
    }

    // Static helper methods reflecting HotSpot AllStatic API
    pub fn lookup_str(text: &str) -> Option<String> {
        Self::global().lookup(text)
    }

    pub fn intern_str(text: &str) -> String {
        Self::global().intern(text)
    }

    pub fn table_size_static() -> usize {
        Self::global().table_size()
    }

    pub fn get_table_statistics_static() -> TableStatistics {
        Self::global().get_table_statistics()
    }

    pub fn has_work_static() -> bool {
        Self::global().has_work()
    }

    pub fn do_concurrent_work_static() {
        Self::global().do_concurrent_work();
    }

    pub fn rehash_table_static() {
        Self::global().rehash_table();
    }

    pub fn verify_static() -> bool {
        Self::global().verify()
    }
}

impl Default for StringTable {
    fn default() -> Self {
        Self::new()
    }
}
