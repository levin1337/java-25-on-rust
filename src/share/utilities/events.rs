use crate::share::utilities::formatBuffer::{format, FormatArg};
use std::io::{self, Write};
use std::sync::{Mutex, OnceLock};

pub const LOG_EVENTS_BUFFER_ENTRIES: usize = 2000;

#[derive(Clone, Debug)]
pub struct EventRecord {
    pub timestamp: f64,
    pub thread: Option<String>,
    pub message: String,
}

pub trait EventLog {
    fn print_log_on(&self, out: &mut dyn Write, max: isize);
    fn matches_name_or_handle(&self, s: &str) -> bool;
    fn print_names(&self, out: &mut dyn Write);
    fn name(&self) -> &str;
    fn handle(&self) -> &str;
}

#[derive(Clone)]
pub struct EventLogBase {
    pub name: String,
    pub handle: String,
    length: usize,
    index: usize,
    count: usize,
    records: Vec<Option<EventRecord>>,
}

impl EventLogBase {
    pub fn new(name: &str, handle: &str, length: usize) -> Self {
        Self {
            name: name.to_string(),
            handle: handle.to_string(),
            length,
            index: 0,
            count: 0,
            records: vec![None; length],
        }
    }

    pub fn fetch_timestamp() -> f64 {
        static START: OnceLock<std::time::Instant> = OnceLock::new();
        let start = START.get_or_init(std::time::Instant::now);
        start.elapsed().as_secs_f64()
    }

    fn compute_log_index(&mut self) -> usize {
        let index = self.index;
        if self.count < self.length {
            self.count += 1;
        }
        self.index += 1;
        if self.index >= self.length {
            self.index = 0;
        }
        index
    }

    pub fn should_log() -> bool {
        true
    }

    pub fn log(&mut self, thread: Option<&str>, fmt: &str, args: &[FormatArg]) {
        if !Self::should_log() {
            return;
        }
        let index = self.compute_log_index();
        self.records[index] = Some(EventRecord {
            timestamp: Self::fetch_timestamp(),
            thread: thread.map(|s| s.to_string()),
            message: format(fmt, args),
        });
    }

    pub fn log_message(&mut self, thread: Option<&str>, message: String) {
        self.log(thread, "%s", &[FormatArg::string(message)]);
    }

    fn print_log_impl(&self, out: &mut dyn Write, max: isize) {
        let _ = writeln!(out, "Log {} ({}):", self.name, self.handle);
        let mut printed = 0isize;
        let start = if self.count < self.length { 0 } else { self.index };
        for k in 0..self.count {
            let idx = (start + k) % self.length;
            if let Some(rec) = &self.records[idx] {
                if max >= 0 && printed >= max {
                    break;
                }
                let _ = write!(out, "Event: {:.3} ", rec.timestamp);
                if let Some(t) = &rec.thread {
                    let _ = write!(out, "Thread \"{}\" ", t);
                }
                let _ = writeln!(out, "{}", rec.message);
                printed += 1;
            }
        }
    }
}

impl EventLog for EventLogBase {
    fn print_log_on(&self, out: &mut dyn Write, max: isize) {
        self.print_log_impl(out, max);
    }

    fn matches_name_or_handle(&self, s: &str) -> bool {
        s == self.name || s == self.handle
    }

    fn print_names(&self, out: &mut dyn Write) {
        let _ = write!(out, "{} ({})", self.name, self.handle);
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn handle(&self) -> &str {
        &self.handle
    }
}

pub type StringEventLog = EventLogBase;
pub type FormatStringEventLog = EventLogBase;

pub struct EventsState {
    logs: Vec<Box<EventLogBase>>,
}

impl EventsState {
    fn new() -> Self {
        Self { logs: Vec::new() }
    }

    fn register(&mut self, log: Box<EventLogBase>) {
        self.logs.push(log);
    }
}

static EVENTS: OnceLock<Mutex<EventsState>> = OnceLock::new();

fn state() -> &'static Mutex<EventsState> {
    EVENTS.get_or_init(|| Mutex::new(EventsState::new()))
}

pub fn events_init(enabled: bool) {
    let mut st = state().lock().unwrap();
    if !enabled || !st.logs.is_empty() {
        return;
    }
    let names: Vec<(&str, &str)> = vec![
        ("Events", "events"),
        ("Nmethod flushes", "nmethodflushes"),
        ("Memory protections", "memprotects"),
        ("VM Operations", "vmops"),
        ("ZGC Phase Switch", "zgcps"),
        ("Internal exceptions", "exc"),
        ("Classes redefined", "redef"),
        ("Classes unloaded", "unload"),
        ("Classes loaded", "load"),
        ("Deoptimization events", "deopt"),
        ("Dll operation events", "dll"),
    ];
    for (n, h) in names {
        st.register(Box::new(EventLogBase::new(n, h, LOG_EVENTS_BUFFER_ENTRIES)));
    }
}

macro_rules! log_to {
    ($handle:expr, $thread:expr, $fmt:expr, $args:expr) => {{
        let mut st = state().lock().unwrap();
        if let Some(log) = st.logs.iter_mut().find(|l| l.handle == $handle) {
            log.log($thread, $fmt, $args);
        }
    }};
}

pub fn log(thread: Option<&str>, fmt: &str, args: &[FormatArg]) {
    log_to!("events", thread, fmt, args);
}

pub fn log_memprotect(thread: Option<&str>, fmt: &str, args: &[FormatArg]) {
    log_to!("memprotects", thread, fmt, args);
}

pub fn log_nmethod_flush(thread: Option<&str>, fmt: &str, args: &[FormatArg]) {
    log_to!("nmethodflushes", thread, fmt, args);
}

pub fn log_vm_operation(thread: Option<&str>, fmt: &str, args: &[FormatArg]) {
    log_to!("vmops", thread, fmt, args);
}

pub fn log_zgc_phase_switch(fmt: &str, args: &[FormatArg]) {
    log_to!("zgcps", None, fmt, args);
}

pub fn log_exception(thread: Option<&str>, fmt: &str, args: &[FormatArg]) {
    log_to!("exc", thread, fmt, args);
}

pub fn log_redefinition(thread: Option<&str>, fmt: &str, args: &[FormatArg]) {
    log_to!("redef", thread, fmt, args);
}

pub fn log_class_unloading(thread: Option<&str>, message: &str) {
    log_to!("unload", thread, "%s", &[FormatArg::string(message.to_string())]);
}

pub fn log_class_loading(thread: Option<&str>, fmt: &str, args: &[FormatArg]) {
    log_to!("load", thread, fmt, args);
}

pub fn log_deopt(thread: Option<&str>, fmt: &str, args: &[FormatArg]) {
    log_to!("deopt", thread, fmt, args);
}

pub fn log_dll(thread: Option<&str>, fmt: &str, args: &[FormatArg]) {
    log_to!("dll", thread, fmt, args);
}

pub fn print_all(out: &mut dyn Write, max: isize) -> io::Result<()> {
    let st = state().lock().unwrap();
    for log in &st.logs {
        log.print_log_on(out, max);
    }
    Ok(())
}

pub fn print_one(out: &mut dyn Write, log_name: &str, max: isize) -> io::Result<()> {
    let st = state().lock().unwrap();
    let mut num_printed = 0;
    for log in &st.logs {
        if log.matches_name_or_handle(log_name) {
            log.print_log_on(out, max);
            num_printed += 1;
        }
    }
    if num_printed == 0 {
        writeln!(
            out,
            "The name \"{}\" did not match any known event log. Valid event log names are:",
            log_name
        )?;
        for log in &st.logs {
            log.print_names(out);
            writeln!(out)?;
        }
    }
    Ok(())
}

pub struct EventMark {
    buffer: String,
}

impl EventMark {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    pub fn log_start(&mut self, fmt: &str, args: &[FormatArg]) {
        self.buffer = format(fmt, args);
        log(None, "%s", &[FormatArg::string(self.buffer.clone())]);
    }

    pub fn log_end(&mut self) {
        self.buffer.push_str(" done");
        log(None, "%s", &[FormatArg::string(self.buffer.clone())]);
    }
}

impl Default for EventMark {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::utilities::formatBuffer::FormatArg;

    #[test]
    fn ring_buffer_log() {
        let mut log = EventLogBase::new("Test", "test", 4);
        for i in 0..6 {
            log.log(Some("thread1"), "msg %d", &[FormatArg::i64(i)]);
        }
        assert_eq!(log.count, 4);
        let mut out = Vec::new();
        log.print_log_on(&mut out, -1);
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("msg 2"));
        assert!(s.contains("msg 5"));
        assert!(!s.contains("msg 0"));
        assert!(log.matches_name_or_handle("test"));
        assert!(log.matches_name_or_handle("Test"));
        assert!(!log.matches_name_or_handle("other"));
    }

    #[test]
    fn events_framework() {
        events_init(true);
        log(Some("t"), "hello %s", &[FormatArg::str("world")]);
        log_memprotect(None, "prot %d", &[FormatArg::i64(1)]);
        let mut out = Vec::new();
        print_all(&mut out, -1).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("hello world"));
        let mut out2 = Vec::new();
        print_one(&mut out2, "events", -1).unwrap();
        assert!(String::from_utf8(out2).unwrap().contains("hello world"));
        let mut mark = EventMark::new();
        mark.log_start("op %d", &[FormatArg::i64(3)]);
        mark.log_end();
    }
}
