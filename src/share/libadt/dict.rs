use std::ffi::{c_void, CStr};
use std::marker::PhantomData;
use std::ptr;

use crate::share::libadt::arena::{resource_area, Arena};

const MAXID: usize = 20;
const SHFT: [u32; MAXID] = [1, 2, 3, 4, 5, 6, 7, 1, 2, 3, 4, 5, 6, 7, 1, 2, 3, 4, 5, 6];
const XSUM: [i32; MAXID] = [
    3, 8, 17, 34, 67, 132, 261, 264, 269, 278, 295, 328, 393, 522, 525, 530, 539, 556, 589,
    654,
];

pub type CmpKey = fn(*const c_void, *const c_void) -> i32;
pub type HashFn = fn(*const c_void) -> i32;

#[repr(C)]
pub(crate) struct Bucket {
    cnt: u32,
    max: u32,
    keyvals: *mut *mut c_void,
}

impl Bucket {
    unsafe fn keyvals(&self, j: usize) -> *mut *mut c_void {
        unsafe { self.keyvals.add(j * 2) }
    }
}

pub struct Dict {
    arena: *mut Arena,
    pub(crate) bin: *mut Bucket,
    size: usize,
    cnt: u32,
    hash: HashFn,
    cmp: CmpKey,
}

impl Dict {
    pub fn new(cmp: CmpKey, hash: HashFn) -> Dict {
        Self::with_arena(cmp, hash, resource_area(), 16)
    }

    pub fn with_arena(cmp: CmpKey, hash: HashFn, arena: &mut Arena, size: usize) -> Dict {
        let size = size.max(1).next_power_of_two().max(16);
        unsafe {
            let bin = arena
                .amalloc_words(size_of::<Bucket>() * size) as *mut Bucket;
            ptr::write_bytes(bin, 0, size);
            Dict {
                arena: arena as *mut Arena,
                bin,
                size,
                cnt: 0,
                hash,
                cmp,
            }
        }
    }

    pub fn deep_copy(&self, arena: &mut Arena) -> Dict {
        unsafe {
            let bin = arena.amalloc_words(size_of::<Bucket>() * self.size) as *mut Bucket;
            ptr::copy_nonoverlapping(self.bin, bin, self.size);
            for i in 0..self.size {
                let b = &*bin.add(i);
                if b.keyvals.is_null() {
                    continue;
                }
                let new_kv = arena
                    .amalloc_words(size_of::<*mut c_void>() * b.max as usize * 2)
                    as *mut *mut c_void;
                ptr::copy_nonoverlapping(
                    b.keyvals,
                    new_kv,
                    b.cnt as usize * 2,
                );
                (*bin.add(i)).keyvals = new_kv;
            }
            Dict {
                arena: arena as *mut Arena,
                bin,
                size: self.size,
                cnt: self.cnt,
                hash: self.hash,
                cmp: self.cmp,
            }
        }
    }

    pub fn size(&self) -> u32 {
        self.cnt
    }

    pub fn is_empty(&self) -> bool {
        self.cnt == 0
    }

    fn doubhash(&mut self) {
        let oldsize = self.size;
        self.size <<= 1;
        unsafe {
            let arena = &mut *self.arena;
            let old_bytes = size_of::<Bucket>() * oldsize;
            let new_bytes = size_of::<Bucket>() * self.size;
            self.bin = arena.arealloc(self.bin as *mut u8, old_bytes, new_bytes) as *mut Bucket;
            ptr::write_bytes(self.bin.add(oldsize), 0, oldsize);
        }
        for i in 0..oldsize {
            unsafe {
                let b = self.bin.add(i);
                if (*b).keyvals.is_null() {
                    continue;
                }
                let nb = self.bin.add(i + oldsize);
                let mut j = (*b).max;
                while j > (*b).cnt {
                    j >>= 1;
                }
                if j == 0 {
                    j = 1;
                }
                (*nb).max = j << 1;
                (*nb).keyvals = (*self.arena)
                    .amalloc_words(size_of::<*mut c_void>() * (*nb).max as usize * 2)
                    as *mut *mut c_void;
                let mut nbcnt = 0u32;
                let mut j = 0u32;
                while j < (*b).cnt {
                    let key = *(*b).keyvals(j as usize);
                    if ((self.hash)(key as *const c_void) as usize & (self.size - 1)) != i {
                        *(*nb).keyvals(nbcnt as usize) = key;
                        *(*nb).keyvals(nbcnt as usize).add(1) =
                            *(*b).keyvals(j as usize).add(1);
                        nbcnt += 1;
                        (*nb).cnt = nbcnt;
                        (*b).cnt -= 1;
                        let last = (*b).cnt;
                        *(*b).keyvals(j as usize) = *(*b).keyvals(last as usize);
                        *(*b).keyvals(j as usize).add(1) =
                            *(*b).keyvals(last as usize).add(1);
                    } else {
                        j += 1;
                    }
                }
            }
        }
    }

    pub fn insert(
        &mut self,
        key: *mut c_void,
        val: *mut c_void,
        replace: bool,
    ) -> *mut c_void {
        let hash = (self.hash)(key as *const c_void) as usize;
        let mut i = hash & (self.size - 1);
        unsafe {
            let mut b = self.bin.add(i);
            for j in 0..(*b).cnt {
                if (self.cmp)(key, *(*b).keyvals(j as usize)) == 0 {
                    if !replace {
                        return *(*b).keyvals(j as usize).add(1);
                    } else {
                        let prior = *(*b).keyvals(j as usize).add(1);
                        *(*b).keyvals(j as usize) = key;
                        *(*b).keyvals(j as usize).add(1) = val;
                        return prior;
                    }
                }
            }
            self.cnt += 1;
            if self.cnt as usize > self.size {
                self.doubhash();
                i = hash & (self.size - 1);
                b = self.bin.add(i);
            }
            if (*b).cnt == (*b).max {
                if (*b).keyvals.is_null() {
                    (*b).max = 2;
                    (*b).keyvals = (*self.arena)
                        .amalloc_words(size_of::<*mut c_void>() * (*b).max as usize * 2)
                        as *mut *mut c_void;
                } else {
                    let old_bytes = size_of::<*mut c_void>() * (*b).max as usize * 2;
                    let new_bytes = size_of::<*mut c_void>() * (*b).max as usize * 4;
                    (*b).keyvals = (*self.arena)
                        .arealloc((*b).keyvals as *mut u8, old_bytes, new_bytes)
                        as *mut *mut c_void;
                    (*b).max <<= 1;
                }
            }
            let cnt = (*b).cnt;
            *(*b).keyvals(cnt as usize) = key;
            *(*b).keyvals(cnt as usize).add(1) = val;
            (*b).cnt = cnt + 1;
            ptr::null_mut()
        }
    }

    pub fn delete(&mut self, key: *const c_void) -> *mut c_void {
        unsafe {
            let i = ((self.hash)(key) as usize) & (self.size - 1);
            let b = self.bin.add(i);
            for j in 0..(*b).cnt {
                if (self.cmp)(key, *(*b).keyvals(j as usize)) == 0 {
                    let prior = *(*b).keyvals(j as usize).add(1);
                    (*b).cnt -= 1;
                    let last = (*b).cnt;
                    *(*b).keyvals(j as usize) = *(*b).keyvals(last as usize);
                    *(*b).keyvals(j as usize).add(1) =
                        *(*b).keyvals(last as usize).add(1);
                    self.cnt -= 1;
                    return prior;
                }
            }
            ptr::null_mut()
        }
    }

    pub fn get(&self, key: *const c_void) -> *mut c_void {
        unsafe {
            let i = ((self.hash)(key) as usize) & (self.size - 1);
            let b = self.bin.add(i);
            for j in 0..(*b).cnt {
                if (self.cmp)(key, *(*b).keyvals(j as usize)) == 0 {
                    return *(*b).keyvals(j as usize).add(1);
                }
            }
            ptr::null_mut()
        }
    }

    pub fn iter(&self) -> DictI<'_> {
        DictI::new(self)
    }

    pub fn print(&self) {
        let mut out = format!(
            "Dict@{:x}[{}] = {{",
            self as *const Dict as usize,
            self.cnt
        );
        let mut it = self.iter();
        while it.test() {
            out.push_str(&format!(
                "({:x},{:x}),",
                it.key as usize, it.value as usize
            ));
            it.next();
        }
        out.push_str("}");
        println!("{}", out);
    }
}

pub struct DictI<'a> {
    dict: *const Dict,
    i: usize,
    j: usize,
    pub key: *mut c_void,
    pub value: *mut c_void,
    _marker: PhantomData<&'a Dict>,
}

impl<'a> DictI<'a> {
    fn new(dict: &'a Dict) -> DictI<'a> {
        let mut it = DictI {
            dict: dict as *const Dict,
            i: usize::MAX,
            j: 0,
            key: ptr::null_mut(),
            value: ptr::null_mut(),
            _marker: PhantomData,
        };
        it.next();
        it
    }

    pub fn reset(&mut self, dict: &'a Dict) {
        self.dict = dict as *const Dict;
        self.i = usize::MAX;
        self.j = 0;
        self.next();
    }

    pub fn test(&self) -> bool {
        unsafe { self.i < (*self.dict).size }
    }

    pub fn next(&mut self) {
        unsafe {
            let d = &*self.dict;
            if self.j != 0 {
                self.j -= 1;
                self.read_current(d);
                return;
            }
            loop {
                self.i = self.i.wrapping_add(1);
                if self.i >= d.size {
                    self.key = ptr::null_mut();
                    self.value = ptr::null_mut();
                    return;
                }
                let cnt = (*d.bin.add(self.i)).cnt;
                if cnt == 0 {
                    continue;
                }
                self.j = cnt as usize - 1;
                self.read_current(d);
                return;
            }
        }
    }

    unsafe fn read_current(&mut self, d: &Dict) {
        unsafe {
            let kv = (*d.bin.add(self.i)).keyvals;
            self.key = *kv.add(self.j * 2);
            self.value = *kv.add(self.j * 2 + 1);
        }
    }
}

pub fn hashstr(t: *const c_void) -> i32 {
    let s = t as *const i8;
    let mut p = s;
    let mut k = 0usize;
    let mut sum: i32 = 0;
    loop {
        let c = unsafe { *p };
        p = unsafe { p.add(1) };
        if c == 0 || k >= MAXID - 1 {
            break;
        }
        let c = (c as i32) << 1;
        let c = c + 1;
        sum += c + (c << SHFT[k]);
        k += 1;
    }
    (sum + XSUM[k]) >> 1
}

pub fn hashptr(key: *const c_void) -> i32 {
    ((key as isize) >> 2) as i32
}

pub fn hashkey(key: *const c_void) -> i32 {
    key as isize as i32
}

pub fn cmpstr(k1: *const c_void, k2: *const c_void) -> i32 {
    unsafe {
        CStr::from_ptr(k1 as *const i8).cmp(CStr::from_ptr(k2 as *const i8)) as i32
    }
}

pub fn cmpkey(key1: *const c_void, key2: *const c_void) -> i32 {
    if key1 == key2 {
        return 0;
    }
    if key1 > key2 {
        return 1;
    }
    -1
}
