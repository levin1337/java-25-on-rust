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
