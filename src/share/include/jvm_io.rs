use std::fmt;
use std::fmt::Write as _;
use std::io::{self, Write};

fn write_truncated(buf: &mut [u8], text: &str) -> i32 {
    if buf.is_empty() {
        return -1;
    }
    let max = buf.len() - 1;
    let bytes = text.as_bytes();
    let n = bytes.len().min(max);
    buf[..n].copy_from_slice(&bytes[..n]);
    buf[n] = 0;
    if bytes.len() > max {
        -1
    } else {
        n as i32
    }
}

pub fn jio_vsnprintf(buf: &mut [u8], args: fmt::Arguments<'_>) -> i32 {
    let mut s = String::new();
    if s.write_fmt(args).is_err() {
        return -1;
    }
    write_truncated(buf, &s)
}

pub fn jio_snprintf(buf: &mut [u8], args: fmt::Arguments<'_>) -> i32 {
    jio_vsnprintf(buf, args)
}

pub fn jio_fprintf<W: Write>(out: &mut W, args: fmt::Arguments<'_>) -> io::Result<i32> {
    let mut s = String::new();
    s.write_fmt(args).map_err(io::Error::other)?;
    let n = s.len() as i32;
    out.write_all(s.as_bytes())?;
    Ok(n)
}

pub fn jio_vfprintf<W: Write>(out: &mut W, args: fmt::Arguments<'_>) -> io::Result<i32> {
    jio_fprintf(out, args)
}
