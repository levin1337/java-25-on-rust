pub fn count_leading_zeros(x: u64) -> u32 {
    x.leading_zeros()
}

pub fn count_trailing_zeros(x: u64) -> u32 {
    x.trailing_zeros()
}

pub fn population_count(x: u64) -> u32 {
    x.count_ones()
}

pub fn next_power_of_2(x: usize) -> usize {
    if x == 0 {
        return 1;
    }
    x.next_power_of_two()
}

pub fn round_to_power_of_2(x: usize) -> usize {
    if x == 0 {
        return 0;
    }
    let p = next_power_of_2(x);
    if p == x {
        p
    } else {
        p / 2
    }
}

pub fn is_power_of_2(x: usize) -> bool {
    x != 0 && (x & (x - 1)) == 0
}

pub fn reverse_bits_u32(mut x: u32) -> u32 {
    x = ((x & 0x55555555) << 1) | ((x >> 1) & 0x55555555);
    x = ((x & 0x33333333) << 2) | ((x >> 2) & 0x33333333);
    x = ((x & 0x0F0F0F0F) << 4) | ((x >> 4) & 0x0F0F0F0F);
    x = ((x & 0x00FF00FF) << 8) | ((x >> 8) & 0x00FF00FF);
    (x << 16) | (x >> 16)
}

pub fn reverse_bits_u64(mut x: u64) -> u64 {
    x = ((x & 0x5555555555555555) << 1) | ((x >> 1) & 0x5555555555555555);
    x = ((x & 0x3333333333333333) << 2) | ((x >> 2) & 0x3333333333333333);
    x = ((x & 0x0F0F0F0F0F0F0F0F) << 4) | ((x >> 4) & 0x0F0F0F0F0F0F0F0F);
    x = ((x & 0x00FF00FF00FF00FF) << 8) | ((x >> 8) & 0x00FF00FF00FF00FF);
    x = ((x & 0x0000FFFF0000FFFF) << 16) | ((x >> 16) & 0x0000FFFF0000FFFF);
    (x << 32) | (x >> 32)
}

pub fn rotate_left_u32(x: u32, n: u32) -> u32 {
    x.rotate_left(n & 31)
}

pub fn rotate_right_u32(x: u32, n: u32) -> u32 {
    x.rotate_right(n & 31)
}

pub fn rotate_left_u64(x: u64, n: u64) -> u64 {
    x.rotate_left((n & 63) as u32)
}

pub fn rotate_right_u64(x: u64, n: u64) -> u64 {
    x.rotate_right((n & 63) as u32)
}

pub fn byteswap_u16(x: u16) -> u16 {
    x.swap_bytes()
}

pub fn byteswap_u32(x: u32) -> u32 {
    x.swap_bytes()
}

pub fn byteswap_u64(x: u64) -> u64 {
    x.swap_bytes()
}

pub fn bit_index(bit: usize) -> usize {
    bit
}

pub fn bit_in_word(bit: usize) -> u32 {
    (bit % crate::share::utilities::globalDefinitions::BITS_PER_WORD) as u32
}

pub fn bit_mask(bit: usize) -> u64 {
    1u64 << bit_in_word(bit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts() {
        assert_eq!(count_leading_zeros(1), 63);
        assert_eq!(count_leading_zeros(0), 64);
        assert_eq!(count_trailing_zeros(8), 3);
        assert_eq!(population_count(0xF0F0), 8);
    }

    #[test]
    fn powers() {
        assert_eq!(next_power_of_2(0), 1);
        assert_eq!(next_power_of_2(5), 8);
        assert_eq!(next_power_of_2(8), 8);
        assert_eq!(round_to_power_of_2(5), 4);
        assert_eq!(round_to_power_of_2(8), 8);
        assert!(is_power_of_2(16));
        assert!(!is_power_of_2(18));
    }

    #[test]
    fn reverse_and_rotate() {
        assert_eq!(reverse_bits_u32(0x80000000), 1);
        assert_eq!(reverse_bits_u32(0x0F0F0F0F), 0xF0F0F0F0);
        assert_eq!(reverse_bits_u64(1), 1u64 << 63);
        assert_eq!(rotate_left_u32(1, 4), 16);
        assert_eq!(rotate_right_u64(1, 1), 1u64 << 63);
        assert_eq!(byteswap_u32(0x01020304), 0x04030201);
        assert_eq!(byteswap_u16(0x0102), 0x0201);
        assert_eq!(byteswap_u64(0x0102030405060708), 0x0807060504030201);
    }
}
