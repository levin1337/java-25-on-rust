pub const ATTRIBUTE_NO_UBSAN: &str = "";

pub const fn no_ubsan_attribute<F>(f: F) -> F {
    f
}
