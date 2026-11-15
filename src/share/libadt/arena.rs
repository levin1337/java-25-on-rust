use std::alloc::{alloc, dealloc, handle_alloc_error, Layout};
use std::cell::UnsafeCell;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::ptr;
use std::sync::OnceLock;

use crate::share::utilities::globalDefinitions::{align_up, is_aligned, BYTES_PER_WORD};

pub const ARENA_AMALLOC_ALIGNMENT: usize = size_of::<usize>();

const CHUNK_SLACK: usize = 40;
pub const CHUNK_TINY_SIZE: usize = 256 - CHUNK_SLACK;
pub const CHUNK_INIT_SIZE: usize = 1024 - CHUNK_SLACK;
pub const CHUNK_MEDIUM_SIZE: usize = 10 * 1024 - CHUNK_SLACK;
pub const CHUNK_SIZE: usize = 32 * 1024 - CHUNK_SLACK;

const CHUNK_ALLOC_ALIGN: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tag {
    ResourceArea,
    Node,
    Compile,
    Type,
    MatcherStates,
    RegallocLive,
    RegallocSplit,
    Superword,
    Cienv,
    HandleArea,
    Other,
}

#[repr(C)]
struct ChunkHeader {
    next: *mut ChunkHeader,
    len: usize,
}

const CHUNK_OVERHEAD: usize = align_up(size_of::<ChunkHeader>(), ARENA_AMALLOC_ALIGNMENT);

impl ChunkHeader {
    unsafe fn bottom(&self) -> *mut u8 {
        unsafe { (self as *const Self as *mut u8).add(CHUNK_OVERHEAD) }
    }

    unsafe fn top(&self) -> *mut u8 {
        unsafe { self.bottom().add(self.len) }
    }

    fn alloc(len: usize) -> *mut ChunkHeader {
        let len = align_up(len, ARENA_AMALLOC_ALIGNMENT);
        let total = CHUNK_OVERHEAD + len;
        let layout = Layout::from_size_align(total, CHUNK_ALLOC_ALIGN)
            .unwrap_or_else(|_| std::process::abort());
        let p = unsafe { alloc(layout) };
        if p.is_null() {
            handle_alloc_error(layout);
        }
        unsafe {
            let hdr = p as *mut ChunkHeader;
            (*hdr).next = ptr::null_mut();
            (*hdr).len = len;
            hdr
        }
    }

    unsafe fn dealloc_chunk(hdr: *mut ChunkHeader) {
        let total = unsafe { CHUNK_OVERHEAD + (*hdr).len };
        let layout = Layout::from_size_align(total, CHUNK_ALLOC_ALIGN)
            .unwrap_or_else(|_| std::process::abort());
        unsafe { dealloc(hdr as *mut u8, layout) };
    }
}

pub struct Arena {
    first: *mut ChunkHeader,
    chunk: *mut ChunkHeader,
    hwm: *mut u8,
    max: *mut u8,
    size_in_bytes: usize,
    nesting: i32,
    tag: Tag,
}

impl Arena {
    pub fn new(tag: Tag) -> Arena {
        Self::with_init_size(tag, CHUNK_INIT_SIZE)
    }

    pub fn with_init_size(tag: Tag, init_size: usize) -> Arena {
        let init_size = align_up(init_size, ARENA_AMALLOC_ALIGNMENT);
        let chunk = ChunkHeader::alloc(init_size);
        unsafe {
            let bottom = (*chunk).bottom();
            Arena {
                first: chunk,
                chunk,
                hwm: bottom,
                max: (*chunk).top(),
                size_in_bytes: init_size,
                nesting: 0,
                tag,
            }
        }
    }

    pub fn resource() -> Arena {
        Self::new(Tag::ResourceArea)
    }

    pub fn tag(&self) -> Tag {
        self.tag
    }

    pub fn size_in_bytes(&self) -> usize {
        self.size_in_bytes
    }

    pub fn hwm(&self) -> *mut u8 {
        self.hwm
    }

    unsafe fn grow(&mut self, x: usize) -> *mut u8 {
        let len = align_up(x, ARENA_AMALLOC_ALIGNMENT).max(CHUNK_SIZE);
        let k = self.chunk;
        let new_chunk = ChunkHeader::alloc(len);
        unsafe {
            if !k.is_null() {
                (*k).next = new_chunk;
            } else {
                self.first = new_chunk;
            }
            self.chunk = new_chunk;
            self.hwm = (*new_chunk).bottom();
            self.max = (*new_chunk).top();
        }
        self.size_in_bytes += len;
        let result = self.hwm;
        self.hwm = unsafe { self.hwm.add(x) };
        result
    }

    unsafe fn internal_amalloc(&mut self, x: usize) -> *mut u8 {
        debug_assert!(is_aligned(x, BYTES_PER_WORD));
        if self.max as usize - self.hwm as usize >= x {
            let old = self.hwm;
            self.hwm = unsafe { self.hwm.add(x) };
            old
        } else {
            unsafe { self.grow(x) }
        }
    }

    pub fn amalloc(&mut self, x: usize) -> *mut u8 {
        let x = align_up(x, ARENA_AMALLOC_ALIGNMENT);
        unsafe { self.internal_amalloc(x) }
    }

    pub fn amalloc_words(&mut self, x: usize) -> *mut u8 {
        debug_assert!(is_aligned(x, BYTES_PER_WORD));
        unsafe { self.internal_amalloc(x) }
    }

    pub fn afree(&mut self, ptr: *mut u8, size: usize) -> bool {
        if ptr.is_null() {
            return true;
        }
        unsafe {
            if ptr.add(size) == self.hwm {
                self.hwm = ptr;
                true
            } else {
                false
            }
        }
    }

    pub fn arealloc(
        &mut self,
        old_ptr: *mut u8,
        old_size: usize,
        new_size: usize,
    ) -> *mut u8 {
        if new_size == 0 {
            self.afree(old_ptr, old_size);
            return ptr::null_mut();
        }
        if old_ptr.is_null() {
            debug_assert_eq!(old_size, 0);
            return self.amalloc(new_size);
        }
        let corrected_new_size = align_up(new_size, ARENA_AMALLOC_ALIGNMENT);
        unsafe {
            if old_ptr.add(old_size) == self.hwm {
                if self.max as usize - old_ptr as usize >= corrected_new_size {
                    self.hwm = old_ptr.add(corrected_new_size);
                    return old_ptr;
                }
            } else if new_size <= old_size {
                return old_ptr;
            }
        }
        let new_ptr = self.amalloc(new_size);
        unsafe {
            ptr::copy_nonoverlapping(old_ptr, new_ptr, old_size);
        }
        self.afree(old_ptr, old_size);
        new_ptr
    }

    pub fn contains(&self, ptr: *const u8) -> bool {
        unsafe {
            if self.chunk.is_null() {
                return false;
            }
            let p = ptr as *mut u8;
            if (*self.chunk).bottom() <= p && p < self.hwm {
                return true;
            }
            let mut k = self.first;
            while !k.is_null() {
                if k != self.chunk {
                    if (*k).bottom() <= p && p < (*k).top() {
                        return true;
                    }
                }
                k = (*k).next;
            }
            false
        }
    }

    pub fn used(&self) -> usize {
        unsafe {
            if self.chunk.is_null() {
                return 0;
            }
            let mut sum = (*self.chunk).len - (self.max as usize - self.hwm as usize);
            let mut k = self.first;
            while !k.is_null() && k != self.chunk {
                sum += (*k).len;
                k = (*k).next;
            }
            sum
        }
    }

    pub fn destruct_contents(&mut self) {
        self.size_in_bytes = 0;
        let mut k = self.first;
        while !k.is_null() {
            unsafe {
                let next = (*k).next;
                ChunkHeader::dealloc_chunk(k);
                k = next;
            }
        }
        self.first = ptr::null_mut();
        self.chunk = ptr::null_mut();
        self.hwm = ptr::null_mut();
        self.max = ptr::null_mut();
    }

    pub fn strdup(&mut self, s: &CStr) -> *mut c_char {
        let bytes = s.to_bytes_with_nul();
        let p = self.amalloc(bytes.len()) as *mut c_char;
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), p as *mut u8, bytes.len());
        }
        p
    }

    pub fn saved_state(&self) -> SavedState {
        SavedState {
            chunk: self.chunk,
            hwm: self.hwm,
            max: self.max,
            size_in_bytes: self.size_in_bytes,
            nesting: self.nesting,
        }
    }

    pub fn activate_state(&mut self, state: &SavedState) {
        debug_assert_eq!(self.nesting, state.nesting);
        self.nesting += 1;
    }

    pub fn deactivate_state(&mut self, state: &SavedState) {
        debug_assert!(self.nesting > state.nesting);
        debug_assert_eq!(self.nesting - state.nesting, 1);
        self.nesting -= 1;
    }

    pub fn nesting(&self) -> i32 {
        self.nesting
    }

    pub fn rollback_to(&mut self, state: &SavedState) {
        debug_assert!(self.nesting > state.nesting);
        debug_assert_eq!(self.nesting - state.nesting, 1);
        if state.chunk.is_null() {
            self.first = ptr::null_mut();
            self.chunk = ptr::null_mut();
            self.hwm = ptr::null_mut();
            self.max = ptr::null_mut();
            self.size_in_bytes = state.size_in_bytes;
            return;
        }
        unsafe {
            if !(*state.chunk).next.is_null() {
                debug_assert!(self.size_in_bytes > state.size_in_bytes);
                self.size_in_bytes = state.size_in_bytes;
                let mut k = (*state.chunk).next;
                (*state.chunk).next = ptr::null_mut();
                while !k.is_null() {
                    let next = (*k).next;
                    ChunkHeader::dealloc_chunk(k);
                    k = next;
                }
                debug_assert!(self.hwm != state.hwm);
            } else {
                debug_assert_eq!(self.size_in_bytes, state.size_in_bytes);
            }
            if self.hwm != state.hwm {
                self.chunk = state.chunk;
                self.hwm = state.hwm;
                self.max = state.max;
            } else {
                debug_assert!(self.chunk == state.chunk);
                debug_assert!(self.hwm == state.hwm);
                debug_assert!(self.max == state.max);
            }
        }
    }
}

unsafe impl Send for Arena {}

impl Drop for Arena {
    fn drop(&mut self) {
        self.destruct_contents();
    }
}

#[derive(Clone, Copy)]
pub struct SavedState {
    chunk: *mut ChunkHeader,
    hwm: *mut u8,
    max: *mut u8,
    size_in_bytes: usize,
    nesting: i32,
}

pub struct ResourceMark<'a> {
    area: *mut Arena,
    state: SavedState,
    _marker: std::marker::PhantomData<&'a mut Arena>,
}

impl<'a> ResourceMark<'a> {
    pub fn new() -> ResourceMark<'static> {
        Self::for_raw(resource_area_ptr())
    }

    pub fn for_area(area: &'a mut Arena) -> ResourceMark<'a> {
        Self::for_raw(area as *mut Arena)
    }

    fn for_raw<'b>(area: *mut Arena) -> ResourceMark<'b> {
        unsafe {
            let a = &mut *area;
            let state = a.saved_state();
            a.activate_state(&state);
            ResourceMark {
                area,
                state,
                _marker: std::marker::PhantomData,
            }
        }
    }

    pub fn reset_to_mark(&self) {
        unsafe {
            (*self.area).rollback_to(&self.state);
        }
    }
}

impl Drop for ResourceMark<'_> {
    fn drop(&mut self) {
        unsafe {
            let a = &mut *self.area;
            a.rollback_to(&self.state);
            a.deactivate_state(&self.state);
        }
    }
}

thread_local! {
    static RESOURCE_AREA: UnsafeCell<Arena> = UnsafeCell::new(Arena::resource());
}

fn resource_area_ptr() -> *mut Arena {
    RESOURCE_AREA.with(|cell| cell.get())
}

pub fn resource_area() -> &'static mut Arena {
    unsafe { &mut *resource_area_ptr() }
}

pub fn with_resource_area<R>(f: impl FnOnce(&mut Arena) -> R) -> R {
    f(unsafe { &mut *resource_area_ptr() })
}

pub fn resource_amalloc(size: usize) -> *mut u8 {
    resource_area().amalloc(size)
}

pub fn resource_arealloc(old: *mut u8, old_size: usize, new_size: usize) -> *mut u8 {
    resource_area().arealloc(old, old_size, new_size)
}

pub fn resource_afree(ptr: *mut u8, size: usize) -> bool {
    resource_area().afree(ptr, size)
}

pub struct GlobalArenas {
    pub compiler: std::sync::Mutex<Arena>,
    pub ci: std::sync::Mutex<Arena>,
    pub other: std::sync::Mutex<Arena>,
}

impl GlobalArenas {
    fn new() -> GlobalArenas {
        GlobalArenas {
            compiler: std::sync::Mutex::new(Arena::new(Tag::Compile)),
            ci: std::sync::Mutex::new(Arena::new(Tag::Cienv)),
            other: std::sync::Mutex::new(Arena::new(Tag::Other)),
        }
    }
}

pub fn global_arenas() -> &'static GlobalArenas {
    static GLOBALS: OnceLock<GlobalArenas> = OnceLock::new();
    GLOBALS.get_or_init(GlobalArenas::new)
}
