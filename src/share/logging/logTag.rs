/*
 * Port of OpenJDK 25 HotSpot LogTag (share/logging/logTag.hpp/.cpp)
 *
 * Log tags classify log messages. The enum mirrors LOG_TAG_LIST from
 * logTag.hpp (product tags only; NOT_PRODUCT/DEBUG_ONLY tags omitted).
 * "all" is a pseudo-tag used only in -Xlog selections, never in log calls.
 */

use super::logLevel::string_utils_similarity;

/// Maximum number of tags in one log tag set (LogTag::MaxTags).
pub const MAX_TAGS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u16)]
pub enum LogTag {
    /// __NO_TAG sentinel — never emitted by from_string.
    NoTag = 0,
    Add,
    Age,
    Alloc,
    Annotation,
    Aot,
    Arguments,
    Array,
    Attach,
    Barrier,
    Blocks,
    Bot,
    Breakpoint,
    Bytecode,
    Cause,
    Cds,
    Census,
    Class,
    Classhisto,
    Cleanup,
    Codecache,
    Compaction,
    Compilation,
    Condy,
    Constantpool,
    Constraints,
    Container,
    Continuations,
    Coops,
    Cpu,
    Cset,
    Data,
    Datacreation,
    Dcmd,
    Decoder,
    Defaultmethods,
    Deoptimization,
    Dependencies,
    Director,
    Dump,
    Dynamic,
    Ergo,
    Event,
    Exceptions,
    Exit,
    Fastlock,
    Finalizer,
    Fingerprint,
    Free,
    Freelist,
    Gc,
    Handshake,
    Hashtables,
    Heap,
    Heapdump,
    Humongous,
    Ihop,
    Ikclass,
    Indy,
    Init,
    Inlinecache,
    Inlining,
    Install,
    Interpreter,
    Itables,
    Jfr,
    Jit,
    Jmethod,
    Jni,
    Jvmci,
    Jvmti,
    Lambda,
    Library,
    Link,
    Liveness,
    Load,
    Loader,
    Logging,
    Malloc,
    Map,
    Mark,
    Marking,
    Membername,
    Memops,
    Metadata,
    Metaspace,
    Methodcomparator,
    Methodhandles,
    Methodtrace,
    Mirror,
    Mmu,
    Module,
    Monitorinflation,
    Monitormismatch,
    Monitortable,
    Native,
    Nestmates,
    Nmethod,
    Nmt,
    Normalize,
    Numa,
    Objecttagging,
    Obsolete,
    Oldobject,
    Oom,
    Oopmap,
    Oops,
    Oopstorage,
    Os,
    Owner,
    Page,
    Pagesize,
    Parser,
    Patch,
    Path,
    Perf,
    Periodic,
    Phases,
    Plab,
    Placeholders,
    Preempt,
    Preorder,
    Preview,
    Promotion,
    Ptrqueue,
    Purge,
    Record,
    Redefine,
    Ref,
    Refine,
    Region,
    Reloc,
    Remset,
    Resolve,
    Safepoint,
    Sampling,
    Scavenge,
    Sealed,
    Setting,
    Smr,
    Stackbarrier,
    Stackmap,
    Stacktrace,
    Stackwalk,
    Start,
    Startup,
    Startuptime,
    State,
    Stats,
    Streaming,
    Stringdedup,
    Stringtable,
    Stubs,
    Subclass,
    Survivor,
    Suspend,
    Sweep,
    Symboltable,
    System,
    Table,
    Task,
    Thread,
    Throttle,
    Timer,
    Tlab,
    Tracking,
    Training,
    Trimnative,
    Unload,
    Unmap,
    Unshareable,
    Update,
    Valuebasedclasses,
    Verification,
    Verify,
    Vmmutex,
    Vmoperation,
    Vmthread,
    Vtables,
    Vtablestubs,
    /// Pseudo-tag "all" — valid in selections only.
    All = u16::MAX,
}

/// Tag names in enum declaration order (index 0 is the empty no-tag name).
static TAG_NAMES: &[&str] = &[
    "",
    "add", "age", "alloc", "annotation", "aot", "arguments", "array", "attach",
    "barrier", "blocks", "bot", "breakpoint", "bytecode", "cause", "cds",
    "census", "class", "classhisto", "cleanup", "codecache", "compaction",
    "compilation", "condy", "constantpool", "constraints", "container",
    "continuations", "coops", "cpu", "cset", "data", "datacreation", "dcmd",
    "decoder", "defaultmethods", "deoptimization", "dependencies", "director",
    "dump", "dynamic", "ergo", "event", "exceptions", "exit", "fastlock",
    "finalizer", "fingerprint", "free", "freelist", "gc", "handshake",
    "hashtables", "heap", "heapdump", "humongous", "ihop", "iklass", "indy",
    "init", "inlinecache", "inlining", "install", "interpreter", "itables",
    "jfr", "jit", "jmethod", "jni", "jvmci", "jvmti", "lambda", "library",
    "link", "liveness", "load", "loader", "logging", "malloc", "map", "mark",
    "marking", "membername", "memops", "metadata", "metaspace",
    "methodcomparator", "methodhandles", "methodtrace", "mirror", "mmu",
    "module", "monitorinflation", "monitormismatch", "monitortable", "native",
    "nestmates", "nmethod", "nmt", "normalize", "numa", "objecttagging",
    "obsolete", "oldobject", "oom", "oopmap", "oops", "oopstorage", "os",
    "owner", "page", "pagesize", "parser", "patch", "path", "perf", "periodic",
    "phases", "plab", "placeholders", "preempt", "preorder", "preview",
    "promotion", "ptrqueue", "purge", "record", "redefine", "ref", "refine",
    "region", "reloc", "remset", "resolve", "safepoint", "sampling",
    "scavenge", "sealed", "setting", "smr", "stackbarrier", "stackmap",
    "stacktrace", "stackwalk", "start", "startup", "startuptime", "state",
    "stats", "streaming", "stringdedup", "stringtable", "stubs", "subclass",
    "survivor", "suspend", "sweep", "symboltable", "system", "table", "task",
    "thread", "throttle", "timer", "tlab", "tracking", "training", "trimnative",
    "unload", "unmap", "unshareable", "update", "valuebasedclasses",
    "verification", "verify", "vmmutex", "vmoperation", "vmthread", "vtables",
    "vtablestubs",
];

impl LogTag {
    /// LogTag::name — printable tag name; "" for NoTag.
    pub fn name(self) -> &'static str {
        if self == Self::All {
            return "all";
        }
        let idx = self as usize;
        if idx < TAG_NAMES.len() {
            TAG_NAMES[idx]
        } else {
            ""
        }
    }

    /// LogTag::from_string — case-insensitive exact match.
    /// Returns None for unknown names and for the "all" pseudo-tag is
    /// returned as Some(All) (handled by callers that accept it).
    pub fn from_string(s: &str) -> Option<Self> {
        if s.eq_ignore_ascii_case("all") {
            return Some(Self::All);
        }
        Self::all_tags().iter().copied().find(|t| t.name().eq_ignore_ascii_case(s))
    }

    /// LogTag::from_prefix — used for `tag*` selectors: matches the first
    /// real tag whose name starts with `prefix` (case-insensitive).
    /// Never matches NoTag or the "all" pseudo-tag.
    pub fn from_prefix(prefix: &str) -> Option<Self> {
        Self::all_tags()
            .iter()
            .copied()
            .find(|t| t.name().len() >= prefix.len() && t.name()[..prefix.len()].eq_ignore_ascii_case(prefix))
    }

    /// LogTag::fuzzy_match — Dice bigram similarity, threshold 0.5.
    /// Never matches the "all" pseudo-tag or NoTag.
    pub fn fuzzy_match(s: &str) -> Option<Self> {
        let mut best: Option<Self> = None;
        let mut best_score = 0.5f64;
        for tag in Self::all_tags() {
            let score = string_utils_similarity(s, tag.name());
            if score >= best_score {
                best = Some(*tag);
                best_score = score;
            }
        }
        best
    }

    /// All real tags in declaration order (excludes NoTag and All).
    pub fn all_tags() -> &'static [LogTag] {
        static TAGS: &[LogTag] = &[
            LogTag::Add, LogTag::Age, LogTag::Alloc, LogTag::Annotation,
            LogTag::Aot, LogTag::Arguments, LogTag::Array, LogTag::Attach,
            LogTag::Barrier, LogTag::Blocks, LogTag::Bot, LogTag::Breakpoint,
            LogTag::Bytecode, LogTag::Cause, LogTag::Cds, LogTag::Census,
            LogTag::Class, LogTag::Classhisto, LogTag::Cleanup, LogTag::Codecache,
            LogTag::Compaction, LogTag::Compilation, LogTag::Condy,
            LogTag::Constantpool, LogTag::Constraints, LogTag::Container,
            LogTag::Continuations, LogTag::Coops, LogTag::Cpu, LogTag::Cset,
            LogTag::Data, LogTag::Datacreation, LogTag::Dcmd, LogTag::Decoder,
            LogTag::Defaultmethods, LogTag::Deoptimization, LogTag::Dependencies,
            LogTag::Director, LogTag::Dump, LogTag::Dynamic, LogTag::Ergo,
            LogTag::Event, LogTag::Exceptions, LogTag::Exit, LogTag::Fastlock,
            LogTag::Finalizer, LogTag::Fingerprint, LogTag::Free, LogTag::Freelist,
            LogTag::Gc, LogTag::Handshake, LogTag::Hashtables, LogTag::Heap,
            LogTag::Heapdump, LogTag::Humongous, LogTag::Ihop, LogTag::Ikclass,
            LogTag::Indy, LogTag::Init, LogTag::Inlinecache, LogTag::Inlining,
            LogTag::Install, LogTag::Interpreter, LogTag::Itables, LogTag::Jfr,
            LogTag::Jit, LogTag::Jmethod, LogTag::Jni, LogTag::Jvmci, LogTag::Jvmti,
            LogTag::Lambda, LogTag::Library, LogTag::Link, LogTag::Liveness,
            LogTag::Load, LogTag::Loader, LogTag::Logging, LogTag::Malloc,
            LogTag::Map, LogTag::Mark, LogTag::Marking, LogTag::Membername,
            LogTag::Memops, LogTag::Metadata, LogTag::Metaspace,
            LogTag::Methodcomparator, LogTag::Methodhandles, LogTag::Methodtrace,
            LogTag::Mirror, LogTag::Mmu, LogTag::Module, LogTag::Monitorinflation,
            LogTag::Monitormismatch, LogTag::Monitortable, LogTag::Native,
            LogTag::Nestmates, LogTag::Nmethod, LogTag::Nmt, LogTag::Normalize,
            LogTag::Numa, LogTag::Objecttagging, LogTag::Obsolete, LogTag::Oldobject,
            LogTag::Oom, LogTag::Oopmap, LogTag::Oops, LogTag::Oopstorage,
            LogTag::Os, LogTag::Owner, LogTag::Page, LogTag::Pagesize,
            LogTag::Parser, LogTag::Patch, LogTag::Path, LogTag::Perf,
            LogTag::Periodic, LogTag::Phases, LogTag::Plab, LogTag::Placeholders,
            LogTag::Preempt, LogTag::Preorder, LogTag::Preview, LogTag::Promotion,
            LogTag::Ptrqueue, LogTag::Purge, LogTag::Record, LogTag::Redefine,
            LogTag::Ref, LogTag::Refine, LogTag::Region, LogTag::Reloc,
            LogTag::Remset, LogTag::Resolve, LogTag::Safepoint, LogTag::Sampling,
            LogTag::Scavenge, LogTag::Sealed, LogTag::Setting, LogTag::Smr,
            LogTag::Stackbarrier, LogTag::Stackmap, LogTag::Stacktrace,
            LogTag::Stackwalk, LogTag::Start, LogTag::Startup, LogTag::Startuptime,
            LogTag::State, LogTag::Stats, LogTag::Streaming, LogTag::Stringdedup,
            LogTag::Stringtable, LogTag::Stubs, LogTag::Subclass, LogTag::Survivor,
            LogTag::Suspend, LogTag::Sweep, LogTag::Symboltable, LogTag::System,
            LogTag::Table, LogTag::Task, LogTag::Thread, LogTag::Throttle,
            LogTag::Timer, LogTag::Tlab, LogTag::Tracking, LogTag::Training,
            LogTag::Trimnative, LogTag::Unload, LogTag::Unmap, LogTag::Unshareable,
            LogTag::Update, LogTag::Valuebasedclasses, LogTag::Verification,
            LogTag::Verify, LogTag::Vmmutex, LogTag::Vmoperation, LogTag::Vmthread,
            LogTag::Vtables, LogTag::Vtablestubs,
        ];
        TAGS
    }
}

/// Convenience aliases mirroring the pre-port API names.
pub const TAG_ALL: LogTag = LogTag::All;
pub const TAG_NO_TAG: LogTag = LogTag::NoTag;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_names_match_list() {
        assert_eq!(LogTag::Gc.name(), "gc");
        assert_eq!(LogTag::Class.name(), "class");
        assert_eq!(LogTag::Load.name(), "load");
        assert_eq!(LogTag::Jit.name(), "jit");
        assert_eq!(LogTag::Exceptions.name(), "exceptions");
        assert_eq!(LogTag::Os.name(), "os");
        assert_eq!(LogTag::All.name(), "all");
        assert_eq!(LogTag::NoTag.name(), "");
    }

    #[test]
    fn tag_names_aligned_with_variants() {
        let tags = LogTag::all_tags();
        assert_eq!(TAG_NAMES.len(), tags.len() + 1);
        for (i, tag) in tags.iter().enumerate() {
            assert_eq!(tag.name(), TAG_NAMES[i + 1], "misaligned at {}", i);
            assert_ne!(tag.name(), "");
        }
        let mut sorted = tags.to_vec();
        sorted.sort_by_key(|t| t.name());
        sorted.dedup();
        assert_eq!(sorted.len(), tags.len(), "duplicate tag names");
    }

    #[test]
    fn from_string_roundtrip() {
        for tag in LogTag::all_tags() {
            assert_eq!(LogTag::from_string(tag.name()), Some(*tag));
            assert_eq!(LogTag::from_string(&tag.name().to_uppercase()), Some(*tag));
        }
        assert_eq!(LogTag::from_string("all"), Some(LogTag::All));
        assert_eq!(LogTag::from_string("nonexistent"), None);
        assert_eq!(LogTag::from_string(""), None);
    }

    #[test]
    fn fuzzy_match_finds_similar() {
        assert_eq!(LogTag::fuzzy_match("clas"), Some(LogTag::Class));
        assert_eq!(LogTag::fuzzy_match("heaps"), Some(LogTag::Heap));
        assert_eq!(LogTag::fuzzy_match("gcs"), None);
        assert_eq!(LogTag::fuzzy_match("xyzzy"), None);
    }
}
