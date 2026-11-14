use std::io::{self, Error};

pub struct Exceptions;

impl Exceptions {
    pub const NULL_POINTER_EXCEPTION: &'static str = "java.lang.NullPointerException";
    pub const ARRAY_INDEX_OUT_OF_BOUNDS_EXCEPTION: &'static str =
        "java.lang.ArrayIndexOutOfBoundsException";
    pub const ARITHMETIC_EXCEPTION: &'static str = "java.lang.ArithmeticException";
    pub const CLASS_CAST_EXCEPTION: &'static str = "java.lang.ClassCastException";
    pub const CLASS_NOT_FOUND_EXCEPTION: &'static str = "java.lang.ClassNotFoundException";
    pub const NO_SUCH_METHOD_ERROR: &'static str = "java.lang.NoSuchMethodError";
    pub const NO_SUCH_FIELD_ERROR: &'static str = "java.lang.NoSuchFieldError";
    pub const ILLEGAL_MONITOR_STATE_EXCEPTION: &'static str =
        "java.lang.IllegalMonitorStateException";

    pub fn throw_msg(name: &str, msg: &str) -> io::Result<()> {
        Err(Error::other(format!("{name}: {msg}")))
    }
}
