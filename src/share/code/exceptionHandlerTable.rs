/*
 * Port of OpenJDK 25 HotSpot ExceptionHandlerTable
 * (share/code/exceptionHandlerTable.hpp / exceptionHandlerTable.cpp)
 *
 * An ExceptionHandlerTable is an abstraction over a list of subtables of
 * exception handlers for CatchNodes.  Each subtable has a one-entry header
 * holding the subtable length and catch_pco (the pc offset of the catch
 * site in the nmethod), followed by `length` entries describing each
 * compiled exception handler reachable from that CatchNode.
 *
 *   table    = { subtable }
 *   subtable = header entry { entry }
 *   header   = (subtable entry count, catch pc offset, scope depth=0)
 *   entry    = (handler bci, handler pc offset, scope depth)
 *
 * Additionally a `PcRangeHandlerTable` provides the interpreter-style
 * pc-range -> handler lookup used when unwinding compiled frames:
 * entries are (start_pc, end_pc, handler_pc, catch_type) sorted by start
 * and scanned in order, matching the semantics of the JVM exception
 * table in the class file.
 */

use crate::share::oops::symbol::Symbol;

/// One entry of an ExceptionHandlerTable subtable.
/// For the subtable header `bci` holds the entry count and `pco` the
/// catch_pco; for regular entries `bci` is the exception-handler bci and
/// `pco` the compiled handler pc offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HandlerTableEntry {
    bci: i32,
    pco: i32,
    scope_depth: i32,
}

impl HandlerTableEntry {
    pub fn new(bci: i32, pco: i32, scope_depth: i32) -> Self {
        debug_assert!(pco >= 0, "pco must be positive");
        debug_assert!(scope_depth >= 0, "scope_depth must be positive");
        Self {
            bci,
            pco,
            scope_depth,
        }
    }

    /// Subtable length (only meaningful for header entries).
    pub fn len(&self) -> i32 {
        self.bci
    }

    pub fn bci(&self) -> i32 {
        self.bci
    }

    pub fn pco(&self) -> i32 {
        self.pco
    }

    pub fn scope_depth(&self) -> i32 {
        self.scope_depth
    }
}

/// Compile-time/runtime exception handler subtable structure.
#[derive(Clone, Debug, Default)]
pub struct ExceptionHandlerTable {
    table: Vec<HandlerTableEntry>,
}

impl ExceptionHandlerTable {
    pub fn new(initial_size: usize) -> Self {
        Self {
            table: Vec::with_capacity(initial_size.max(1)),
        }
    }

    /// Runtime view over an nmethod's serialized handler table
    /// (`ExceptionHandlerTable(const nmethod*)` analogue): `entries` is
    /// the raw decoded table.
    pub fn from_entries(entries: Vec<HandlerTableEntry>) -> Self {
        Self { table: entries }
    }

    pub fn add_entry(&mut self, entry: HandlerTableEntry) {
        self.table.push(entry);
    }

    /// Index of the subtable whose header catch_pco equals `catch_pco`.
    pub fn subtable_for(&self, catch_pco: i32) -> Option<usize> {
        let mut i = 0usize;
        while i < self.table.len() {
            let t = &self.table[i];
            if t.pco() == catch_pco {
                return Some(i);
            }
            i += (t.len() as usize) + 1; // +1 for header
        }
        None
    }

    /// `(compile-time) add entries`: registers all handlers for one
    /// catch site.
    #[allow(clippy::too_many_arguments)]
    pub fn add_subtable(
        &mut self,
        catch_pco: i32,
        handler_bcis: &[i32],
        scope_depths_from_top_scope: Option<&[i32]>,
        handler_pcos: &[i32],
    ) {
        debug_assert!(self.subtable_for(catch_pco).is_none());
        debug_assert_eq!(handler_bcis.len(), handler_pcos.len());
        debug_assert!(
            scope_depths_from_top_scope
                .map_or(true, |s| s.len() == handler_bcis.len())
        );
        if !handler_bcis.is_empty() {
            self.add_entry(HandlerTableEntry::new(handler_bcis.len() as i32, catch_pco, 0));
            for (i, (&bci, &pco)) in handler_bcis.iter().zip(handler_pcos.iter()).enumerate() {
                let scope_depth = scope_depths_from_top_scope.map_or(0, |s| s[i]);
                self.add_entry(HandlerTableEntry::new(bci, pco, scope_depth));
                let e = self
                    .entry_for(catch_pco, bci, scope_depth)
                    .expect("entry not added correctly");
                debug_assert_eq!(e.pco(), pco);
                debug_assert_eq!(e.scope_depth(), scope_depth);
            }
        }
    }

    /// Find the handler entry for `(catch_pco, handler_bci, scope_depth)`.
    pub fn entry_for(&self, catch_pco: i32, handler_bci: i32, scope_depth: i32) -> Option<&HandlerTableEntry> {
        let start = self.subtable_for(catch_pco)?;
        let header = &self.table[start];
        let mut l = header.len();
        let mut idx = start;
        while l > 0 {
            idx += 1;
            let t = &self.table[idx];
            if t.bci() == handler_bci && t.scope_depth() == scope_depth {
                return Some(t);
            }
            l -= 1;
        }
        None
    }

    /// Iterate all subtables as (header, entries) slices.
    pub fn subtables(&self) -> Vec<(&HandlerTableEntry, &[HandlerTableEntry])> {
        let mut out = Vec::new();
        let mut i = 0usize;
        while i < self.table.len() {
            let header = &self.table[i];
            let len = header.len() as usize;
            let end = (i + 1 + len).min(self.table.len());
            out.push((header, &self.table[i + 1..end]));
            i = end;
        }
        out
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    pub fn entries(&self) -> &[HandlerTableEntry] {
        &self.table
    }

    /// Serialized size in bytes (3 x i32 per entry, 4-byte aligned).
    pub fn size_in_bytes(&self) -> usize {
        self.table.len() * 12
    }

    /// Encode the table as raw little-endian i32 triples.
    pub fn copy_bytes_to(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.size_in_bytes());
        for e in &self.table {
            out.extend_from_slice(&e.bci.to_le_bytes());
            out.extend_from_slice(&e.pco.to_le_bytes());
            out.extend_from_slice(&e.scope_depth.to_le_bytes());
        }
        out
    }

    /// Decode a serialized table produced by `copy_bytes_to`.
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut table = Vec::with_capacity(bytes.len() / 12);
        for chunk in bytes.chunks_exact(12) {
            table.push(HandlerTableEntry {
                bci: i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]),
                pco: i32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]),
                scope_depth: i32::from_le_bytes([chunk[8], chunk[9], chunk[10], chunk[11]]),
            });
        }
        Self { table }
    }

    pub fn print_on(&self, f: &mut dyn std::fmt::Write, base: Option<usize>) {
        for (header, entries) in self.subtables() {
            match base {
                Some(b) => {
                    let _ = writeln!(
                        f,
                        "catch_pco = {} (pc={}, {} entries)",
                        header.pco(),
                        b as i64 + header.pco() as i64,
                        entries.len()
                    );
                }
                None => {
                    let _ = writeln!(
                        f,
                        "catch_pco = {} ({} entries)",
                        header.pco(),
                        entries.len()
                    );
                }
            }
            for t in entries {
                match base {
                    Some(b) => {
                        let _ = writeln!(
                            f,
                            "  bci {} at scope depth {} -> pco {} (pc={})",
                            t.bci(),
                            t.scope_depth(),
                            t.pco(),
                            b as i64 + t.pco() as i64
                        );
                    }
                    None => {
                        let _ = writeln!(
                            f,
                            "  bci {} at scope depth {} -> pco {}",
                            t.bci(),
                            t.scope_depth(),
                            t.pco()
                        );
                    }
                }
            }
        }
    }
}

/// A single entry of a pc-range exception handler table (the JVM
/// `exception_table` attribute layout lifted to native pc offsets).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PcRangeHandlerEntry {
    pub start_pc: i32,
    pub end_pc: i32,
    pub handler_pc: i32,
    pub catch_type: Option<Symbol>,
}

impl PcRangeHandlerEntry {
    pub fn new(start_pc: i32, end_pc: i32, handler_pc: i32, catch_type: Option<Symbol>) -> Self {
        Self {
            start_pc,
            end_pc,
            handler_pc,
            catch_type,
        }
    }

    #[inline]
    pub fn covers(&self, pc: i32) -> bool {
        pc >= self.start_pc && pc < self.end_pc
    }
}

/// Sorted-by-start_pc table of exception handler ranges covering a
/// compiled method's code (or the interpreter's bytecode exception
/// table expressed in bcis).  Lookup scans in order, first match wins —
/// matching JVM exception dispatch where earlier entries take precedence.
#[derive(Clone, Debug, Default)]
pub struct PcRangeHandlerTable {
    entries: Vec<PcRangeHandlerEntry>,
}

impl PcRangeHandlerTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            entries: Vec::with_capacity(cap),
        }
    }

    /// Insert keeping entries sorted by `start_pc` (stable for equal
    /// starts, preserving class-file order).
    pub fn add_entry(&mut self, entry: PcRangeHandlerEntry) {
        let pos = self
            .entries
            .partition_point(|e| e.start_pc <= entry.start_pc);
        self.entries.insert(pos, entry);
    }

    pub fn add(&mut self, start_pc: i32, end_pc: i32, handler_pc: i32, catch_type: Option<Symbol>) {
        self.add_entry(PcRangeHandlerEntry::new(start_pc, end_pc, handler_pc, catch_type));
    }

    /// First handler whose range covers `pc`.  `catch_type` of `None`
    /// matches any thrown type; `Some(t)` matches only that type or
    /// subtypes — subtype checks are delegated to `is_subtype` when
    /// provided.
    pub fn lookup(&self, pc: i32, thrown_type: &Symbol, is_subtype: Option<&dyn Fn(&Symbol, &Symbol) -> bool>) -> Option<&PcRangeHandlerEntry>
    {
        for e in &self.entries {
            if !e.covers(pc) {
                continue;
            }
            match &e.catch_type {
                None => return Some(e),
                Some(expected) => {
                    let matches = match is_subtype {
                        Some(f) => f(thrown_type, expected),
                        None => thrown_type == expected,
                    };
                    if matches {
                        return Some(e);
                    }
                }
            }
        }
        None
    }

    /// Lookup without subtype resolution (exact catch-type match).
    pub fn lookup_exact(&self, pc: i32, thrown_type: &Symbol) -> Option<&PcRangeHandlerEntry> {
        self.lookup(pc, thrown_type, None::<&dyn Fn(&Symbol, &Symbol) -> bool>)
    }

    /// All entries covering `pc`, in dispatch order.
    pub fn covering(&self, pc: i32) -> impl Iterator<Item = &PcRangeHandlerEntry> {
        self.entries.iter().filter(move |e| e.covers(pc))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[PcRangeHandlerEntry] {
        &self.entries
    }

    /// Build a pc-range table from a `ConstMethod` exception table,
    /// translating bcis to pc offsets via `bci_to_pc`.
    pub fn from_bci_table<F>(
        entries: &[(u16, u16, u16, Option<Symbol>)],
        bci_to_pc: F,
    ) -> Self
    where
        F: Fn(u16) -> i32,
    {
        let mut t = Self::with_capacity(entries.len());
        for &(start, end, handler, ref catch) in entries {
            t.add(
                bci_to_pc(start),
                bci_to_pc(end),
                bci_to_pc(handler),
                catch.clone(),
            );
        }
        t
    }

    /// Build a pc-range table directly from `ConstMethod` exception
    /// table entries (`start_pc`/`end_pc`/`handler_pc` are bcis in the
    /// class file; `catch_type` is a constant-pool index, `0` = catch-all).
    pub fn from_const_method_table<F, G>(
        entries: &[crate::share::oops::method::ExceptionTableEntry],
        bci_to_pc: F,
        catch_type_name: G,
    ) -> Self
    where
        F: Fn(usize) -> i32,
        G: Fn(u16) -> Option<Symbol>,
    {
        let mut t = Self::with_capacity(entries.len());
        for e in entries {
            t.add(
                bci_to_pc(e.start_pc),
                bci_to_pc(e.end_pc),
                bci_to_pc(e.handler_pc),
                catch_type_name(e.catch_type),
            );
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_table_subtables() {
        let mut t = ExceptionHandlerTable::new(8);
        t.add_subtable(10, &[20, 30], Some(&[0, 1]), &[100, 200]);
        t.add_subtable(50, &[60], None, &[300]);
        assert_eq!(t.len(), 2 + 3);
        assert_eq!(t.subtable_for(10), Some(0));
        assert_eq!(t.subtable_for(50), Some(3));
        let e = t.entry_for(10, 30, 1).unwrap();
        assert_eq!(e.pco(), 200);
        assert_eq!(e.bci(), 30);
        assert_eq!(e.scope_depth(), 1);
    }

    #[test]
    fn handler_table_serialization_round_trip() {
        let mut t = ExceptionHandlerTable::new(8);
        t.add_subtable(10, &[20, 30], Some(&[0, 1]), &[100, 200]);
        let bytes = t.copy_bytes_to();
        assert_eq!(bytes.len(), t.size_in_bytes());
        let back = ExceptionHandlerTable::from_bytes(&bytes);
        assert_eq!(back.entries(), t.entries());
        let e = back.entry_for(10, 20, 0).unwrap();
        assert_eq!(e.pco(), 100);
    }

    #[test]
    fn pc_range_lookup_order_and_catch_all() {
        let mut t = PcRangeHandlerTable::new();
        let catch_all = Symbol::new(b"java/lang/Throwable");
        let io = Symbol::new(b"java/io/IOException");
        t.add(0, 50, 200, None);
        t.add(60, 70, 100, Some(io.clone()));
        // Catch-all range wins inside [0,50) for any thrown type.
        let e = t.lookup_exact(15, &catch_all).unwrap();
        assert_eq!(e.handler_pc, 200);
        // The typed range only matches its catch type.
        let e2 = t.lookup(65, &io, None).unwrap();
        assert_eq!(e2.handler_pc, 100);
        assert!(t.lookup_exact(65, &catch_all).is_none());
        // Outside all ranges.
        assert!(t.lookup_exact(55, &catch_all).is_none());
        // covering() reports every range containing the pc.
        assert_eq!(t.covering(15).count(), 1);
        assert_eq!(t.covering(65).count(), 1);
    }

    #[test]
    fn pc_range_from_bci_table() {
        let entries: [(u16, u16, u16, Option<Symbol>); 1] =
            [(0, 10, 20, Some(Symbol::new(b"E")))];
        let t = PcRangeHandlerTable::from_bci_table(&entries, |bci| bci as i32 * 4);
        assert_eq!(t.entries().len(), 1);
        assert_eq!(t.entries()[0].start_pc, 0);
        assert_eq!(t.entries()[0].end_pc, 40);
        assert_eq!(t.entries()[0].handler_pc, 80);
    }
}
