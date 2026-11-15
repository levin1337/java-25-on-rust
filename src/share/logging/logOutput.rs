/*
 * Port of OpenJDK 25 HotSpot LogOutput hierarchy
 * (share/logging/logOutput.hpp, logFileStreamOutput.cpp/.hpp,
 *  logFileOutput.cpp/.hpp)
 *
 * LogOutput is the sink for one configured -Xlog output. LogFileStreamOutput
 * is the shared base for stdout/stderr/file outputs: it renders decorations
 * with per-output column padding, supports foldmultilines, and flushes after
 * every record. LogFileOutput adds size-based rotation (filesize/filecount)
 * and %p/%t/%hn filename placeholders.
 */

use super::logDecorations::{Decorations, LogDecorations, N_DECORATORS};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// A single configured log output destination.
pub trait LogOutput: Send + Sync {
    /// write_internal + write_decorations equivalent: renders the full
    /// record (decorations + message, multi-line aware) and writes it.
    /// Returns the number of bytes written.
    fn write_record(
        &self,
        decorations: &LogDecorations,
        decorators: &Decorations,
        msg: &str,
    ) -> io::Result<usize>;

    fn flush(&self) -> io::Result<()>;
    fn name(&self) -> &str;
    /// LogOutput::set_option — key=value output option (foldmultilines).
    fn set_option(&self, _key: &str, _value: &str) -> bool {
        false
    }
    /// LogOutput::describe — one-line description for VM.log print.
    fn describe(&self) -> String;
    /// LogFileOutput::force_rotate — no-op for non-rotating outputs.
    fn force_rotate(&self) {}
}

enum Sink {
    Stdout,
    Stderr,
    File(RotatingFile),
    /// Output that failed to initialize; writes are dropped.
    Failed,
}

struct OutState {
    sink: Sink,
    padding: [usize; N_DECORATORS],
    fold_multilines: bool,
    write_error_shown: bool,
}

/// LogFileStreamOutput + LogFileOutput unified: stdout, stderr and
/// rotating file outputs share the same write path.
pub struct StreamLogOutput {
    name: String,
    state: Mutex<OutState>,
}

impl StreamLogOutput {
    /// LogStdoutOutput — name "stdout".
    pub fn stdout() -> Self {
        Self {
            name: "stdout".to_string(),
            state: Mutex::new(OutState {
                sink: Sink::Stdout,
                padding: [0; N_DECORATORS],
                fold_multilines: false,
                write_error_shown: false,
            }),
        }
    }

    /// LogStderrOutput — name "stderr".
    pub fn stderr() -> Self {
        Self {
            name: "stderr".to_string(),
            state: Mutex::new(OutState {
                sink: Sink::Stderr,
                padding: [0; N_DECORATORS],
                fold_multilines: false,
                write_error_shown: false,
            }),
        }
    }

    /// LogFileOutput — name is the normalized "file=<path>" spec;
    /// `options` is the raw output-options string ("filesize=..,filecount=..
    /// ,foldmultilines=.."). Initialization failures yield a Failed sink
    /// that drops writes, mirroring the C++ error path.
    pub fn file(name_spec: &str, options: &str) -> Self {
        let raw_path = name_spec
            .strip_prefix("file=")
            .unwrap_or(name_spec)
            .trim_matches('"');
        let file_name = expand_filename_placeholders(raw_path);
        let mut state = OutState {
            sink: Sink::Failed,
            padding: [0; N_DECORATORS],
            fold_multilines: false,
            write_error_shown: false,
        };
        if let Ok((rot, fold)) = RotatingFile::initialize(&file_name, options) {
            state.sink = Sink::File(rot);
            state.fold_multilines = fold;
        }
        Self {
            name: name_spec.to_string(),
            state: Mutex::new(state),
        }
    }

    /// Output that always drops writes (used for invalid output specs).
    pub fn failed(name: &str) -> Self {
        Self {
            name: name.to_string(),
            state: Mutex::new(OutState {
                sink: Sink::Failed,
                padding: [0; N_DECORATORS],
                fold_multilines: false,
                write_error_shown: false,
            }),
        }
    }

    fn write_all(&self, state: &mut OutState, bytes: &[u8]) -> io::Result<usize> {
        match &mut state.sink {
            Sink::Stdout => {
                let mut out = io::stdout().lock();
                out.write_all(bytes)?;
                out.flush()?;
            }
            Sink::Stderr => {
                let mut err = io::stderr().lock();
                err.write_all(bytes)?;
                err.flush()?;
            }
            Sink::File(rot) => {
                rot.file.write_all(bytes)?;
                rot.file.flush()?;
            }
            Sink::Failed => return Ok(0),
        }
        Ok(bytes.len())
    }
}

impl LogOutput for StreamLogOutput {
    fn write_record(
        &self,
        decorations: &LogDecorations,
        decorators: &Decorations,
        msg: &str,
    ) -> io::Result<usize> {
        let mut state = self.state.lock().unwrap();
        if matches!(state.sink, Sink::Failed) {
            return Ok(0);
        }

        let mut buf = String::new();
        let deco_block_len;
        if decorators.is_empty() {
            deco_block_len = 0;
        } else {
            let prefix = decorations.render_prefix(&mut state.padding);
            deco_block_len = prefix.len().saturating_sub(1);
            buf.push_str(&prefix);
        }

        if !state.fold_multilines {
            // Split on newlines; continuation lines get a blank decoration
            // field of the same width (write_internal, !fold_multilines).
            let cont = if deco_block_len > 0 {
                LogDecorations::continuation_prefix(deco_block_len)
            } else {
                String::new()
            };
            let mut first = true;
            let mut base = msg;
            loop {
                match base.find('\n') {
                    Some(pos) => {
                        if first {
                            buf.push_str(&base[..pos]);
                            first = false;
                        } else {
                            buf.push_str(&cont);
                            buf.push_str(&base[..pos]);
                        }
                        buf.push('\n');
                        base = &base[pos + 1..];
                    }
                    None => break,
                }
            }
            if first {
                buf.push_str(base);
            } else {
                buf.push_str(&cont);
                buf.push_str(base);
            }
            buf.push('\n');
        } else {
            // foldmultilines: '\n' -> "\n" escape sequence text, '\\' -> "\\\\".
            let mut cur = msg;
            loop {
                match cur.find(['\n', '\\']) {
                    Some(pos) => {
                        buf.push_str(&cur[..pos]);
                        if cur.as_bytes()[pos] == b'\n' {
                            buf.push_str("\\n");
                        } else {
                            buf.push_str("\\\\");
                        }
                        cur = &cur[pos + 1..];
                    }
                    None => break,
                }
            }
            buf.push_str(cur);
            buf.push('\n');
        }

        let n = match self.write_all(&mut state, buf.as_bytes()) {
            Ok(n) => n,
            Err(e) => {
                if !state.write_error_shown {
                    eprintln!("Could not write log: {} ({})", self.name, e);
                    state.write_error_shown = true;
                }
                return Err(e);
            }
        };
        if n > 0 {
            if let Sink::File(rot) = &mut state.sink {
                rot.current_size += n as u64;
                if rot.should_rotate() {
                    rot.rotate()?;
                }
            }
        }
        Ok(n)
    }

    fn flush(&self) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        match &mut state.sink {
            Sink::Stdout => io::stdout().flush(),
            Sink::Stderr => io::stderr().flush(),
            Sink::File(rot) => rot.file.flush(),
            Sink::Failed => Ok(()),
        }
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn set_option(&self, key: &str, value: &str) -> bool {
        if key.eq_ignore_ascii_case("foldmultilines") {
            if value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("false") {
                self.state.lock().unwrap().fold_multilines =
                    value.eq_ignore_ascii_case("true");
                return true;
            }
            return false;
        }
        false
    }

    fn describe(&self) -> String {
        let state = self.state.lock().unwrap();
        let mut s = format!("{} ", self.name);
        if let Sink::File(rot) = &state.sink {
            s.push_str(&format!(
                "filecount={},filesize={}K,foldmultilines={}",
                rot.file_count,
                rot.rotate_size / 1024,
                if state.fold_multilines { "true" } else { "false" }
            ));
        } else {
            s.push_str(&format!(
                "foldmultilines={}",
                if state.fold_multilines { "true" } else { "false" }
            ));
        }
        s
    }

    fn force_rotate(&self) {
        let mut state = self.state.lock().unwrap();
        if let Sink::File(rot) = &mut state.sink {
            let _ = rot.rotate();
        }
    }
}

/// LogFileOutput — rotating file sink (logFileOutput.cpp).
struct RotatingFile {
    file: File,
    file_name: PathBuf,
    file_count: u32,
    file_count_max_digits: u32,
    rotate_size: u64,
    current_size: u64,
    current_file: u32,
}

const DEFAULT_FILE_COUNT: u32 = 5;
const MAX_ROTATION_FILE_COUNT: u32 = 1000;
const DEFAULT_FILE_SIZE: u64 = 20 * 1024 * 1024;

impl RotatingFile {
    fn archive_name(&self, index: u32) -> PathBuf {
        let mut name = self.file_name.as_os_str().to_os_string();
        name.push(format!(".{:0width$}", index, width = self.file_count_max_digits as usize));
        PathBuf::from(name)
    }

    fn initialize(file_name: &str, options: &str) -> io::Result<(Self, bool)> {
        let path = PathBuf::from(file_name);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let mut file_count = DEFAULT_FILE_COUNT;
        let mut rotate_size = DEFAULT_FILE_SIZE;
        let mut fold_multilines = false;
        parse_output_options(options, &mut |key, value| {
            match key {
                "filecount" => {
                    if let Ok(v) = value.parse::<u32>() {
                        if v <= MAX_ROTATION_FILE_COUNT {
                            file_count = v;
                        }
                    }
                }
                "filesize" => {
                    if let Some(v) = parse_size_value(value) {
                        rotate_size = v;
                    }
                }
                "foldmultilines" => {
                    fold_multilines = value.eq_ignore_ascii_case("true");
                }
                _ => {}
            }
        });

        let file_count_max_digits = if file_count > 0 {
            digits(file_count - 1)
        } else {
            0
        };

        let file_exists = path.exists();
        let mut current_file = 0u32;
        if file_count > 0 && file_exists {
            if !path.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} is not a regular file", file_name),
                ));
            }
            current_file = next_file_number(&path, file_count_max_digits, file_count);
            archive(&path, current_file, file_count_max_digits);
            current_file = (current_file + 1) % file_count.max(1);
        }

        let mut open_opts = OpenOptions::new();
        open_opts.create(true).append(true);
        let file = open_opts.open(&path)?;

        if file_count == 0 && path.is_file() {
            // filecount=0: truncate the existing log file (ftruncate equivalent).
            file.set_len(0)?;
        }

        Ok((
            Self {
                file,
                file_name: path,
                file_count,
                file_count_max_digits,
                rotate_size,
                current_size: 0,
                current_file,
            },
            fold_multilines,
        ))
    }

    fn should_rotate(&self) -> bool {
        self.file_count > 0 && self.rotate_size > 0 && self.current_size >= self.rotate_size
    }

    fn increment_file_count(&mut self) {
        self.current_file += 1;
        if self.current_file == self.file_count {
            self.current_file = 0;
        }
    }

    fn archive(&self) {
        let target = self.archive_name(self.current_file);
        let _ = fs::remove_file(&target);
        let _ = fs::rename(&self.file_name, &target);
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;
        self.archive();
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file_name)?;
        self.current_size = 0;
        self.increment_file_count();
        Ok(())
    }
}

fn digits(n: u32) -> u32 {
    if n < 10 {
        1
    } else if n < 100 {
        2
    } else {
        3
    }
}

/// next_file_number — first unused archive index, or the oldest existing one.
fn next_file_number(path: &Path, digits: u32, file_count: u32) -> u32 {
    let mut oldest_idx = 0u32;
    let mut oldest_time = std::time::SystemTime::UNIX_EPOCH;
    let mut found = false;
    for i in 0..file_count {
        let mut name = path.as_os_str().to_os_string();
        name.push(format!(".{:0width$}", i, width = digits as usize));
        let candidate = PathBuf::from(name);
        if !candidate.exists() {
            return i;
        }
        if !found {
            oldest_time = fs::metadata(&candidate)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            oldest_idx = i;
            found = true;
        } else if let Ok(t) = fs::metadata(&candidate).and_then(|m| m.modified()) {
            if t < oldest_time {
                oldest_time = t;
                oldest_idx = i;
            }
        }
    }
    oldest_idx
}

fn archive(path: &Path, index: u32, digits: u32) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{:0width$}", index, width = digits as usize));
    let target = PathBuf::from(name);
    let _ = fs::remove_file(&target);
    let _ = fs::rename(path, &target);
    target
}

/// LogFileOutput::make_file_name — expand %p (pid), %t (vm start
/// timestamp), %hn (hostname); only the first occurrence of each is replaced.
fn expand_filename_placeholders(file_name: &str) -> String {
    if !file_name.contains("%p") && !file_name.contains("%t") && !file_name.contains("%hn") {
        return file_name.to_string();
    }
    let pid = std::process::id().to_string();
    let ts = vm_start_timestamp();
    let host = host_name_for_filename();
    let mut out = String::with_capacity(file_name.len() + 32);
    let mut rest = file_name;
    let mut replaced_p = false;
    let mut replaced_t = false;
    let mut replaced_h = false;
    while let Some(pos) = rest.find('%') {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        if !replaced_p && tail.starts_with("%p") {
            out.push_str(&pid);
            rest = &tail[2..];
            replaced_p = true;
        } else if !replaced_h && tail.starts_with("%hn") {
            out.push_str(&host);
            rest = &tail[3..];
            replaced_h = true;
        } else if !replaced_t && tail.starts_with("%t") {
            out.push_str(&ts);
            rest = &tail[2..];
            replaced_t = true;
        } else {
            out.push('%');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

fn vm_start_timestamp() -> String {
    static TS: OnceLock<String> = OnceLock::new();
    TS.get_or_init(|| {
        let ms = crate::share::runtime::os::Os::vm_startup_time_ms();
        let secs = ms.div_euclid(1000);
        let z = secs.div_euclid(86_400) + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        let sod = secs.rem_euclid(86_400);
        format!(
            "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
            y,
            m,
            d,
            sod / 3600,
            sod % 3600 / 60,
            sod % 60
        )
    })
    .clone()
}

fn host_name_for_filename() -> String {
    #[cfg(windows)]
    {
        crate::share::platform::windows::OsWindows::get_host_name()
            .unwrap_or_else(|| "unknown-host".to_string())
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .unwrap_or_else(|| "unknown-host".to_string())
    }
}

/// Arguments::atojulong-style size parse: decimal digits with optional
/// K/M/G suffix (case-insensitive).
fn parse_size_value(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mult) = if let Some(prefix) = s.strip_suffix(|c| c == 'k' || c == 'K') {
        (prefix, 1024u64)
    } else if let Some(prefix) = s.strip_suffix(|c| c == 'm' || c == 'M') {
        (prefix, 1024 * 1024)
    } else if let Some(prefix) = s.strip_suffix(|c| c == 'g' || c == 'G') {
        (prefix, 1024 * 1024 * 1024)
    } else {
        (s, 1u64)
    };
    let v: u64 = num.trim().parse().ok()?;
    v.checked_mul(mult)
}

fn parse_output_options(options: &str, f: &mut dyn FnMut(&str, &str)) {
    if options.is_empty() {
        return;
    }
    for opt in options.split(',') {
        let opt = opt.trim();
        if let Some((k, v)) = opt.split_once('=') {
            f(k.trim(), v.trim());
        }
    }
}

/// Output that discards everything.
pub struct NullOutput;

impl LogOutput for NullOutput {
    fn write_record(
        &self,
        _decorations: &LogDecorations,
        _decorators: &Decorations,
        _msg: &str,
    ) -> io::Result<usize> {
        Ok(0)
    }
    fn flush(&self) -> io::Result<()> {
        Ok(())
    }
    fn name(&self) -> &str {
        "null"
    }
    fn describe(&self) -> String {
        "null ".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::logging::logTag::LogTag;
    use crate::share::logging::logTagSet::LogTagSet;

    fn record(output: &dyn LogOutput, mask: &Decorations, msg: &str) -> usize {
        let ts = LogTagSet::new(&[LogTag::Gc]);
        let deco = LogDecorations::new(crate::share::logging::logLevel::LogLevel::Info, &ts, mask);
        output.write_record(&deco, mask, msg).unwrap()
    }

    #[test]
    fn null_output_drops() {
        let n = record(&NullOutput, &Decorations::default(), "hello");
        assert_eq!(n, 0);
    }

    #[test]
    fn file_output_writes_and_rotates() {
        let dir = std::env::temp_dir().join(format!("jvm25-log-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.log");
        let spec = format!("file={}", path.display());
        let out = StreamLogOutput::file(&spec, "filesize=1k,filecount=3");
        for i in 0..200 {
            record(&out, &Decorations::parse("none").unwrap(), &format!("line {} {}", i, "x".repeat(40)));
        }
        out.flush().unwrap();
        assert!(path.exists());
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("line 199"));
        let archives: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("test.log."))
            .collect();
        assert!(!archives.is_empty(), "expected rotated archive files");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_output_fold_multilines() {
        let dir = std::env::temp_dir().join(format!("jvm25-log-fold-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fold.log");
        let spec = format!("file={}", path.display());
        let out = StreamLogOutput::file(&spec, "foldmultilines=true");
        record(&out, &Decorations::parse("none").unwrap(), "a\nb\\c");
        out.flush().unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("a\\nb\\\\c"), "got: {:?}", content);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn filename_placeholders() {
        assert_eq!(expand_filename_placeholders("plain.log"), "plain.log");
        let p = expand_filename_placeholders("hotspot-%p.log");
        assert!(p.contains(&std::process::id().to_string()));
        assert!(p.ends_with(".log"));
        let t = expand_filename_placeholders("x-%t.log");
        assert!(t.contains('-') && t.ends_with(".log"));
        let h = expand_filename_placeholders("x-%hn.log");
        assert!(!h.contains("%hn"));
    }

    #[test]
    fn parse_size_suffixes() {
        assert_eq!(parse_size_value("1024"), Some(1024));
        assert_eq!(parse_size_value("1k"), Some(1024));
        assert_eq!(parse_size_value("2M"), Some(2 * 1024 * 1024));
        assert_eq!(parse_size_value("1g"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_size_value("abc"), None);
    }
}
