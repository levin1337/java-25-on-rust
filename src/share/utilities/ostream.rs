use std::io::{self, Write as IoWrite};

pub trait OutputStream: IoWrite {
    fn print(&mut self, s: &str) -> io::Result<()> {
        self.write_all(s.as_bytes())
    }

    fn println(&mut self, s: &str) -> io::Result<()> {
        self.write_all(s.as_bytes())?;
        self.write_all(b"\n")
    }

    fn print_cr(&mut self) -> io::Result<()> {
        self.write_all(b"\n")
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

    pub fn as_str(&self) -> &str {
        &self.buffer
    }

    pub fn into_string(self) -> String {
        self.buffer
    }
}

impl io::Write for StringStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let s =
            std::str::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.buffer.push_str(s);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
