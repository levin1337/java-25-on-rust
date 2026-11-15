use super::z_address::ZAddress;
use super::z_address::Z_GRANULE_SIZE_SHIFT;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ZMarkStackEntry(u64);

impl ZMarkStackEntry {
    const FINALIZABLE: u64 = 1 << 0;
    const PARTIAL_ARRAY: u64 = 1 << 1;
    const FOLLOW: u64 = 1 << 2;
    const INC_LIVE: u64 = 1 << 3;
    const MARK: u64 = 1 << 4;
    const OBJECT_ADDRESS_SHIFT: u32 = 5;
    const OBJECT_ADDRESS_MASK: u64 = (1u64 << 59) - 1;
    const PARTIAL_ARRAY_OFFSET_SHIFT: u32 = 32;
    const PARTIAL_ARRAY_LENGTH_SHIFT: u32 = 2;
    const PARTIAL_ARRAY_LENGTH_MASK: u64 = (1u64 << 30) - 1;

    pub fn object(object_address: ZAddress, mark: bool, inc_live: bool, follow: bool, finalizable: bool) -> Self {
        let mut entry = 0u64;
        entry |= (object_address << Self::OBJECT_ADDRESS_SHIFT) & Self::OBJECT_ADDRESS_MASK;
        entry |= ((mark as u64) << 4) | ((inc_live as u64) << 3) | ((follow as u64) << 2);
        entry |= (finalizable as u64) & Self::FINALIZABLE;
        Self(entry)
    }

    pub fn partial_array(offset: usize, length: usize, finalizable: bool) -> Self {
        let mut entry = Self::PARTIAL_ARRAY | ((finalizable as u64) & Self::FINALIZABLE);
        entry |= ((length as u64) << Self::PARTIAL_ARRAY_LENGTH_SHIFT) & Self::PARTIAL_ARRAY_LENGTH_MASK;
        entry |= (offset as u64) << Self::PARTIAL_ARRAY_OFFSET_SHIFT;
        Self(entry)
    }

    pub fn finalizable(self) -> bool {
        (self.0 & Self::FINALIZABLE) != 0
    }

    pub fn partial_array(self) -> bool {
        (self.0 & Self::PARTIAL_ARRAY) != 0
    }

    pub fn follow(self) -> bool {
        (self.0 & Self::FOLLOW) != 0
    }

    pub fn inc_live(self) -> bool {
        (self.0 & Self::INC_LIVE) != 0
    }

    pub fn mark(self) -> bool {
        (self.0 & Self::MARK) != 0
    }

    pub fn object_address_raw(self) -> ZAddress {
        (self.0 >> Self::OBJECT_ADDRESS_SHIFT) & Self::OBJECT_ADDRESS_MASK
    }

    pub fn partial_array_offset(self) -> usize {
        ((self.0 >> Self::PARTIAL_ARRAY_OFFSET_SHIFT) & 0xffff_ffff) as usize
    }

    pub fn partial_array_length(self) -> usize {
        ((self.0 >> Self::PARTIAL_ARRAY_LENGTH_SHIFT) & Self::PARTIAL_ARRAY_LENGTH_MASK) as usize
    }

    pub fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone)]
pub struct ZMarkStack {
    entries: Vec<ZMarkStackEntry>,
    top: usize,
}

impl ZMarkStack {
    pub fn new(capacity: usize) -> Self {
        Self { entries: vec![ZMarkStackEntry::default(); capacity], top: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.top == 0
    }

    pub fn is_full(&self) -> bool {
        self.top == self.entries.len()
    }

    pub fn len(&self) -> usize {
        self.top
    }

    pub fn push(&mut self, value: ZMarkStackEntry) {
        assert!(!self.is_full());
        self.entries[self.top] = value;
        self.top += 1;
    }

    pub fn pop(&mut self) -> Option<ZMarkStackEntry> {
        if self.is_empty() {
            None
        } else {
            self.top -= 1;
            Some(self.entries[self.top])
        }
    }

    pub fn try_push(&mut self, value: ZMarkStackEntry) -> bool {
        if self.is_full() {
            false
        } else {
            self.push(value);
            true
        }
    }
}

#[derive(Default)]
pub struct ZMarkStackList {
    stacks: Mutex<Vec<ZMarkStack>>,
}

impl ZMarkStackList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.stacks.lock().unwrap().is_empty()
    }

    pub fn length(&self) -> usize {
        self.stacks.lock().unwrap().len()
    }

    pub fn push(&self, stack: ZMarkStack) {
        self.stacks.lock().unwrap().push(stack);
    }

    pub fn pop(&self) -> Option<ZMarkStack> {
        self.stacks.lock().unwrap().pop()
    }
}

pub struct ZMarkStripe {
    published: ZMarkStackList,
    overflowed: ZMarkStackList,
}

impl ZMarkStripe {
    pub fn new() -> Self {
        Self { published: ZMarkStackList::new(), overflowed: ZMarkStackList::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.published.is_empty() && self.overflowed.is_empty()
    }

    pub fn population(&self) -> usize {
        self.published.length() + self.overflowed.length()
    }

    pub fn publish_stack(&self, stack: ZMarkStack, publish: bool) {
        if publish {
            self.published.push(stack);
        } else {
            self.overflowed.push(stack);
        }
    }

    pub fn steal_stack(&self) -> Option<ZMarkStack> {
        self.published.pop().or_else(|| self.overflowed.pop())
    }
}

pub struct ZMarkStripeSet {
    stripes: Vec<ZMarkStripe>,
    nstripes_mask: usize,
}

impl ZMarkStripeSet {
    pub fn new(nstripes: usize) -> Self {
        let nstripes = nstripes.clamp(1, super::z_address::Z_MARK_STRIPES_MAX).next_power_of_two();
        Self {
            stripes: (0..nstripes).map(|_| ZMarkStripe::new()).collect(),
            nstripes_mask: nstripes - 1,
        }
    }

    pub fn nstripes(&self) -> usize {
        self.stripes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stripes.iter().all(ZMarkStripe::is_empty)
    }

    pub fn is_crowded(&self) -> bool {
        self.stripes.iter().map(ZMarkStripe::population).sum::<usize>() > self.nstripes() * 2
    }

    pub fn stripe_at(&self, index: usize) -> &ZMarkStripe {
        &self.stripes[index & self.nstripes_mask]
    }

    pub fn stripe_for_worker(&self, worker_id: usize) -> &ZMarkStripe {
        self.stripe_at(worker_id)
    }

    pub fn stripe_for_addr(&self, address: ZAddress) -> &ZMarkStripe {
        self.stripe_at((address >> Z_GRANULE_SIZE_SHIFT) as usize)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ZMarkStripe> {
        self.stripes.iter()
    }
}

pub struct ZMarkThreadLocalStacks {
    stacks: Vec<Option<ZMarkStack>>,
}

impl ZMarkThreadLocalStacks {
    pub fn new(nstripes: usize) -> Self {
        Self { stacks: (0..nstripes).map(|_| None).collect() }
    }

    pub fn is_empty(&self, stripes: &ZMarkStripeSet) -> bool {
        self.stacks.iter().all(Option::is_none) && stripes.is_empty()
    }

    pub fn install(&mut self, stripe_index: usize, stack: ZMarkStack) {
        self.stacks[stripe_index] = Some(stack);
    }

    pub fn steal(&mut self, stripes: &ZMarkStripeSet, stripe_index: usize) -> Option<ZMarkStack> {
        stripes.stripe_at(stripe_index).steal_stack().or_else(|| self.stacks[stripe_index].take())
    }

    pub fn push(&mut self, stripe_index: usize, value: ZMarkStackEntry, publish: bool, stripes: &ZMarkStripeSet) {
        let stack = self.stacks[stripe_index].get_or_insert_with(|| ZMarkStack::new(1024));
        if stack.is_full() {
            let old = std::mem::replace(stack, ZMarkStack::new(1024));
            stripes.stripe_at(stripe_index).publish_stack(old, publish);
            stack.push(value);
        } else {
            stack.push(value);
        }
    }

    pub fn pop(&mut self, stripe_index: usize) -> Option<ZMarkStackEntry> {
        self.stacks[stripe_index].as_mut().and_then(ZMarkStack::pop)
    }

    pub fn flush(&mut self, stripes: &ZMarkStripeSet) {
        for (index, stack) in self.stacks.iter_mut().enumerate() {
            if let Some(stack) = stack.take() {
                stripes.stripe_at(index).publish_stack(stack, true);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ZMarkCacheEntry {
    pub page: usize,
    pub objects: u32,
    pub bytes: usize,
}

pub struct ZMarkCache {
    shift: u32,
    cache: Vec<ZMarkCacheEntry>,
}

impl ZMarkCache {
    pub fn new(nstripes: usize) -> Self {
        let size = super::z_address::Z_MARK_CACHE_SIZE;
        Self {
            shift: (nstripes.max(1).next_power_of_two() as u32).trailing_zeros(),
            cache: vec![ZMarkCacheEntry::default(); size],
        }
    }

    pub fn set_nstripes(&mut self, nstripes: usize) {
        self.shift = (nstripes.max(1).next_power_of_two() as u32).trailing_zeros();
    }

    pub fn inc_live(&mut self, page: usize, bytes: usize) {
        let index = (page >> self.shift as usize) & (self.cache.len() - 1);
        let entry = &mut self.cache[index];
        if entry.page == page || entry.objects == 0 {
            entry.page = page;
            entry.objects += 1;
            entry.bytes += bytes;
        } else {
            entry.objects = entry.objects.saturating_sub(1);
            entry.bytes = entry.bytes.saturating_sub(bytes.min(entry.bytes));
            if entry.objects == 0 {
                entry.page = page;
                entry.objects = 1;
                entry.bytes = bytes;
            }
        }
    }
}

pub trait ZMarkHooks {
    fn mark_object(&mut self, address: ZAddress, finalizable: bool) -> bool;
    fn follow_object(&mut self, address: ZAddress, finalizable: bool) -> Vec<ZMarkStackEntry>;
    fn follow_partial_array(&mut self, offset: usize, length: usize, finalizable: bool) -> Vec<ZMarkStackEntry>;
}

pub struct ZMark {
    stripes: ZMarkStripeSet,
    stacks: ZMarkThreadLocalStacks,
    cache: ZMarkCache,
    roots: Vec<ZMarkStackEntry>,
    nproactiveflush: usize,
    nterminateflush: usize,
    ntrycomplete: usize,
    ncontinue: usize,
    active: bool,
}

impl ZMark {
    pub fn new(nstripes: usize) -> Self {
        Self {
            stripes: ZMarkStripeSet::new(nstripes),
            stacks: ZMarkThreadLocalStacks::new(nstripes),
            cache: ZMarkCache::new(nstripes),
            roots: Vec::new(),
            nproactiveflush: 0,
            nterminateflush: 0,
            ntrycomplete: 0,
            ncontinue: 0,
            active: false,
        }
    }

    pub fn start(&mut self) {
        self.active = true;
        self.roots.clear();
        self.stacks = ZMarkThreadLocalStacks::new(self.stripes.nstripes());
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn mark_object(&mut self, address: ZAddress, finalizable: bool) {
        assert!(self.active);
        self.roots.push(ZMarkStackEntry::object(address, true, true, true, finalizable));
    }

    pub fn mark_young(&mut self, address: ZAddress) {
        self.mark_object(address, false);
    }

    pub fn mark_finalizable(&mut self, address: ZAddress) {
        self.mark_object(address, true);
    }

    pub fn push_partial_array(&mut self, offset: usize, length: usize, finalizable: bool) {
        self.roots.push(ZMarkStackEntry::partial_array(offset, length, finalizable));
    }

    pub fn mark_young_roots<I>(&mut self, roots: I)
    where
        I: IntoIterator<Item = (ZAddress, bool)>,
    {
        for (address, finalizable) in roots {
            self.mark_object(address, finalizable);
        }
    }

    pub fn mark_old_roots<I>(&mut self, roots: I)
    where
        I: IntoIterator<Item = (ZAddress, bool)>,
    {
        self.mark_young_roots(roots);
    }

    pub fn prepare_work(&mut self) {
        for (index, entry) in self.roots.drain(..).enumerate() {
            self.stacks.push(index % self.stripes.nstripes(), entry, true, &self.stripes);
        }
    }

    pub fn follow_work<H: ZMarkHooks>(&mut self, hooks: &mut H, worker_id: usize) -> bool {
        let stripe = worker_id % self.stripes.nstripes();
        let mut did_work = false;
        while let Some(entry) = self.stacks.pop(stripe) {
            did_work = true;
            let children = if entry.partial_array() {
                hooks.follow_partial_array(entry.partial_array_offset(), entry.partial_array_length(), entry.finalizable())
            } else {
                let address = entry.object_address_raw();
                if entry.mark() {
                    hooks.mark_object(address, entry.finalizable());
                }
                if entry.follow() {
                    hooks.follow_object(address, entry.finalizable())
                } else {
                    Vec::new()
                }
            };
            for child in children {
                self.stacks.push(stripe, child, true, &self.stripes);
            }
        }
        if self.stripes.is_empty() {
            self.ntrycomplete += 1;
        } else {
            self.ncontinue += 1;
        }
        did_work
    }

    pub fn try_steal(&mut self, worker_id: usize) -> Option<ZMarkStackEntry> {
        let stripe = worker_id % self.stripes.nstripes();
        let stack = self.stacks.steal(&self.stripes, stripe)?;
        let entry = stack.entries_last();
        if entry.is_some() {
            self.stacks.install(stripe, stack);
        }
        entry
    }

    pub fn flush(&mut self) {
        self.stacks.flush(&self.stripes);
        self.nterminateflush += 1;
    }

    pub fn try_proactive_flush(&mut self) -> bool {
        self.nproactiveflush += 1;
        self.flush();
        true
    }

    pub fn end(&mut self) -> bool {
        self.flush();
        self.active = false;
        self.stripes.is_empty() && self.roots.is_empty()
    }

    pub fn free(&mut self) {
        self.active = false;
        self.roots.clear();
        self.stacks = ZMarkThreadLocalStacks::new(self.stripes.nstripes());
    }

    pub fn statistics(&self) -> (usize, usize, usize, usize) {
        (self.nproactiveflush, self.nterminateflush, self.ntrycomplete, self.ncontinue)
    }

    pub fn stripes(&self) -> &ZMarkStripeSet {
        &self.stripes
    }
}

impl ZMarkStack {
    fn entries_last(&self) -> Option<ZMarkStackEntry> {
        if self.is_empty() { None } else { Some(self.entries[self.top - 1]) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Hooks {
        marked: Vec<ZAddress>,
        followed: Vec<ZAddress>,
    }

    impl ZMarkHooks for Hooks {
        fn mark_object(&mut self, address: ZAddress, _finalizable: bool) -> bool {
            self.marked.push(address);
            true
        }
        fn follow_object(&mut self, address: ZAddress, _finalizable: bool) -> Vec<ZMarkStackEntry> {
            self.followed.push(address);
            Vec::new()
        }
        fn follow_partial_array(&mut self, _offset: usize, _length: usize, _finalizable: bool) -> Vec<ZMarkStackEntry> {
            Vec::new()
        }
    }

    #[test]
    fn mark_stack_entry_layout() {
        let entry = ZMarkStackEntry::object(0x1234, true, true, true, true);
        assert!(entry.mark());
        assert!(entry.follow());
        assert!(entry.inc_live());
        assert!(entry.finalizable());
        assert_eq!(entry.object_address_raw(), 0x1234);
    }

    #[test]
    fn mark_drains_roots() {
        let mut mark = ZMark::new(1);
        mark.start();
        mark.mark_object(0x100, false);
        mark.prepare_work();
        let mut hooks = Hooks { marked: Vec::new(), followed: Vec::new() };
        assert!(mark.follow_work(&mut hooks, 0));
        assert_eq!(hooks.marked, vec![0x100]);
        assert_eq!(hooks.followed, vec![0x100]);
        assert!(mark.end());
    }
}
