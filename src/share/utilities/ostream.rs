use crate::share::utilities::formatBuffer::{FormatArg, format};
use std::io::{self, Write as IoWrite};

pub trait OutputStream: IoWrite {
    fn print(&mut self, s: &str) -> io::Result<()> {
        self.write_all(s.as_bytes())
    }

    fn print_fmt(&mut self, fmt: &str, args: &[FormatArg]) -> io::Result<()> {
        self.write_all(format(fmt, args).as_bytes())
    }

    fn print_cr(&mut self) -> io::Result<()> {
        self.write_all(b"\n")
    }

    fn println(&mut self, s: &str) -> io::Result<()> {
        self.write_all(s.as_bytes())?;
        self.write_all(b"\n")
    }

    fn print_jlong(&mut self, v: i64) -> io::Result<()> {
        self.print_fmt("%d", &[FormatArg::i64(v)])
    }

    fn print_julong(&mut self, v: u64) -> io::Result<()> {
        self.print_fmt("%u", &[FormatArg::u64(v)])
    }

    fn print_jint(&mut self, v: i32) -> io::Result<()> {
        self.print_fmt("%d", &[FormatArg::i64(v as i64)])
    }

    fn print_juint(&mut self, v: u32) -> io::Result<()> {
        self.print_fmt("%u", &[FormatArg::u64(v as u64)])
    }

    fn print_ptr(&mut self, p: usize) -> io::Result<()> {
        self.print_fmt("%p", &[FormatArg::ptr(p)])
    }

    fn print_address(&mut self, p: usize) -> io::Result<()> {
        self.print_fmt("%p", &[FormatArg::ptr(p)])
    }

    fn print_raw(&mut self, s: &str) -> io::Result<()> {
        self.write_all(s.as_bytes())
    }

    fn write_text(&mut self, s: &str) -> io::Result<()> {
        self.print(s)
    }

    fn cr(&mut self) -> io::Result<()> {
        self.print_cr()
    }
}

impl<W: IoWrite> OutputStream for W {}

#[derive(Default)]
pub struct StringStream {
    buffer: String,
}

impl StringStream {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buffer: String::with_capacity(capacity),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.buffer
    }

    pub fn into_string(self) -> String {
        self.buffer
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn reset(&mut self) {
        self.buffer.clear();
    }

    pub fn print(&mut self, s: &str) {
        self.buffer.push_str(s);
    }

    pub fn print_fmt(&mut self, fmt: &str, args: &[FormatArg]) {
        self.buffer.push_str(&format(fmt, args));
    }

    pub fn print_cr(&mut self) {
        self.buffer.push('\n');
    }

    pub fn print_jlong(&mut self, v: i64) {
        self.buffer.push_str(&v.to_string());
    }

    pub fn print_julong(&mut self, v: u64) {
        self.buffer.push_str(&v.to_string());
    }

    pub fn print_ptr(&mut self, p: usize) {
        self.buffer.push_str(&format!("0x{:x}", p));
    }

    pub fn print_raw(&mut self, s: &str) {
        self.buffer.push_str(s);
    }
}

impl io::Write for StringStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let s = std::str::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.buffer.push_str(s);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}


impl std::fmt::Display for StringStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::utilities::formatBuffer::FormatArg;

    #[test]
    fn string_stream_print() {
        let mut ss = StringStream::new();
        ss.print("hello ");
        ss.print_fmt("%s %d", &[FormatArg::str("world"), FormatArg::i64(7)]);
        ss.print_cr();
        assert_eq!(ss.as_str(), "hello world 7\n");
        ss.reset();
        assert!(ss.is_empty());
    }

    #[test]
    fn trait_print() {
        let mut ss = StringStream::new();
        OutputStream::print(&mut ss, "abc").unwrap();
        OutputStream::print_cr(&mut ss).unwrap();
        OutputStream::print_ptr(&mut ss, 0xff).unwrap();
        assert_eq!(ss.as_str(), "abc\n0xff");
    }
}
