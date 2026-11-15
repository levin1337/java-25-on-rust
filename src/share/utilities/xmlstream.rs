use crate::share::utilities::formatBuffer::{format, FormatArg};
use std::io::{self, Write};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarkupState {
    Body,
    Head,
    Elem,
}

pub struct XmlStream<W: Write> {
    out: W,
    last_flush: u64,
    written: u64,
    markup_state: MarkupState,
    tag_stack: Vec<String>,
    stamp_fn: Box<dyn Fn() -> String>,
}

impl<W: Write> XmlStream<W> {
    pub fn new(out: W) -> Self {
        Self {
            out,
            last_flush: 0,
            written: 0,
            markup_state: MarkupState::Body,
            tag_stack: Vec::new(),
            stamp_fn: Box::new(default_stamp),
        }
    }

    pub fn with_stamp_fn(out: W, f: impl Fn() -> String + 'static) -> Self {
        Self {
            stamp_fn: Box::new(f),
            ..Self::new(out)
        }
    }

    pub fn set_stamp_fn(&mut self, f: impl Fn() -> String + 'static) {
        self.stamp_fn = Box::new(f);
    }

    pub fn is_open(&self) -> bool {
        true
    }

    pub fn inside_attrs(&self) -> bool {
        self.markup_state != MarkupState::Body
    }

    pub fn markup_state(&self) -> MarkupState {
        self.markup_state
    }

    pub fn count(&self) -> u64 {
        self.written
    }

    pub fn unflushed_count(&self) -> u64 {
        self.written - self.last_flush
    }

    fn raw_write(&mut self, s: &str) -> io::Result<()> {
        self.out.write_all(s.as_bytes())?;
        self.written += s.len() as u64;
        Ok(())
    }

    pub fn write(&mut self, s: &str) -> io::Result<()> {
        self.raw_write(s)
    }

    pub fn write_bytes(&mut self, buf: &[u8]) -> io::Result<()> {
        self.out.write_all(buf)?;
        self.written += buf.len() as u64;
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.out.flush()?;
        self.last_flush = self.written;
        Ok(())
    }

    pub fn print_raw(&mut self, s: &str) -> io::Result<()> {
        self.raw_write(s)
    }

    pub fn print_raw_cr(&mut self, s: &str) -> io::Result<()> {
        self.raw_write(s)?;
        self.raw_write("\n")
    }

    pub fn print_cr(&mut self, s: &str) -> io::Result<()> {
        self.raw_write(s)?;
        self.raw_write("\n")
    }

    pub fn cr(&mut self) -> io::Result<()> {
        self.raw_write("\n")
    }

    pub fn write_text(&mut self, s: &str) -> io::Result<()> {
        for ch in s.chars() {
            let esc = match ch {
                '\'' => Some("&apos;"),
                '"' => Some("&quot;"),
                '<' => Some("&lt;"),
                '&' => Some("&amp;"),
                '>' => Some("&gt;"),
                _ => None,
            };
            match esc {
                Some(e) => self.raw_write(e)?,
                None => {
                    let mut buf = [0u8; 4];
                    self.raw_write(ch.encode_utf8(&mut buf))?;
                }
            }
        }
        Ok(())
    }

    fn tag_kind(kind: &str) -> String {
        kind.split_whitespace().next().unwrap_or("").to_string()
    }

    fn va_tag(&mut self, push: bool, kind: &str) -> io::Result<()> {
        debug_assert!(!self.inside_attrs(), "cannot print tag inside attrs");
        if push {
            let k = Self::tag_kind(kind);
            debug_assert!(!k.is_empty(), "tag must not be empty");
            self.tag_stack.push(k);
        }
        self.raw_write("<")?;
        self.raw_write(kind)?;
        self.markup_state = if push { MarkupState::Head } else { MarkupState::Elem };
        Ok(())
    }

    fn pop_tag(&mut self, tag: &str) {
        let mut bad = false;
        while let Some(cur) = self.tag_stack.last() {
            if cur == tag {
                self.tag_stack.pop();
                break;
            }
            let cur = self.tag_stack.pop().unwrap();
            let _ = self.print_raw(&format!("</{}> <!-- missing closing tag -->\n", cur));
            bad = true;
        }
        if self.tag_stack.is_empty() && bad {
            // fell off the stack without finding tag
        }
        debug_assert!(!bad, "bad tag in log");
    }

    pub fn elem(&mut self, kind: &str, attrs: &[FormatArg]) -> io::Result<()> {
        self.begin_elem(kind, attrs)?;
        self.end_elem()
    }

    pub fn begin_elem(&mut self, kind: &str, attrs: &[FormatArg]) -> io::Result<()> {
        let full = format(kind, attrs);
        self.va_tag(false, &full)
    }

    pub fn end_elem(&mut self) -> io::Result<()> {
        debug_assert!(self.markup_state == MarkupState::Elem, "misplaced end_elem");
        self.raw_write("/>\n")?;
        self.markup_state = MarkupState::Body;
        Ok(())
    }

    pub fn end_elem_attrs(&mut self, fmt: &str, args: &[FormatArg]) -> io::Result<()> {
        self.raw_write(&format(fmt, args))?;
        self.end_elem()
    }

    pub fn head(&mut self, kind: &str, attrs: &[FormatArg]) -> io::Result<()> {
        self.begin_head(kind, attrs)?;
        self.end_head()
    }

    pub fn begin_head(&mut self, kind: &str, attrs: &[FormatArg]) -> io::Result<()> {
        let full = format(kind, attrs);
        self.va_tag(true, &full)
    }

    pub fn end_head(&mut self) -> io::Result<()> {
        debug_assert!(self.markup_state == MarkupState::Head, "misplaced end_head");
        self.raw_write(">\n")?;
        self.markup_state = MarkupState::Body;
        Ok(())
    }

    pub fn end_head_attrs(&mut self, fmt: &str, args: &[FormatArg]) -> io::Result<()> {
        self.raw_write(&format(fmt, args))?;
        self.end_head()
    }

    pub fn tail(&mut self, kind: &str) -> io::Result<()> {
        self.pop_tag(kind);
        self.raw_write("</")?;
        self.raw_write(kind)?;
        self.raw_write(">\n")
    }

    pub fn done(&mut self, kind: &str, attrs: &[FormatArg]) -> io::Result<()> {
        let kind_only = Self::tag_kind(kind);
        let done_kind = format!("{}_done", kind_only);
        let mut full = done_kind;
        let rest = kind[kind_only.len()..].trim_start();
        if !rest.is_empty() {
            full.push(' ');
            full.push_str(rest);
        }
        self.begin_elem(&full, attrs)?;
        self.stamp()?;
        self.end_elem()?;
        self.tail(&kind_only)
    }

    pub fn done_raw(&mut self, kind: &str) -> io::Result<()> {
        let stamp = (self.stamp_fn)();
        self.raw_write("<")?;
        self.raw_write(kind)?;
        self.raw_write("_done stamp='")?;
        self.raw_write(&stamp)?;
        self.raw_write("'/>\n")?;
        self.raw_write("</")?;
        self.raw_write(kind)?;
        self.raw_write(">\n")
    }

    pub fn stamp(&mut self) -> io::Result<()> {
        debug_assert!(self.inside_attrs(), "stamp must be an attribute");
        let stamp = (self.stamp_fn)();
        self.raw_write(" stamp='")?;
        self.raw_write(&stamp)?;
        self.raw_write("'")
    }

    pub fn text(&mut self, fmt: &str, args: &[FormatArg]) -> io::Result<()> {
        let s = format(fmt, args);
        self.write_text(&s)
    }

    pub fn text_stream(&mut self) -> XmlTextStream<'_, W> {
        XmlTextStream { outer: self }
    }

    pub fn into_inner(self) -> W {
        self.out
    }
}

fn default_stamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs_f64();
    format!("{:.3}", secs)
}

pub struct XmlTextStream<'a, W: Write> {
    outer: &'a mut XmlStream<W>,
}

impl<'a, W: Write> XmlTextStream<'a, W> {
    pub fn print(&mut self, s: &str) -> io::Result<()> {
        self.outer.write_text(s)
    }

    pub fn print_fmt(&mut self, fmt: &str, args: &[FormatArg]) -> io::Result<()> {
        let s = format(fmt, args);
        self.outer.write_text(&s)
    }
}

impl<'a, W: Write> Write for XmlTextStream<'a, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let s = std::str::from_utf8(buf)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.outer.write_text(s)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.outer.flush()
    }
}

impl<W: Write> Write for XmlStream<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_bytes(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        XmlStream::flush(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::utilities::formatBuffer::FormatArg;

    #[test]
    fn elem_simple() {
        let mut out = Vec::new();
        {
            let mut x = XmlStream::new(&mut out);
            x.elem("X Y='Z'", &[]).unwrap();
        }
        assert_eq!(String::from_utf8(out).unwrap(), "<X Y='Z'/>\n");
    }

    #[test]
    fn head_tail() {
        let mut out = Vec::new();
        {
            let mut x = XmlStream::new(&mut out);
            x.head("top a='1'", &[]).unwrap();
            x.elem("child", &[]).unwrap();
            x.text("plain <text> & \"q\"", &[]).unwrap();
            x.tail("top").unwrap();
        }
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("<top a='1'>\n"));
        assert!(s.contains("<child/>\n"));
        assert!(s.contains("plain &lt;text&gt; &amp; &quot;q&quot;"));
        assert!(s.contains("</top>\n"));
    }

    #[test]
    fn begin_end_general() {
        let mut out = Vec::new();
        {
            let mut x = XmlStream::new(&mut out);
            x.begin_head("A", &[]).unwrap();
            x.print_raw(" extra='v'").unwrap();
            x.end_head().unwrap();
            x.begin_elem("B", &[]).unwrap();
            x.end_elem_attrs(" tail='t'", &[]).unwrap();
        }
        let s = String::from_utf8(out).unwrap();
        assert_eq!(s, "<A extra='v'>\n<B tail='t'/>\n");
    }

    #[test]
    fn done_event() {
        let mut out = Vec::new();
        {
            let mut x = XmlStream::with_stamp_fn(&mut out, || "1.500".to_string());
            x.head("kind", &[]).unwrap();
            x.done("kind", &[]).unwrap();
        }
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("<kind_done stamp='1.500'/>"));
        assert!(s.contains("</kind>"));
    }

    #[test]
    fn text_stream_escaping() {
        let mut out = Vec::new();
        {
            let mut x = XmlStream::new(&mut out);
            x.head("t", &[]).unwrap();
            {
                let mut ts = x.text_stream();
                ts.print("a<b>&\"'").unwrap();
            }
            x.tail("t").unwrap();
        }
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("a&lt;b&gt;&amp;&quot;&apos;"));
    }
}
