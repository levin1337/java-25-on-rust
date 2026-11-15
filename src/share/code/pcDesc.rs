/*
 * Port of OpenJDK 25 HotSpot PcDesc (share/code/pcDesc.hpp / pcDesc.cpp)
 *
 * PcDescs map a native PC offset in an nmethod to the corresponding
 * bytecode index (BCI), scope information, and deoptimization state.
 */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PcDesc {
    pc_offset: i32,
    scope_decode_offset: i32,
    obj_decode_offset: i32,
    flags: i32,
    /// Bytecode index this pc maps to for deoptimization.  HotSpot keeps
    /// the bci inside the ScopeDesc chain; the C1 backend currently has
    /// no scope-data blob, so the broker normalizes the bci it finds in
    /// `scope_decode_offset` into this field at install time.
    bci: i32,
}

impl PcDesc {
    /// Sentinel for "no valid bci" (MethodCompilation::INVALID_FRAME_STATE_BCI).
    pub const UNKNOWN_BCI: i32 = -6;
    pub const PCDESC_REEXECUTE: i32 = 1 << 0;
    pub const PCDESC_IS_METHOD_HANDLE_INVOKE: i32 = 1 << 1;
    pub const PCDESC_RETURN_OOP: i32 = 1 << 2;
    pub const PCDESC_RETHROW_EXCEPTION: i32 = 1 << 3;
    pub const PCDESC_HAS_EA_LOCAL_IN_SCOPE: i32 = 1 << 4;
    pub const PCDESC_ARG_ESCAPE: i32 = 1 << 5;

    pub const LOWER_OFFSET_LIMIT: i32 = -1;
    pub const UPPER_OFFSET_LIMIT: i32 = (u32::MAX >> 1) as i32;

    pub fn new(pc_offset: i32, scope_decode_offset: i32, flags: i32) -> Self {
        Self {
            pc_offset,
            scope_decode_offset,
            obj_decode_offset: 0,
            flags,
            bci: Self::UNKNOWN_BCI,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_obj(
        pc_offset: i32,
        scope_decode_offset: i32,
        obj_decode_offset: i32,
        flags: i32,
    ) -> Self {
        Self {
            pc_offset,
            scope_decode_offset,
            obj_decode_offset,
            flags,
            bci: Self::UNKNOWN_BCI,
        }
    }

    /// Full constructor including the deopt bci.
    pub fn with_bci(
        pc_offset: i32,
        scope_decode_offset: i32,
        obj_decode_offset: i32,
        flags: i32,
        bci: i32,
    ) -> Self {
        Self {
            pc_offset,
            scope_decode_offset,
            obj_decode_offset,
            flags,
            bci,
        }
    }

    #[inline]
    pub fn pc_offset(&self) -> i32 {
        self.pc_offset
    }

    #[inline]
    pub fn set_pc_offset(&mut self, offset: i32) {
        self.pc_offset = offset;
    }

    #[inline]
    pub fn scope_decode_offset(&self) -> i32 {
        self.scope_decode_offset
    }

    #[inline]
    pub fn set_scope_decode_offset(&mut self, offset: i32) {
        self.scope_decode_offset = offset;
    }

    #[inline]
    pub fn obj_decode_offset(&self) -> i32 {
        self.obj_decode_offset
    }

    #[inline]
    pub fn set_obj_decode_offset(&mut self, offset: i32) {
        self.obj_decode_offset = offset;
    }

    #[inline]
    pub fn flags(&self) -> i32 {
        self.flags
    }

    #[inline]
    pub fn bci(&self) -> i32 {
        self.bci
    }

    #[inline]
    pub fn set_bci(&mut self, bci: i32) {
        self.bci = bci;
    }

    #[inline]
    pub fn set_flags(&mut self, flags: i32) {
        self.flags = flags;
    }

    #[inline]
    fn set_flag(&mut self, mask: i32, val: bool) {
        if val {
            self.flags |= mask;
        } else {
            self.flags &= !mask;
        }
    }

    #[inline]
    pub fn should_reexecute(&self) -> bool {
        (self.flags & Self::PCDESC_REEXECUTE) != 0
    }

    #[inline]
    pub fn set_should_reexecute(&mut self, val: bool) {
        self.set_flag(Self::PCDESC_REEXECUTE, val);
    }

    #[inline]
    pub fn is_method_handle_invoke(&self) -> bool {
        (self.flags & Self::PCDESC_IS_METHOD_HANDLE_INVOKE) != 0
    }

    #[inline]
    pub fn set_is_method_handle_invoke(&mut self, val: bool) {
        self.set_flag(Self::PCDESC_IS_METHOD_HANDLE_INVOKE, val);
    }

    #[inline]
    pub fn is_return_oop(&self) -> bool {
        (self.flags & Self::PCDESC_RETURN_OOP) != 0
    }

    #[inline]
    pub fn set_return_oop(&mut self, val: bool) {
        self.set_flag(Self::PCDESC_RETURN_OOP, val);
    }

    #[inline]
    pub fn should_rethrow_exception(&self) -> bool {
        (self.flags & Self::PCDESC_RETHROW_EXCEPTION) != 0
    }

    #[inline]
    pub fn rethrow_exception(&self) -> bool {
        self.should_rethrow_exception()
    }

    #[inline]
    pub fn set_rethrow_exception(&mut self, val: bool) {
        self.set_flag(Self::PCDESC_RETHROW_EXCEPTION, val);
    }

    #[inline]
    pub fn has_ea_local_in_scope(&self) -> bool {
        (self.flags & Self::PCDESC_HAS_EA_LOCAL_IN_SCOPE) != 0
    }

    #[inline]
    pub fn set_has_ea_local_in_scope(&mut self, val: bool) {
        self.set_flag(Self::PCDESC_HAS_EA_LOCAL_IN_SCOPE, val);
    }

    #[inline]
    pub fn arg_escape(&self) -> bool {
        (self.flags & Self::PCDESC_ARG_ESCAPE) != 0
    }

    #[inline]
    pub fn set_arg_escape(&mut self, val: bool) {
        self.set_flag(Self::PCDESC_ARG_ESCAPE, val);
    }

    #[inline]
    pub fn real_pc(&self, code_begin: usize) -> usize {
        code_begin.saturating_add_signed(self.pc_offset as isize)
    }

    #[inline]
    pub fn is_same_info(&self, other: &PcDesc) -> bool {
        self.scope_decode_offset == other.scope_decode_offset
            && self.obj_decode_offset == other.obj_decode_offset
            && self.flags == other.flags
    }
}

/// LRU cache of recently found PcDescs (PcDescCache analogue, cache_size = 4).
#[derive(Clone)]
pub struct PcDescCache {
    entries: Vec<Option<PcDesc>>,
}

impl PcDescCache {
    pub const CACHE_SIZE: usize = 4;

    pub fn new() -> Self {
        Self {
            entries: vec![None; Self::CACHE_SIZE],
        }
    }

    pub fn init_to(&mut self, initial: PcDesc) {
        self.entries.fill(Some(initial));
    }

    fn match_desc(pc: &PcDesc, pc_offset: i32, approximate: bool) -> bool {
        if !approximate {
            pc.pc_offset() == pc_offset
        } else {
            // Do not look before the sentinel (caller guarantees
            // pc_offset > PcDesc::lower_offset_limit).
            pc_offset <= pc.pc_offset()
        }
    }

    pub fn find_pc_desc(&self, pc_offset: i32, approximate: bool) -> Option<PcDesc> {
        // Step one: most recently added value.
        if let Some(res) = self.entries[0] {
            if approximate && Self::match_desc(&res, pc_offset, approximate) {
                return Some(res);
            }
        }
        // Step two: the rest of the LRU cache.
        for entry in self.entries.iter().skip(1) {
            match entry {
                Some(res) => {
                    if Self::match_desc(res, pc_offset, approximate) {
                        return Some(*res);
                    }
                }
                None => break,
            }
        }
        None
    }

    pub fn add_pc_desc(&mut self, mut pc_desc: PcDesc) {
        // Shift the cache, pushing pc_desc to the front.
        for i in 0..Self::CACHE_SIZE {
            let next = self.entries[i];
            self.entries[i] = Some(pc_desc);
            if let Some(n) = next {
                pc_desc = n;
            }
        }
    }

    pub fn last_pc_desc(&self) -> Option<PcDesc> {
        self.entries[0]
    }
}

/// Container over a sorted PcDesc array with a leading sentinel
/// (PcDescContainer analogue).  `descs` must be sorted by pc_offset and
/// contain the sentinel (pc_offset == PcDesc::lower_offset_limit) at
/// index 0 and the upper sentinel at the end.
pub struct PcDescContainer {
    cache: PcDescCache,
}

impl PcDescContainer {
    pub fn new(descs: &[PcDesc]) -> Self {
        let mut c = Self {
            cache: PcDescCache::new(),
        };
        if let Some(first) = descs.first() {
            c.cache.init_to(*first);
        }
        c
    }

    /// Exact-match lookup: returns the index into `descs`.
    pub fn find_pc_desc(
        &mut self,
        descs: &[PcDesc],
        pc_offset: i32,
        approximate: bool,
    ) -> Option<usize> {
        if descs.is_empty() {
            return None;
        }
        if pc_offset < PcDesc::LOWER_OFFSET_LIMIT
            || pc_offset as u64 > PcDesc::UPPER_OFFSET_LIMIT as u64
        {
            return None;
        }

        if let Some(cached) = self.cache.find_pc_desc(pc_offset, approximate) {
            if let Some(idx) = descs.iter().position(|d| {
                d.pc_offset() == cached.pc_offset()
                    && d.scope_decode_offset() == cached.scope_decode_offset()
                    && d.obj_decode_offset() == cached.obj_decode_offset()
            }) {
                return Some(idx);
            }
        }

        self.linear_or_quasi_search(descs, pc_offset, approximate)
    }

    /// Quasi-linear search ported from
    /// PcDescContainer::find_pc_desc_internal: giant steps of
    /// 4096 -> 256 -> 16 -> 1 then a linear pass (~16 elements).
    fn quasi_search(&mut self, descs: &[PcDesc], pc_offset: i32, approximate: bool) -> Option<usize> {
        if descs.len() < 2 {
            return None;
        }
        // lower = index 0 (sentinel, pc_offset < target), upper = len-1
        // (final sentinel, pc_offset >= target).
        let mut lower = 0usize;
        let mut upper = descs.len() - 1;
        if lower >= upper {
            return None;
        }

        // Use the last successful return as a split point.
        if let Some(mid_desc) = self.cache.last_pc_desc() {
            if let Some(mid) = descs.iter().position(|d| {
                d.pc_offset() == mid_desc.pc_offset()
                    && d.scope_decode_offset() == mid_desc.scope_decode_offset()
            }) {
                if mid_desc.pc_offset() < pc_offset {
                    lower = mid;
                } else {
                    upper = mid;
                }
            }
        }

        // Giant steps: 4096, 256, 16, 1 (LOG2_RADIX = 4).
        let mut step = 1usize << (4 * 3);
        while step > 1 {
            while lower + step < upper {
                let mid = lower + step;
                if descs[mid].pc_offset() < pc_offset {
                    lower = mid;
                } else {
                    upper = mid;
                    break;
                }
            }
            step >>= 4;
        }

        // Sneak up linearly.
        loop {
            let mid = lower + 1;
            if mid >= upper {
                break;
            }
            if descs[mid].pc_offset() < pc_offset {
                lower = mid;
            } else {
                upper = mid;
                break;
            }
        }

        // `upper` is now the first desc with pc_offset >= target.
        let cand = upper;
        if cand >= descs.len() {
            return None;
        }
        let d = &descs[cand];
        let ok = if approximate {
            // approximate: pc_offset <= d.pc_offset() && previous < pc_offset
            d.pc_offset() >= pc_offset
                && cand > 0
                && descs[cand - 1].pc_offset() < pc_offset
        } else {
            d.pc_offset() == pc_offset
        };
        if ok {
            self.cache.add_pc_desc(*d);
            Some(cand)
        } else {
            None
        }
    }

    fn linear_or_quasi_search(
        &mut self,
        descs: &[PcDesc],
        pc_offset: i32,
        approximate: bool,
    ) -> Option<usize> {
        // For small arrays a plain binary search is equivalent and cheap.
        if descs.len() <= 32 {
            let idx = descs.partition_point(|d| d.pc_offset() < pc_offset);
            if idx >= descs.len() {
                return None;
            }
            let d = &descs[idx];
            let ok = if approximate {
                d.pc_offset() >= pc_offset
                    && idx > 0
                    && descs[idx - 1].pc_offset() < pc_offset
            } else {
                d.pc_offset() == pc_offset
            };
            if ok {
                return Some(idx);
            }
            return None;
        }
        self.quasi_search(descs, pc_offset, approximate)
    }
}

impl Default for PcDescCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_descs() -> Vec<PcDesc> {
        let mut v = vec![PcDesc::new(PcDesc::LOWER_OFFSET_LIMIT, 0, 0)];
        for (i, off) in [4i32, 16, 40, 128, 512].iter().enumerate() {
            v.push(PcDesc::with_bci(
                *off,
                0,
                0,
                PcDesc::PCDESC_REEXECUTE,
                i as i32 * 3,
            ));
        }
        v.push(PcDesc::new(PcDesc::UPPER_OFFSET_LIMIT, 0, 0));
        v
    }

    #[test]
    fn exact_lookup_finds_desc() {
        let descs = sample_descs();
        let mut c = PcDescContainer::new(&descs);
        let idx = c.find_pc_desc(&descs, 40, false).unwrap();
        assert_eq!(descs[idx].pc_offset(), 40);
        assert_eq!(descs[idx].bci(), 6);
        assert!(c.find_pc_desc(&descs, 41, false).is_none());
    }

    #[test]
    fn approximate_lookup_returns_covering_desc() {
        let descs = sample_descs();
        let mut c = PcDescContainer::new(&descs);
        // Approximate mode returns the first desc with pc_offset >= target
        // (HotSpot quasi-search "upper" semantics).
        let idx = c.find_pc_desc(&descs, 100, true).unwrap();
        assert_eq!(descs[idx].pc_offset(), 128);
        let idx = c.find_pc_desc(&descs, 16, true).unwrap();
        assert_eq!(descs[idx].pc_offset(), 16);
        let idx = c.find_pc_desc(&descs, 4, true).unwrap();
        assert_eq!(descs[idx].pc_offset(), 4);
    }

    #[test]
    fn flags_round_trip() {
        let mut d = PcDesc::new(8, 2, PcDesc::PCDESC_REEXECUTE | PcDesc::PCDESC_RETURN_OOP);
        assert!(d.should_reexecute());
        assert!(d.is_return_oop());
        assert!(!d.rethrow_exception());
        d.set_rethrow_exception(true);
        d.set_arg_escape(true);
        assert!(d.rethrow_exception());
        assert!(d.arg_escape());
        d.set_flags(0);
        assert!(!d.should_reexecute());
        assert!(!d.arg_escape());
        assert_eq!(d.bci(), PcDesc::UNKNOWN_BCI);
        d.set_bci(7);
        assert_eq!(d.bci(), 7);
    }

    #[test]
    fn cache_lru_ordering() {
        let mut cache = PcDescCache::new();
        let a = PcDesc::new(1, 0, 0);
        let b = PcDesc::new(2, 0, 0);
        cache.add_pc_desc(a);
        cache.add_pc_desc(b);
        assert_eq!(cache.last_pc_desc().unwrap().pc_offset(), 2);
        assert_eq!(
            cache.find_pc_desc(1, false).map(|d| d.pc_offset()),
            Some(1)
        );
        assert_eq!(cache.find_pc_desc(9, false), None);
        assert_eq!(cache.find_pc_desc(9, true), None);
    }
}

