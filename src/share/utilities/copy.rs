use std::ptr;

pub type HeapWord = u64;
pub const HEAP_WORD_SIZE: usize = size_of::<HeapWord>();

pub fn conjoint_words(from: &[HeapWord], to: &mut [HeapWord]) {
    let count = from.len().min(to.len());
    unsafe {
        ptr::copy(from.as_ptr(), to.as_mut_ptr(), count);
    }
}

pub fn disjoint_words(from: &[HeapWord], to: &mut [HeapWord]) {
    let count = from.len().min(to.len());
    unsafe {
        ptr::copy_nonoverlapping(from.as_ptr(), to.as_mut_ptr(), count);
    }
}

pub fn conjoint_jbytes(from: &[u8], to: &mut [u8]) {
    let count = from.len().min(to.len());
    unsafe {
        ptr::copy(from.as_ptr(), to.as_mut_ptr(), count);
    }
}

pub fn disjoint_jbytes(from: &[u8], to: &mut [u8]) {
    let count = from.len().min(to.len());
    unsafe {
        ptr::copy_nonoverlapping(from.as_ptr(), to.as_mut_ptr(), count);
    }
}

pub fn conjoint_jshorts(from: &[i16], to: &mut [i16]) {
    let count = from.len().min(to.len());
    unsafe {
        ptr::copy(from.as_ptr(), to.as_mut_ptr(), count);
    }
}

pub fn disjoint_jshorts(from: &[i16], to: &mut [i16]) {
    let count = from.len().min(to.len());
    unsafe {
        ptr::copy_nonoverlapping(from.as_ptr(), to.as_mut_ptr(), count);
    }
}

pub fn conjoint_jints(from: &[i32], to: &mut [i32]) {
    let count = from.len().min(to.len());
    unsafe {
        ptr::copy(from.as_ptr(), to.as_mut_ptr(), count);
    }
}

pub fn disjoint_jints(from: &[i32], to: &mut [i32]) {
    let count = from.len().min(to.len());
    unsafe {
        ptr::copy_nonoverlapping(from.as_ptr(), to.as_mut_ptr(), count);
    }
}

pub fn conjoint_jlongs(from: &[i64], to: &mut [i64]) {
    let count = from.len().min(to.len());
    unsafe {
        ptr::copy(from.as_ptr(), to.as_mut_ptr(), count);
    }
}

pub fn disjoint_jlongs(from: &[i64], to: &mut [i64]) {
    let count = from.len().min(to.len());
    unsafe {
        ptr::copy_nonoverlapping(from.as_ptr(), to.as_mut_ptr(), count);
    }
}

pub fn conjoint_oops<T: Copy>(from: &[T], to: &mut [T]) {
    let count = from.len().min(to.len());
    unsafe {
        ptr::copy(from.as_ptr(), to.as_mut_ptr(), count);
    }
}

pub fn disjoint_oops<T: Copy>(from: &[T], to: &mut [T]) {
    let count = from.len().min(to.len());
    unsafe {
        ptr::copy_nonoverlapping(from.as_ptr(), to.as_mut_ptr(), count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_copies() {
        let src = vec![1u64, 2, 3, 4];
        let mut dst = vec![0u64; 4];
        disjoint_words(&src, &mut dst);
        assert_eq!(dst, src);
    }

    #[test]
    fn byte_copies() {
        let src = vec![1u8, 2, 3];
        let mut dst = vec![0u8; 3];
        conjoint_jbytes(&src, &mut dst);
        assert_eq!(dst, src);
    }

    #[test]
    fn typed_copies() {
        let src = vec![1i64, -2, 3];
        let mut dst = vec![0i64; 3];
        conjoint_jlongs(&src, &mut dst);
        assert_eq!(dst, src);
        let src16 = vec![1i16, 2];
        let mut dst16 = vec![0i16; 2];
        conjoint_jshorts(&src16, &mut dst16);
        assert_eq!(dst16, src16);
        let src32 = vec![1i32, -2];
        let mut dst32 = vec![0i32; 2];
        disjoint_jints(&src32, &mut dst32);
        assert_eq!(dst32, src32);
        let mut oops = vec![7u64, 8];
        let mut oops_dst = vec![0u64; 2];
        conjoint_oops(&oops, &mut oops_dst);
        assert_eq!(oops_dst, oops);
        oops[0] = 9;
        assert_eq!(oops_dst[0], 7);
    }
}
