/*
 * Port of OpenJDK 25 HotSpot SymbolTable (share/classfile/symbolTable.hpp, symbolTable.cpp)
 *
 * The SymbolTable is the global thread-safe interning table for VM symbols.
 * HotSpot uses SymbolTable to ensure that distinct instances of the same utf8
 * string share a single canonical Symbol handle.
 */

use crate::share::oops::symbol::Symbol;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock};

pub const SYMBOL_ALLOC_BATCH_SIZE: usize = 8;
pub const SYMBOL_ALLOC_ARENA_SIZE: usize = 360 * 1024;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TableStatistics {
    pub table_size: usize,
    pub number_of_entries: usize,
    pub load_factor: f64,
    pub empty_buckets: usize,
    pub literal_bytes: usize,
}

pub fn hash_symbol(bytes: &[u8], use_alt: bool) -> u32 {
    if use_alt {
        let mut h: u32 = 0x811c9dc5;
        for &b in bytes {
            h ^= b as u32;
            h = h.wrapping_mul(16777619);
        }
        h
    } else {
        let mut h: u32 = 0;
        for &b in bytes {
            h = (31u32).wrapping_mul(h).wrapping_add(b as u32);
        }
        h
    }
}

pub fn unicode_to_modified_utf8(chars: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(chars.len());
    for &ch in chars {
        if ch == 0 {
            out.push(0xc0);
            out.push(0x80);
        } else if ch <= 0x7f {
            out.push(ch as u8);
        } else if ch <= 0x7ff {
            out.push((0xc0 | (ch >> 6)) as u8);
            out.push((0x80 | (ch & 0x3f)) as u8);
        } else {
            out.push((0xe0 | (ch >> 12)) as u8);
            out.push((0x80 | ((ch >> 6) & 0x3f)) as u8);
            out.push((0x80 | (ch & 0x3f)) as u8);
        }
    }
    out
}

pub struct SymbolTable {
    table: RwLock<HashMap<String, Symbol>>,
    permanent_symbols: RwLock<HashSet<String>>,
    needs_rehashing: AtomicBool,
    has_work: AtomicBool,
    use_alt_hash: AtomicBool,
}

static GLOBAL_SYMBOL_TABLE: OnceLock<SymbolTable> = OnceLock::new();

impl SymbolTable {
    pub fn new() -> Self {
        Self {
            table: RwLock::new(HashMap::new()),
            permanent_symbols: RwLock::new(HashSet::new()),
            needs_rehashing: AtomicBool::new(false),
            has_work: AtomicBool::new(false),
            use_alt_hash: AtomicBool::new(false),
        }
    }

    pub fn global() -> &'static SymbolTable {
        GLOBAL_SYMBOL_TABLE.get_or_init(Self::new)
    }

    pub fn lookup_only(&self, name: &str) -> Option<Symbol> {
        let read_guard = self.table.read().unwrap();
        read_guard.get(name).cloned()
    }

    pub fn lookup_only_unicode(&self, chars: &[u16]) -> Option<Symbol> {
        let utf8 = unicode_to_modified_utf8(chars);
        let s = String::from_utf8_lossy(&utf8);
        self.lookup_only(&s)
    }

    pub fn probe(&self, name: &str) -> Option<Symbol> {
        self.lookup_only(name)
    }

    pub fn probe_unicode(&self, chars: &[u16]) -> Option<Symbol> {
        self.lookup_only_unicode(chars)
    }

    pub fn new_symbol(&self, name: &str) -> Symbol {
        {
            let read_guard = self.table.read().unwrap();
            if let Some(sym) = read_guard.get(name) {
                return sym.clone();
            }
        }

        let mut write_guard = self.table.write().unwrap();
        if let Some(sym) = write_guard.get(name) {
            return sym.clone();
        }

        let sym = Symbol::from(name);
        write_guard.insert(name.to_string(), sym.clone());
        sym
    }

    pub fn new_symbol_from_utf8(&self, bytes: &[u8]) -> Symbol {
        let s = String::from_utf8_lossy(bytes);
        self.new_symbol(&s)
    }

    pub fn new_symbol_slice(&self, sym: &Symbol, begin: usize, end: usize) -> Symbol {
        let bytes = sym.as_bytes();
        let slice = if begin <= end && end <= bytes.len() {
            &bytes[begin..end]
        } else {
            &[]
        };
        self.new_symbol_from_utf8(slice)
    }

    pub fn new_symbol_unicode(&self, chars: &[u16]) -> Symbol {
        let utf8 = unicode_to_modified_utf8(chars);
        self.new_symbol_from_utf8(&utf8)
    }

    pub fn new_permanent_symbol(&self, name: &str) -> Symbol {
        let sym = self.new_symbol(name);
        self.permanent_symbols
            .write()
            .unwrap()
            .insert(name.to_string());
        sym
    }

    pub fn contains(&self, name: &str) -> bool {
        self.table.read().unwrap().contains_key(name)
    }

    pub fn symbols_count(&self) -> usize {
        self.table.read().unwrap().len()
    }

    pub fn table_size(&self) -> usize {
        let count = self.symbols_count();
        if count == 0 {
            16
        } else {
            count.next_power_of_two()
        }
    }

    pub fn get_table_statistics(&self) -> TableStatistics {
        let guard = self.table.read().unwrap();
        let entries = guard.len();
        let literal_bytes = guard.keys().map(|k| k.len()).sum();
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

    pub fn trigger_cleanup(&self) {
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
            self.trigger_cleanup();
        }
    }

    pub fn rehash_table(&self) {
        self.use_alt_hash.store(true, Ordering::Relaxed);
        self.needs_rehashing.store(false, Ordering::Relaxed);
    }

    pub fn symbols_do<F: FnMut(&Symbol)>(&self, mut f: F) {
        let guard = self.table.read().unwrap();
        for sym in guard.values() {
            f(sym);
        }
    }

    pub fn clear(&self) {
        self.table.write().unwrap().clear();
        self.permanent_symbols.write().unwrap().clear();
        self.needs_rehashing.store(false, Ordering::Relaxed);
        self.has_work.store(false, Ordering::Relaxed);
    }

    pub fn verify(&self) -> bool {
        let guard = self.table.read().unwrap();
        for (k, sym) in guard.iter() {
            if sym.as_str() != k {
                return false;
            }
        }
        true
    }

    // Static helper methods reflecting HotSpot AllStatic API
    pub fn probe_symbol(name: &str) -> Option<Symbol> {
        Self::global().probe(name)
    }

    pub fn probe_unicode_symbol(chars: &[u16]) -> Option<Symbol> {
        Self::global().probe_unicode(chars)
    }

    pub fn new_symbol_str(name: &str) -> Symbol {
        Self::global().new_symbol(name)
    }

    pub fn new_permanent_symbol_str(name: &str) -> Symbol {
        Self::global().new_permanent_symbol(name)
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

    pub fn trigger_cleanup_static() {
        Self::global().trigger_cleanup();
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

impl Default for SymbolTable {
    fn default() -> Self {
        Self::new()
    }
}
