/*
 * Port of OpenJDK 25 HotSpot ShenandoahStringDedup
 * (share/gc/shenandoah/shenandoahStringDedup.hpp / shenandoahStringDedup.inline.hpp,
 *  on top of share/gc/shared/stringdedup/stringDedup.hpp)
 *
 * Candidate selection for string deduplication during marking and a
 * deduplication table mapping byte-array contents to canonical object
 * addresses. The dedup flag lives in the java.lang.String flags word;
 * age comes from the mark word.
 */

use super::shenandoahForwarding::MAX_AGE;
use std::collections::{HashMap, VecDeque};

pub const DEDUPLICATION_FORBIDDEN: u32 = 1 << 0;
pub const DEDUPLICATION_REQUESTED: u32 = 1 << 1;

pub const ENABLED_AGE_THRESHOLD: u64 = 2;
pub const ENABLED_AGE_LIMIT: u64 = 4;

#[derive(Clone, Copy, Debug)]
pub struct StringCandidateInfo {
    pub is_string: bool,
    pub has_value: bool,
    pub age: u64,
    pub flags: u32,
}

pub struct ShenandoahStringDedup {
    enabled: bool,
    table: HashMap<Vec<u8>, u64>,
    requests: VecDeque<u64>,
    total_requests: usize,
    total_deduplicated: usize,
}

impl ShenandoahStringDedup {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            table: HashMap::new(),
            requests: VecDeque::new(),
            total_requests: 0,
            total_deduplicated: 0,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn is_string_candidate(&self, info: &StringCandidateInfo) -> bool {
        info.is_string && info.has_value
    }

    pub fn dedup_requested(&self, info: &StringCandidateInfo) -> bool {
        (info.flags & DEDUPLICATION_REQUESTED) != 0
    }

    pub fn is_candidate(&self, info: &StringCandidateInfo) -> bool {
        if !self.is_string_candidate(info) {
            return false;
        }
        info.age <= MAX_AGE
            && info.age <= ENABLED_AGE_THRESHOLD
            && !self.dedup_requested(info)
            && (info.flags & DEDUPLICATION_FORBIDDEN) == 0
    }

    pub fn request(&mut self, obj: u64) {
        if self.requests.len() >= 100_000 {
            return;
        }
        self.requests.push_back(obj);
        self.total_requests += 1;
    }

    pub fn requests_len(&self) -> usize {
        self.requests.len()
    }

    pub fn drain_requests(&mut self) -> Vec<u64> {
        self.requests.drain(..).collect()
    }

    pub fn lookup(&self, bytes: &[u8]) -> Option<u64> {
        self.table.get(bytes).copied()
    }

    pub fn insert(&mut self, bytes: Vec<u8>, obj: u64) -> bool {
        if self.table.contains_key(&bytes) {
            return false;
        }
        self.table.insert(bytes, obj);
        true
    }

    pub fn table_size(&self) -> usize {
        self.table.len()
    }

    pub fn total_requests(&self) -> usize {
        self.total_requests
    }

    pub fn note_deduplicated(&mut self) {
        self.total_deduplicated += 1;
    }

    pub fn total_deduplicated(&self) -> usize {
        self.total_deduplicated
    }
}

impl Default for ShenandoahStringDedup {
    fn default() -> Self {
        Self::new(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(age: u64, flags: u32) -> StringCandidateInfo {
        StringCandidateInfo {
            is_string: true,
            has_value: true,
            age,
            flags,
        }
    }

    #[test]
    fn candidate_selection() {
        let dedup = ShenandoahStringDedup::new(true);
        assert!(dedup.is_candidate(&info(0, 0)));
        assert!(dedup.is_candidate(&info(ENABLED_AGE_THRESHOLD, 0)));
        assert!(!dedup.is_candidate(&info(ENABLED_AGE_THRESHOLD + 1, 0)));
        assert!(!dedup.is_candidate(&info(0, DEDUPLICATION_REQUESTED)));
        assert!(!dedup.is_candidate(&info(0, DEDUPLICATION_FORBIDDEN)));
        let mut non_string = info(0, 0);
        non_string.is_string = false;
        assert!(!dedup.is_candidate(&non_string));
    }

    #[test]
    fn dedup_table_and_requests() {
        let mut dedup = ShenandoahStringDedup::new(true);
        dedup.request(0x1000);
        dedup.request(0x2000);
        assert_eq!(dedup.requests_len(), 2);
        let drained = dedup.drain_requests();
        assert_eq!(drained, vec![0x1000, 0x2000]);
        assert_eq!(dedup.requests_len(), 0);

        assert!(dedup.insert(b"abc".to_vec(), 0x3000));
        assert!(!dedup.insert(b"abc".to_vec(), 0x4000));
        assert_eq!(dedup.lookup(b"abc"), Some(0x3000));
        assert_eq!(dedup.table_size(), 1);
    }
}
