pub const K: usize = 1024;
pub const M: usize = K * 1024;
pub const G: usize = M * 1024;

pub const BITS_PER_BYTE: usize = 8;
pub const BYTES_PER_WORD: usize = size_of::<usize>();
pub const BITS_PER_WORD: usize = BYTES_PER_WORD * BITS_PER_BYTE;

pub const fn align_up(size: usize, alignment: usize) -> usize {
    (size + alignment - 1) & !(alignment - 1)
}

pub const fn align_down(size: usize, alignment: usize) -> usize {
    size & !(alignment - 1)
}

pub const fn is_aligned(size: usize, alignment: usize) -> bool {
    (size & (alignment - 1)) == 0
}

pub const fn is_power_of_2(x: usize) -> bool {
    x > 0 && (x & (x - 1)) == 0
}
