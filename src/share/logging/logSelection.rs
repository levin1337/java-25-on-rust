/*
 * Port of OpenJDK 25 HotSpot LogSelection / LogSelectionList
 * (share/logging/logSelection.cpp, logSelectionList.cpp)
 *
 * A LogSelection is one comma-free "what" expression of the form
 * tag1[+tag2...][*][=level]. A LogSelectionList is the comma-separated
 * list of selections that configures one log output.
 */

use super::logLevel::LogLevel;
use super::logTag::{LogTag, MAX_TAGS};
use super::logTagSet::LogTagSet;

/// LogSelection — parsed single selection.
#[derive(Debug, Clone, PartialEq)]
pub struct LogSelection {
    pub tags: Vec<LogTag>,
    pub wildcard: bool,
    pub level: LogLevel,
}

fn parse_internal(s: &str, err: &mut String) -> Option<LogSelection> {
    // Split off the level part at the first '='.
    let (tag_part, level) = match s.find('=') {
        Some(pos) => {
            let level_str = &s[pos + 1..];
            let level = match LogLevel::from_string(level_str) {
                Some(l) => l,
                None => {
                    err.push_str(&format!("Invalid level '{}' in log selection.\n", level_str));
                    if let Some(m) = LogLevel::fuzzy_match(level_str) {
                        err.push_str(&format!(" Did you mean '{}'?\n", m.name()));
                    }
                    return None;
                }
            };
            (&s[..pos], level)
        }
        None => (s, LogLevel::UNSPECIFIED),
    };

    // Special tag "all": empty tag list with wildcard semantics.
    if tag_part == "all" {
        return Some(LogSelection {
            tags: Vec::new(),
            wildcard: true,
            level,
        });
    }

    // Trailing '*' makes the selection a prefix/wildcard match.
    let (tag_part, wildcard) = match tag_part.strip_suffix('*') {
        Some(prefix) => (prefix, true),
        None => (tag_part, false),
    };
    if tag_part.contains('*') {
        err.push_str(&format!("Invalid tag '{}' in log selection.\n", tag_part));
        return None;
    }

    let mut tags = Vec::new();
    for cur in tag_part.split('+') {
        let tag = match LogTag::from_string(cur) {
            Some(LogTag::All) | None => {
                err.push_str(&format!("Invalid tag '{}' in log selection.\n", cur));
                if let Some(m) = LogTag::fuzzy_match(cur) {
                    err.push_str(&format!(" Did you mean '{}'?\n", m.name()));
                }
                return None;
            }
            Some(t) => t,
        };
        if tags.len() == MAX_TAGS {
            err.push_str(&format!(
                "Too many tags in log selection '{}' (can only have up to {}).\n",
                tag_part, MAX_TAGS
            ));
            return None;
        }
        tags.push(tag);
    }

    // Duplicate tags are rejected.
    for i in 0..tags.len() {
        for j in 0..tags.len() {
            if i != j && tags[i] == tags[j] {
                err.push_str(&format!(
                    "Log selection contains duplicates of tag {}.\n",
                    tags[i].name()
                ));
                return None;
            }
        }
    }

    Some(LogSelection {
        tags,
        wildcard,
        level,
    })
}

impl LogSelection {
    /// LogSelection::parse — returns None on error with message in `err`.
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut err = String::new();
        match parse_internal(s.trim(), &mut err) {
            Some(sel) => Ok(sel),
            None => Err(err),
        }
    }

    /// LogSelection::selects — does this selection cover the tag set?
    /// Exact selections require identical tag count and full containment;
    /// wildcard selections require only containment.
    pub fn selects(&self, ts: &LogTagSet) -> bool {
        if !self.wildcard && self.tags.len() != ts.ntags() {
            return false;
        }
        self.tags.iter().all(|t| ts.contains(*t))
    }

    /// LogSelection::superset_of — all tags of `other` are in this selection.
    pub fn superset_of(&self, other: &LogSelection) -> bool {
        other.tags.iter().all(|t| self.tags.contains(t))
    }

    /// LogSelection::consists_of — tag set equality (order-insensitive).
    pub fn consists_of(&self, tags: &[LogTag]) -> bool {
        if tags.len() != self.tags.len() {
            return false;
        }
        tags.iter().all(|t| self.tags.contains(t))
    }

    pub fn ntags(&self) -> usize {
        self.tags.len()
    }

    /// LogSelection::describe_tags_on — "gc+heap*" style.
    pub fn describe_tags(&self) -> String {
        let mut s = self
            .tags
            .iter()
            .map(|t| t.name())
            .collect::<Vec<_>>()
            .join("+");
        if self.wildcard {
            s.push('*');
        }
        s
    }

    /// LogSelection::describe_on — "gc+heap=debug" style.
    pub fn describe(&self) -> String {
        format!("{}={}", self.describe_tags(), self.level.name())
    }

    /// LogSelection::similarity — Soerensen-Dice over tag intersection.
    pub fn similarity(&self, other: &LogSelection) -> f64 {
        let mut intersecting = 0usize;
        for t in &self.tags {
            if other.tags.contains(t) {
                intersecting += 1;
            }
        }
        let total = self.tags.len() + other.tags.len();
        if total == 0 {
            return 0.0;
        }
        2.0 * intersecting as f64 / total as f64
    }

    /// LogSelection::suggest_similar_matching — best-effort suggestions for
    /// selections that match no registered tag set.
    pub fn suggest_similar(&self) -> Vec<LogSelection> {
        const SUGGESTION_CAP: usize = 5;
        const SIMILARITY_REQUIREMENT: f64 = 0.3;
        let mut suggestions: Vec<LogSelection> = Vec::new();

        if !self.wildcard {
            let sel = LogSelection {
                tags: self.tags.clone(),
                wildcard: true,
                level: self.level,
            };
            if sel.tag_sets_selected() > 0 {
                suggestions.push(sel);
            }
        }

        for ts in LogTagSet::all() {
            let mut sel = LogSelection {
                tags: ts.tags().to_vec(),
                wildcard: true,
                level: self.level,
            };
            if sel.tag_sets_selected() == 1 {
                sel.wildcard = false;
            }
            let score = self.similarity(&sel);
            if score < SIMILARITY_REQUIREMENT {
                continue;
            }
            if suggestions.len() < SUGGESTION_CAP {
                suggestions.push(sel);
                continue;
            }
            let mut min_pos = 0usize;
            let mut min = f64::MAX;
            for (i, s) in suggestions.iter().enumerate() {
                let sc = self.similarity(s);
                if sc < min {
                    min = sc;
                    min_pos = i;
                }
            }
            if score > min {
                suggestions[min_pos] = sel;
            }
        }

        suggestions.sort_by(|a, b| {
            let sa = self.similarity(b);
            let sb = self.similarity(a);
            sa.partial_cmp(&sb)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.ntags().cmp(&b.ntags()))
                .then(b.tag_sets_selected().cmp(&a.tag_sets_selected()))
        });
        suggestions
    }

    /// Number of registered tag sets this selection matches.
    pub fn tag_sets_selected(&self) -> usize {
        LogTagSet::all()
            .iter()
            .filter(|ts| self.selects(ts))
            .count()
    }
}

/// LogSelectionList — comma-separated selections configuring one output.
#[derive(Debug, Clone, Default)]
pub struct LogSelectionList {
    pub selections: Vec<LogSelection>,
}

const DEFAULT_EXPRESSION: &str = "all";

impl LogSelectionList {
    /// LogSelectionList::parse — empty input means "all".
    pub fn parse(s: Option<&str>) -> Result<Self, String> {
        let s = match s {
            None | Some("") => DEFAULT_EXPRESSION,
            Some(v) => v,
        };
        let mut list = LogSelectionList::default();
        for part in s.split(',') {
            let sel = LogSelection::parse(part)?;
            list.selections.push(sel);
        }
        Ok(list)
    }

    /// LogSelectionList::level_for — level of the last matching selection,
    /// or None (NotMentioned) when no selection covers the tag set.
    pub fn level_for(&self, ts: &LogTagSet) -> Option<LogLevel> {
        let mut level = None;
        for sel in &self.selections {
            if sel.selects(ts) {
                level = Some(sel.level);
            }
        }
        level
    }

    /// LogSelectionList::verify_selections — error text for selections that
    /// match no registered tag set.
    pub fn verify_selections(&self) -> String {
        let mut out = String::new();
        for sel in &self.selections {
            if sel.tag_sets_selected() == 0 {
                out.push_str("No tag set matches selection: ");
                out.push_str(&sel.describe_tags());
                out.push_str(". ");
                let suggestions = sel.suggest_similar();
                if !suggestions.is_empty() {
                    out.push_str("Did you mean any of the following?");
                    for s in &suggestions {
                        out.push(' ');
                        out.push_str(&s.describe_tags());
                    }
                }
                out.push('\n');
            }
        }
        out
    }

    /// LogSelectionList::get_default_decorators — true when the selections
    /// do NOT disable the default decorators (i.e. defaults apply).
    /// Defaults are disabled only when every selection is a superset of
    /// the undecorated {jit,inlining} tag set (LogDecorators rule).
    pub fn has_default_decorators(&self) -> bool {
        for sel in &self.selections {
            if !Self::selection_disables_defaults(sel) {
                return true;
            }
        }
        false
    }

    fn selection_disables_defaults(sel: &LogSelection) -> bool {
        sel.tags.contains(&LogTag::Jit) && sel.tags.contains(&LogTag::Inlining)
    }

    /// Convenience: selections describing the effective config string.
    pub fn describe(&self) -> String {
        self.selections
            .iter()
            .map(|s| s.describe())
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(tags: &[LogTag]) -> LogTagSet {
        LogTagSet::new(tags)
    }

    #[test]
    fn parse_basic_selections() {
        let sel = LogSelection::parse("gc+heap=debug").unwrap();
        assert_eq!(sel.tags, vec![LogTag::Gc, LogTag::Heap]);
        assert!(!sel.wildcard);
        assert_eq!(sel.level, LogLevel::Debug);

        let sel = LogSelection::parse("class*").unwrap();
        assert_eq!(sel.tags, vec![LogTag::Class]);
        assert!(sel.wildcard);
        assert_eq!(sel.level, LogLevel::Info);

        let sel = LogSelection::parse("all=off").unwrap();
        assert!(sel.tags.is_empty());
        assert!(sel.wildcard);
        assert_eq!(sel.level, LogLevel::Off);

        let sel = LogSelection::parse("jit+inlining=trace").unwrap();
        assert_eq!(sel.level, LogLevel::Trace);
    }

    #[test]
    fn parse_rejects_invalid() {
        assert!(LogSelection::parse("gc=verbose").is_err());
        assert!(LogSelection::parse("nosuchtag=info").is_err());
        assert!(LogSelection::parse("gc+gc=info").is_err());
        assert!(LogSelection::parse("all+gc=info").is_err());
        let six = "a+b+c+d+e+f=info";
        assert!(LogSelection::parse(six).is_err());
        assert!(LogSelection::parse("gc**=info").is_err());
    }

    #[test]
    fn selection_matching_semantics() {
        let exact = LogSelection::parse("gc+heap=info").unwrap();
        let ts_gc_heap = ts(&[LogTag::Gc, LogTag::Heap]);
        let ts_gc = ts(&[LogTag::Gc]);
        let ts_gc_heap_ref = ts(&[LogTag::Gc, LogTag::Heap, LogTag::Ref]);

        assert!(exact.selects(&ts_gc_heap));
        assert!(!exact.selects(&ts_gc));
        assert!(!exact.selects(&ts_gc_heap_ref));

        let wild = LogSelection::parse("gc*=debug").unwrap();
        assert!(wild.selects(&ts_gc));
        assert!(wild.selects(&ts_gc_heap));
        assert!(wild.selects(&ts_gc_heap_ref));
        assert!(!wild.selects(&ts(&[LogTag::Heap])));

        let all = LogSelection::parse("all=warning").unwrap();
        assert!(all.selects(&ts_gc));
        assert!(all.selects(&ts_gc_heap_ref));
    }

    #[test]
    fn selection_list_level_for() {
        let list = LogSelectionList::parse(Some("gc=debug,all=warning")).unwrap();
        assert_eq!(list.level_for(&ts(&[LogTag::Gc])), Some(LogLevel::Debug));
        assert_eq!(list.level_for(&ts(&[LogTag::Os])), Some(LogLevel::Warning));

        let list = LogSelectionList::parse(Some("gc=debug,gc+heap=trace")).unwrap();
        assert_eq!(list.level_for(&ts(&[LogTag::Gc, LogTag::Heap])), Some(LogLevel::Trace));
        assert_eq!(list.level_for(&ts(&[LogTag::Gc])), Some(LogLevel::Debug));

        let list = LogSelectionList::parse(Some("gc=debug")).unwrap();
        assert_eq!(list.level_for(&ts(&[LogTag::Os])), None);

        assert_eq!(LogSelectionList::parse(None).unwrap().selections.len(), 1);
        assert_eq!(LogSelectionList::parse(Some("")).unwrap().selections[0].level, LogLevel::Info);
    }

    #[test]
    fn default_decorators_rule() {
        assert!(LogSelectionList::parse(Some("all=info")).unwrap().has_default_decorators());
        assert!(!LogSelectionList::parse(Some("jit+inlining=debug")).unwrap().has_default_decorators());
        assert!(LogSelectionList::parse(Some("jit+inlining=debug,gc=info")).unwrap().has_default_decorators());
    }

    #[test]
    fn describe_formats() {
        let sel = LogSelection::parse("gc+heap*=debug").unwrap();
        assert_eq!(sel.describe(), "gc+heap*=debug");
        let all = LogSelection::parse("all=off").unwrap();
        assert_eq!(all.describe_tags(), "*");
    }
}
