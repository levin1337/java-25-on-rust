/*
 * Port of OpenJDK 25 HotSpot LogTag (share/logging/logTag.hpp)
 *
 * Defines Unified JVM Logging tags (gc, class, load, unload, thread, jit, init, heap, etc.).
 */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogTag {
    All,
    Gc,
    Class,
    Load,
    Unload,
    Thread,
    Jit,
    Init,
    Heap,
    Monitor,
    Os,
    Safepoint,
}

impl LogTag {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "all" => Some(Self::All),
            "gc" => Some(Self::Gc),
            "class" => Some(Self::Class),
            "load" => Some(Self::Load),
            "unload" => Some(Self::Unload),
            "thread" => Some(Self::Thread),
            "jit" | "compiler" => Some(Self::Jit),
            "init" => Some(Self::Init),
            "heap" => Some(Self::Heap),
            "monitor" => Some(Self::Monitor),
            "os" => Some(Self::Os),
            "safepoint" => Some(Self::Safepoint),
            _ => None,
        }
    }

    /// Prefix match for wildcard selectors like "class*" (logSelection.cpp).
    /// Returns the tag whose name starts with `prefix`, or All for "*".
    pub fn from_prefix(prefix: &str) -> Option<Self> {
        let p = prefix.to_ascii_lowercase();
        if p.is_empty() {
            return Some(Self::All);
        }
        // Exact match first.
        if let Some(t) = Self::from_str(&p) {
            return Some(t);
        }
        // Otherwise match any tag whose name starts with the prefix.
        const ALL: &[LogTag] = &[
            LogTag::Gc, LogTag::Class, LogTag::Load, LogTag::Unload,
            LogTag::Thread, LogTag::Jit, LogTag::Init, LogTag::Heap,
            LogTag::Monitor, LogTag::Os, LogTag::Safepoint,
        ];
        ALL.iter().copied().find(|t| t.name().starts_with(p.as_str()))
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Gc => "gc",
            Self::Class => "class",
            Self::Load => "load",
            Self::Unload => "unload",
            Self::Thread => "thread",
            Self::Jit => "jit",
            Self::Init => "init",
            Self::Heap => "heap",
            Self::Monitor => "monitor",
            Self::Os => "os",
            Self::Safepoint => "safepoint",
        }
    }
}
