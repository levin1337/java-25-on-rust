/*
 * OpenJDK 25 HotSpot jfr/recorder/service/jfrOptionSet.hpp + .cpp port
 *
 * Per-recording option set (settings + values), keyed by name.
 * Rust: Mutex<BTreeMap<String,String>> per recording.
 */

use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct JfrOptionSet {
    options: BTreeMap<String, String>,
}

static OPTIONS: Mutex<OptionSetHolder> = Mutex::new(OptionSetHolder::empty());

pub struct OptionSetHolder {
    sets: Vec<JfrOptionSet>,
    active: usize,
}

impl OptionSetHolder {
    const fn empty() -> Self {
        OptionSetHolder { sets: Vec::new(), active: 0 }
    }
}

impl JfrOptionSet {
    pub fn new() -> Self {
        JfrOptionSet { options: BTreeMap::new() }
    }

    pub fn set(&mut self, name: &str, value: &str) {
        self.options.insert(name.to_string(), value.to_string());
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.options.get(name).map(|s| s.as_str())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.options.contains_key(name)
    }

    // flushpoint option
    pub fn flushpoint(&self) -> bool {
        self.get("flushpoint").map(|v| v == "true" || v == "1").unwrap_or(false)
    }
}

pub fn global_set_option(name: &str, value: &str) {
    let mut g = OPTIONS.lock().unwrap();
    if g.sets.is_empty() {
        g.sets.push(JfrOptionSet::new());
    }
    let i = g.active.min(g.sets.len() - 1);
    g.sets[i].set(name, value);
}

pub fn global_get_option(name: &str) -> Option<String> {
    let g = OPTIONS.lock().unwrap();
    g.sets.get(g.active).and_then(|s| s.get(name)).map(|s| s.to_string())
}

/// JfrOptionSet::compressed_integers — checkpoint streams use compressed
/// integer encoding unless the recording disabled it.
pub fn compressed_integers() -> bool {
    global_get_option("compressedintegers")
        .map(|v| !(v == "false" || v == "0"))
        .unwrap_or(true)
}
