pub trait PrimitiveCast<To>: Sized {
    fn cast(self) -> To;
}

macro_rules! impl_int_cast {
    ($($from:ty => $to:ty),* $(,)?) => {
        $(
            impl PrimitiveCast<$to> for $from {
                #[inline]
                fn cast(self) -> $to {
                    self as $to
                }
            }
        )*
    };
}

macro_rules! impl_float_int_bit_cast {
    ($($float:ty => $int:ty),* $(,)?) => {
        $(
            impl PrimitiveCast<$int> for $float {
                #[inline]
                fn cast(self) -> $int {
                    self.to_bits() as $int
                }
            }
            impl PrimitiveCast<$float> for $int {
                #[inline]
                fn cast(self) -> $float {
                    <$float>::from_bits(self as _)
                }
            }
        )*
    };
}

impl_int_cast! {
    i8 => i8, i8 => u8, i8 => i16, i8 => u16, i8 => i32, i8 => u32,
    i8 => i64, i8 => u64, i8 => i128, i8 => u128, i8 => isize, i8 => usize,
    u8 => i8, u8 => u8, u8 => i16, u8 => u16, u8 => i32, u8 => u32,
    u8 => i64, u8 => u64, u8 => i128, u8 => u128, u8 => isize, u8 => usize,
    i16 => i8, i16 => u8, i16 => i16, i16 => u16, i16 => i32, i16 => u32,
    i16 => i64, i16 => u64, i16 => i128, i16 => u128, i16 => isize, i16 => usize,
    u16 => i8, u16 => u8, u16 => i16, u16 => u16, u16 => i32, u16 => u32,
    u16 => i64, u16 => u64, u16 => i128, u16 => u128, u16 => isize, u16 => usize,
    i32 => i8, i32 => u8, i32 => i16, i32 => u16, i32 => i32, i32 => u32,
    i32 => i64, i32 => u64, i32 => i128, i32 => u128, i32 => isize, i32 => usize,
    u32 => i8, u32 => u8, u32 => i16, u32 => u16, u32 => i32, u32 => u32,
    u32 => i64, u32 => u64, u32 => i128, u32 => u128, u32 => isize, u32 => usize,
    i64 => i8, i64 => u8, i64 => i16, i64 => u16, i64 => i32, i64 => u32,
    i64 => i64, i64 => u64, i64 => i128, i64 => u128, i64 => isize, i64 => usize,
    u64 => i8, u64 => u8, u64 => i16, u64 => u16, u64 => i32, u64 => u32,
    u64 => i64, u64 => u64, u64 => i128, u64 => u128, u64 => isize, u64 => usize,
    i128 => i8, i128 => u8, i128 => i16, i128 => u16, i128 => i32, i128 => u32,
    i128 => i64, i128 => u64, i128 => i128, i128 => u128, i128 => isize, i128 => usize,
    u128 => i8, u128 => u8, u128 => i16, u128 => u16, u128 => i32, u128 => u32,
    u128 => i64, u128 => u64, u128 => i128, u128 => u128, u128 => isize, u128 => usize,
    isize => i8, isize => u8, isize => i16, isize => u16, isize => i32, isize => u32,
    isize => i64, isize => u64, isize => i128, isize => u128, isize => isize, isize => usize,
    usize => i8, usize => u8, usize => i16, usize => u16, usize => i32, usize => u32,
    usize => i64, usize => u64, usize => i128, usize => u128, usize => isize, usize => usize,
    f32 => f32, f64 => f64,
}

impl_float_int_bit_cast! {
    f32 => i32,
    f32 => u32,
    f64 => i64,
    f64 => u64,
}

pub fn cast_ptr_to_int<T>(ptr: *const T) -> usize {
    ptr as usize
}

pub fn cast_int_to_ptr<T>(addr: usize) -> *const T {
    addr as *const T
}

pub fn cast_mut_ptr_to_int<T>(ptr: *mut T) -> usize {
    ptr as usize
}

pub fn cast_int_to_mut_ptr<T>(addr: usize) -> *mut T {
    addr as *mut T
}

pub trait Translate {
    type Decayed;
    fn decay(self) -> Self::Decayed;
    fn recover(decayed: Self::Decayed) -> Self;
}

impl Translate for f32 {
    type Decayed = i32;

    #[inline]
    fn decay(self) -> i32 {
        self.to_bits() as i32
    }

    #[inline]
    fn recover(decayed: i32) -> f32 {
        f32::from_bits(decayed as u32)
    }
}

impl Translate for f64 {
    type Decayed = i64;

    #[inline]
    fn decay(self) -> i64 {
        self.to_bits() as i64
    }

    #[inline]
    fn recover(decayed: i64) -> f64 {
        f64::from_bits(decayed as u64)
    }
}

#[macro_export]
macro_rules! impl_translate_for_enum {
    ($ty:ty, $decayed:ty) => {
        impl $crate::share::metaprogramming::primitive_conversions::Translate for $ty {
            type Decayed = $decayed;

            #[inline]
            fn decay(self) -> $decayed {
                self as $decayed
            }

            #[inline]
            fn recover(decayed: $decayed) -> Self {
                decayed as $ty
            }
        }
    };
}
