/*
 * OpenJDK 25 HotSpot jfr/metadata/jfrMetadataWriter.hpp + .cpp port
 *
 * Serializes the metadata descriptor tree into a JFRWriter, matching
 * the Java MetadataWriter binary layout:
 *   u4 stringPoolSize
 *   string pool (varint-128 utf8 strings)
 *   element tree: nameIdx(u4), attrCount(u4), attr name/value idx,
 *                 childCount(u4), children...
 */

use std::collections::{BTreeMap, HashMap};

use crate::share::jfr::metadata::jfr_metadata_element::JfrMetadataElement;
use crate::share::jfr::writers::jfr_writer::JfrWriter;

pub struct JfrMetadataWriter;

impl JfrMetadataWriter {
    // Serialize the full descriptor tree into `w` at current position.
    pub fn write_metadata(w: &mut JfrWriter, root: &JfrMetadataElement) {
        // 1. collect string pool (ordered by first appearance)
        let mut pool: Vec<String> = Vec::new();
        let mut index: HashMap<String, u32> = HashMap::new();
        fn intern(s: &str, pool: &mut Vec<String>, index: &mut HashMap<String, u32>) -> u32 {
            if let Some(&i) = index.get(s) {
                return i;
            }
            let i = pool.len() as u32;
            pool.push(s.to_string());
            index.insert(s.to_string(), i);
            i
        }
        fn walk(e: &JfrMetadataElement, pool: &mut Vec<String>, index: &mut HashMap<String, u32>) {
            intern(&e.name, pool, index);
            let mut attrs: Vec<(&String, &String)> = e.attributes.iter().collect();
            attrs.sort();
            for (k, v) in attrs {
                intern(k, pool, index);
                intern(v, pool, index);
            }
            for c in &e.children {
                walk(c, pool, index);
            }
        }
        walk(root, &mut pool, &mut index);

        // 2. write pool size + strings (JFR string encoding: utf8)
        w.write_u4(pool.len() as u32);
        for s in &pool {
            w.write_utf8(Some(s));
        }

        // 3. write element tree
        fn write_element(w: &mut JfrWriter, e: &JfrMetadataElement, index: &HashMap<String, u32>) {
            w.write_u4(*index.get(&e.name).unwrap_or(&0));
            w.write_u4(e.attributes.len() as u32);
            let mut attrs: Vec<(&String, &String)> = e.attributes.iter().collect();
            attrs.sort();
            for (k, v) in attrs {
                w.write_u4(*index.get(k).unwrap_or(&0));
                w.write_u4(*index.get(v).unwrap_or(&0));
            }
            w.write_u4(e.children.len() as u32);
            for c in &e.children {
                write_element(w, c, index);
            }
        }
        write_element(w, root, &index);
    }

    // Size in bytes of the serialized form (for offset patching).
    pub fn serialized_size(root: &JfrMetadataElement) -> usize {
        // approximate: used for reserving space before writing
        let mut size = 4; // pool size
        let mut seen: BTreeMap<String, ()> = BTreeMap::new();
        fn walk(e: &JfrMetadataElement, seen: &mut BTreeMap<String, ()>) -> usize {
            let mut n = 0;
            if seen.insert(e.name.clone(), ()).is_none() {
                n += 1 + e.name.len() + 2;
            }
            let mut attrs: Vec<(&String, &String)> = e.attributes.iter().collect();
            attrs.sort();
            for (k, v) in attrs {
                if seen.insert(k.clone(), ()).is_none() {
                    n += 1 + k.len() + 2;
                }
                if seen.insert(v.clone(), ()).is_none() {
                    n += 1 + v.len() + 2;
                }
            }
            for c in &e.children {
                n += walk(c, seen);
            }
            n
        }
        size += walk(root, &mut seen);
        // element tree
        fn tree_size(e: &JfrMetadataElement) -> usize {
            let mut n = 4 + 4; // name + attr count
            let mut attrs: Vec<(&String, &String)> = e.attributes.iter().collect();
            attrs.sort();
            n += attrs.len() * 8;
            n += 4; // child count
            for c in &e.children {
                n += tree_size(c);
            }
            n
        }
        size + tree_size(root)
    }
}
