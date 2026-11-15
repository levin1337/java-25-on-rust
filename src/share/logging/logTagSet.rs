/*
 * Port of OpenJDK 25 HotSpot LogTagSet (share/logging/logTagSet.hpp/.cpp)
 *
 * A LogTagSet is a combination of up to LogTag::MAX_TAGS tags that occurs
 * in a log call. Tag sets are interned in a global registry: identical tag
 * combinations (same tags, same order) share one LogTagSet, mirroring the
 * per-instantiation LogTagSetMapping statics of the C++ code.
 */

use super::logTag::{LogTag, MAX_TAGS};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Described tag sets shown by -Xlog:help / VM.log list
/// (logTagSetDescriptions.cpp).
pub const TAGSET_DESCRIPTIONS: &[(&[LogTag], &str)] = &[(
    &[LogTag::Logging],
    "Logging for the log framework itself",
)];

#[derive(Debug, Clone)]
pub struct LogTagSet {
    id: u32,
    tags: Vec<LogTag>,
}

struct Registry {
    map: HashMap<Vec<LogTag>, u32>,
    sets: Vec<LogTagSet>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        Mutex::new(Registry {
            map: HashMap::new(),
            sets: Vec::new(),
        })
    })
}

impl LogTagSet {
    /// LogTagSet constructor equivalent: interns the tag combination.
    /// Tags beyond MAX_TAGS are rejected by returning None via try_new;
    /// new() truncates defensively (callers should use try_new).
    pub fn try_new(tags: &[LogTag]) -> Option<Self> {
        if tags.is_empty() || tags.len() > MAX_TAGS {
            return None;
        }
        if tags.iter().any(|t| *t == LogTag::NoTag || *t == LogTag::All) {
            return None;
        }
        let mut reg = registry().lock().unwrap();
        if let Some(&id) = reg.map.get(tags) {
            return Some(reg.sets[id as usize].clone());
        }
        let id = reg.sets.len() as u32;
        let set = LogTagSet {
            id,
            tags: tags.to_vec(),
        };
        reg.map.insert(tags.to_vec(), id);
        reg.sets.push(set.clone());
        Some(set)
    }

    /// Convenience for a single-tag set; panics only on invalid input.
    pub fn new(tags: &[LogTag]) -> Self {
        Self::try_new(tags).expect("invalid log tag set")
    }

    pub fn for_tag(tag: LogTag) -> Self {
        Self::new(&[tag])
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn ntags(&self) -> usize {
        self.tags.len()
    }

    pub fn tag(&self, idx: usize) -> LogTag {
        self.tags[idx]
    }

    pub fn tags(&self) -> &[LogTag] {
        &self.tags
    }

    /// LogTagSet::contains — exact tag membership.
    pub fn contains(&self, tag: LogTag) -> bool {
        self.tags.contains(&tag)
    }

    /// LogTagSet::label — "gc+heap" style label.
    pub fn label(&self) -> String {
        self.label_with("+")
    }

    pub fn label_with(&self, separator: &str) -> String {
        self.tags
            .iter()
            .map(|t| t.name())
            .collect::<Vec<_>>()
            .join(separator)
    }

    /// All registered tag sets (LogTagSet::first/next traversal equivalent).
    pub fn all() -> Vec<LogTagSet> {
        registry().lock().unwrap().sets.clone()
    }

    pub fn count() -> usize {
        registry().lock().unwrap().sets.len()
    }

    /// LogTagSet::describe_tagsets — described tag sets listing.
    pub fn describe_tagsets() -> String {
        let mut out = String::from("Described tag sets:\n");
        for (tags, descr) in TAGSET_DESCRIPTIONS {
            if let Some(ts) = Self::try_new(tags) {
                out.push(' ');
                out.push_str(&ts.label_with("+"));
                out.push_str(": ");
                out.push_str(descr);
                out.push('\n');
            }
        }
        out
    }

    /// LogTagSet::list_all_tagsets — sorted list of registered tag sets.
    pub fn list_all() -> String {
        let mut labels: Vec<String> = Self::all().iter().map(|ts| ts.label()).collect();
        labels.sort();
        format!("Available tag sets: {}\n", labels.join(", "))
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagset_interning() {
        let a = LogTagSet::new(&[LogTag::Gc, LogTag::Heap]);
        let b = LogTagSet::new(&[LogTag::Gc, LogTag::Heap]);
        let c = LogTagSet::new(&[LogTag::Heap, LogTag::Gc]);
        assert_eq!(a.id(), b.id());
        assert_ne!(a.id(), c.id());
        assert_eq!(a.label(), "gc+heap");
        assert_eq!(c.label(), "heap+gc");
    }

    #[test]
    fn tagset_contains_and_label() {
        let ts = LogTagSet::new(&[LogTag::Gc, LogTag::Ref, LogTag::Init]);
        assert!(ts.contains(LogTag::Gc));
        assert!(!ts.contains(LogTag::Os));
        assert_eq!(ts.ntags(), 3);
        assert_eq!(ts.tag(0), LogTag::Gc);
        assert_eq!(ts.label_with(","), "gc,ref,init");
    }

    #[test]
    fn tagset_rejects_invalid() {
        assert!(LogTagSet::try_new(&[]).is_none());
        assert!(LogTagSet::try_new(&[LogTag::All]).is_none());
        assert!(LogTagSet::try_new(&[LogTag::NoTag]).is_none());
        let six = [LogTag::Gc, LogTag::Heap, LogTag::Ref, LogTag::Init, LogTag::Os, LogTag::Jit];
        assert!(LogTagSet::try_new(&six).is_none());
    }

    #[test]
    fn described_tagsets_listed() {
        let s = LogTagSet::describe_tagsets();
        assert!(s.contains("logging"));
        assert!(s.contains("log framework"));
        let l = LogTagSet::list_all();
        assert!(l.contains("gc+heap") || l.contains("logging"));
    }
}
