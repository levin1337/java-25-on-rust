/*
 * Port of OpenJDK 25 HotSpot ExceptionHandlerTable
 * (share/code/exceptionHandlerTable.hpp / exceptionHandlerTable.cpp)
 *
 * Implements compiled nmethod and interpreter exception handler lookup table.
 */

use crate::share::oops::symbol::Symbol;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandlerTableEntry {
    pub start_pc: usize,
    pub end_pc: usize,
    pub handler_pc: usize,
    pub catch_type: Option<Symbol>,
}

impl HandlerTableEntry {
    pub fn new(
        start_pc: usize,
        end_pc: usize,
        handler_pc: usize,
        catch_type: Option<Symbol>,
    ) -> Self {
        Self {
            start_pc,
            end_pc,
            handler_pc,
            catch_type,
        }
    }

    #[inline]
    pub fn covers(&self, pc: usize) -> bool {
        pc >= self.start_pc && pc < self.end_pc
    }

    #[inline]
    pub fn matches(&self, pc: usize, thrown_type: &Symbol) -> bool {
        if !self.covers(pc) {
            return false;
        }
        match &self.catch_type {
            None => true, // catch-all / finally block
            Some(expected) => expected == thrown_type,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ExceptionHandlerTable {
    entries: Vec<HandlerTableEntry>,
}

impl ExceptionHandlerTable {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn add_entry(
        &mut self,
        start_pc: usize,
        end_pc: usize,
        handler_pc: usize,
        catch_type: Option<Symbol>,
    ) {
        self.entries.push(HandlerTableEntry::new(
            start_pc, end_pc, handler_pc, catch_type,
        ));
    }

    pub fn entry_for(&self, pc: usize, thrown_type: &Symbol) -> Option<usize> {
        for entry in &self.entries {
            if entry.matches(pc, thrown_type) {
                return Some(entry.handler_pc);
            }
        }
        None
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[inline]
    pub fn entries(&self) -> &[HandlerTableEntry] {
        &self.entries
    }
}
