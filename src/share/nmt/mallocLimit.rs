/*
 * Port of OpenJDK 25 HotSpot mallocLimit (share/nmt/mallocLimit.hpp/.cpp)
 *
 * -XX:MallocLimit=<size>[:oom|fatal]            — global malloc limit
 * -XX:MallocLimit=<tag>:<size>[:flag][,...]     — per-category limits
 *
 * When NMT is enabled, exceeding a limit reports (fatal or OOM warning)
 * via MallocMemorySummary::check_exceeds_limit.
 */

use super::memTag::MemTag;
use super::nmtCommon::NmtUtil;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MallocLimitMode {
    TriggerFatal = 0,
    TriggerOom = 1,
}

impl MallocLimitMode {
    fn name(self) -> &'static str {
        match self {
            Self::TriggerFatal => "fatal",
            Self::TriggerOom => "oom",
        }
    }
}

/// One limit entry: size 0 means "no limit".
#[derive(Debug, Clone, Copy)]
pub struct MallocLimit {
    pub sz: usize,
    pub mode: MallocLimitMode,
}

impl Default for MallocLimit {
    fn default() -> Self {
        Self { sz: 0, mode: MallocLimitMode::TriggerFatal }
    }
}

/// MallocLimitSet — global limit plus per-tag category limits.
pub struct MallocLimitSet {
    glob: MallocLimit,
    cat: Vec<MallocLimit>,
}

impl MallocLimitSet {
    pub fn new() -> Self {
        let mut set = Self {
            glob: MallocLimit::default(),
            cat: vec![MallocLimit::default(); MemTag::MtNumberOfTags as usize],
        };
        set.reset();
        set
    }

    pub fn reset(&mut self) {
        self.glob = MallocLimit::default();
        for c in self.cat.iter_mut() {
            *c = MallocLimit::default();
        }
    }

    #[inline]
    pub fn set_global_limit(&mut self, s: usize, mode: MallocLimitMode) {
        self.glob.sz = s;
        self.glob.mode = mode;
    }

    #[inline]
    pub fn set_category_limit(&mut self, mem_tag: MemTag, s: usize, mode: MallocLimitMode) {
        self.cat[NmtUtil::tag_to_index(mem_tag)] = MallocLimit { sz: s, mode };
    }

    #[inline]
    pub fn global_limit(&self) -> &MallocLimit {
        &self.glob
    }

    #[inline]
    pub fn category_limit(&self, mem_tag: MemTag) -> &MallocLimit {
        &self.cat[NmtUtil::tag_to_index(mem_tag)]
    }

    /// Parse an option string:
    ///   MallocLimit=<size>[:flag]
    ///   MallocLimit=<category>:<size>[:flag][,<category>:<size>[:flag]...]
    pub fn parse_malloclimit_option(&mut self, optionstring: &str) -> Result<(), String> {
        self.reset();
        let s = optionstring.trim();
        if s.is_empty() {
            return Err("Empty string".to_string());
        }

        // Global form: <size>[:flag] — try it first. It applies only when
        // the part before the first ':' is a plain size and what follows
        // (if anything) is a mode flag.
        let (head, tail) = match s.find(':') {
            Some(i) => (&s[..i], Some(&s[i + 1..])),
            None => (s, None),
        };
        if let Some(sz) = parse_size(head) {
            match tail {
                None => {
                    self.glob.sz = sz;
                    return Ok(());
                }
                Some(flag) if parse_mode(flag).is_some() => {
                    self.glob.sz = sz;
                    self.glob.mode = parse_mode(flag).unwrap();
                    return Ok(());
                }
                _ => {}
            }
        }

        // Category-specific form: comma-separated <cat>:<size>[:flag].
        for part in s.split(',') {
            let part = part.trim();
            if part.is_empty() {
                return Err("Expected category name".to_string());
            }
            // Split off the optional :flag suffix — but flag only follows
            // the size, so find ':' separators carefully:
            //   <cat>:<size>  or  <cat>:<size>:<flag>
            let first_colon = part.find(':').ok_or("Expected colon")?;
            let cat_str = &part[..first_colon];
            let mem_tag = NmtUtil::string_to_mem_tag(cat_str);
            if mem_tag == MemTag::MtNone {
                return Err(format!("Expected category name, got '{}'", cat_str));
            }
            let rest = &part[first_colon + 1..];
            // rest = <size> or <size>:<flag>
            let (size_str, flag_str) = match rest.find(':') {
                Some(i) => (&rest[..i], Some(&rest[i + 1..])),
                None => (rest, None),
            };
            let sz = parse_size(size_str).ok_or("Expected size")?;
            let mut mode = MallocLimitMode::TriggerFatal;
            if let Some(f) = flag_str {
                mode = parse_mode(f).ok_or("Expected flag")?;
            }
            self.set_category_limit(mem_tag, sz, mode);
        }
        Ok(())
    }

    /// Render like C++ print_on.
    pub fn print_on(&self) -> String {
        let mut out = String::new();
        if self.glob.sz > 0 {
            out.push_str(&format!(
                "MallocLimit: total limit: {} ({})\n",
                self.glob.sz,
                self.glob.mode.name()
            ));
        } else {
            for i in 0..MemTag::MtNumberOfTags as usize {
                let l = &self.cat[i];
                if l.sz > 0 {
                    out.push_str(&format!(
                        "MallocLimit: category \"{}\" limit: {} ({})\n",
                        NmtUtil::tag_to_enum_name(NmtUtil::index_to_tag(i)),
                        l.sz,
                        l.mode.name()
                    ));
                }
            }
        }
        out
    }
}

impl Default for MallocLimitSet {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_mode(s: &str) -> Option<MallocLimitMode> {
    if s.eq_ignore_ascii_case("oom") {
        Some(MallocLimitMode::TriggerOom)
    } else if s.eq_ignore_ascii_case("fatal") {
        Some(MallocLimitMode::TriggerFatal)
    } else {
        None
    }
}

/// Parse a memory size with optional suffix (k/m/g, case-insensitive),
/// like parse_integer<size_t> in utilities/parseInteger.hpp.
fn parse_size(s: &str) -> Option<usize> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (num_part, mult) = match s.chars().last() {
        Some(c) if c.is_ascii_alphabetic() => {
            let mult = match c.to_ascii_lowercase() {
                'k' => 1024usize,
                'm' => 1024 * 1024,
                'g' => 1024 * 1024 * 1024,
                'b' => 1usize,
                _ => return None,
            };
            (&s[..s.len() - 1], mult)
        }
        _ => (s, 1usize),
    };
    let v: usize = num_part.parse().ok()?;
    v.checked_mul(mult)
}

/// MallocLimitHandler — process-wide limit set.
pub struct MallocLimitHandler;

static LIMITS: OnceLock<Mutex<MallocLimitSet>> = OnceLock::new();
static HAVE_LIMIT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl MallocLimitHandler {
    fn limits() -> &'static Mutex<MallocLimitSet> {
        LIMITS.get_or_init(|| Mutex::new(MallocLimitSet::new()))
    }

    pub fn have_limit() -> bool {
        HAVE_LIMIT.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn global_limit() -> Option<MallocLimit> {
        if !Self::have_limit() {
            return None;
        }
        let limits = Self::limits().lock().unwrap();
        let l = limits.global_limit();
        if l.sz > 0 { Some(*l) } else { None }
    }

    pub fn category_limit(mem_tag: MemTag) -> Option<MallocLimit> {
        if !Self::have_limit() {
            return None;
        }
        let limits = Self::limits().lock().unwrap();
        let l = limits.category_limit(mem_tag);
        if l.sz > 0 { Some(*l) } else { None }
    }

    /// Initialize from the -XX:MallocLimit option string (None/empty = unset).
    /// Returns Err with the parse error message on failure.
    pub fn initialize(options: Option<&str>) -> Result<(), String> {
        HAVE_LIMIT.store(false, std::sync::atomic::Ordering::Relaxed);
        if let Some(opts) = options {
            if !opts.is_empty() {
                let mut set = Self::limits().lock().unwrap();
                set.parse_malloclimit_option(opts)?;
                HAVE_LIMIT.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        Ok(())
    }

    pub fn print_on() -> String {
        if Self::have_limit() {
            Self::limits().lock().unwrap().print_on()
        } else {
            "MallocLimit: unset\n".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_global() {
        let mut set = MallocLimitSet::new();
        assert!(set.parse_malloclimit_option("1g").is_ok());
        assert_eq!(set.global_limit().sz, 1024 * 1024 * 1024);
        assert!(set.parse_malloclimit_option("100m:oom").is_ok());
        assert_eq!(set.global_limit().sz, 100 * 1024 * 1024);
        assert_eq!(set.global_limit().mode, MallocLimitMode::TriggerOom);
    }

    #[test]
    fn parse_categories() {
        let mut set = MallocLimitSet::new();
        assert!(set.parse_malloclimit_option("Compiler:100m,GC:50m:oom").is_ok());
        assert_eq!(set.category_limit(MemTag::MtCompiler).sz, 100 * 1024 * 1024);
        assert_eq!(set.category_limit(MemTag::MtGC).sz, 50 * 1024 * 1024);
        assert_eq!(set.category_limit(MemTag::MtGC).mode, MallocLimitMode::TriggerOom);
        assert_eq!(set.category_limit(MemTag::MtSymbol).sz, 0);
        assert!(set.parse_malloclimit_option("Bogus:100m").is_err());
    }

    #[test]
    fn parse_sizes() {
        assert_eq!(parse_size("42"), Some(42));
        assert_eq!(parse_size("10k"), Some(10 * 1024));
        assert_eq!(parse_size("2G"), Some(2 * 1024 * 1024 * 1024));
        assert_eq!(parse_size(""), None);
        assert_eq!(parse_size("abc"), None);
    }
}
