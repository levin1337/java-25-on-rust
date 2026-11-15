use std::borrow::Borrow;
use std::collections::hash_map::DefaultHasher;
use std::hash::{BuildHasher, Hash, Hasher};

#[derive(Clone, Debug)]
struct Entry<K, V> {
    key: K,
    value: V,
    next: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct HashTable<K, V, S = DefaultHasher> {
    buckets: Vec<Option<usize>>,
    entries: Vec<Entry<K, V>>,
    used: usize,
    hasher: S,
}

impl<K, V, S> HashTable<K, V, S>
where
    K: Eq + Hash,
    S: BuildHasher,
{
    pub fn with_hasher(bucket_len: usize, hasher: S) -> Self {
        let bucket_len = bucket_len.max(1).next_power_of_two();
        Self {
            buckets: vec![None; bucket_len],
            entries: Vec::new(),
            used: 0,
            hasher,
        }
    }

    pub fn bucket_len(&self) -> usize {
        self.buckets.len()
    }

    pub fn table_size(&self) -> usize {
        self.entries.len()
    }

    pub fn number_of_elements(&self) -> usize {
        self.used
    }

    pub fn is_empty(&self) -> bool {
        self.used == 0
    }

    fn hash<Q: Hash + ?Sized>(&self, key: &Q) -> usize {
        let mut h = self.hasher.build_hasher();
        key.hash(&mut h);
        h.finish() as usize
    }

    fn bucket_index<Q: Hash + ?Sized>(&self, key: &Q) -> usize {
        self.hash(key) & (self.buckets.len() - 1)
    }

    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let b = self.bucket_index(key);
        let mut cur = self.buckets[b];
        while let Some(i) = cur {
            let e = &self.entries[i];
            if e.key.borrow() == key {
                return true;
            }
            cur = e.next;
        }
        false
    }

    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let b = self.bucket_index(key);
        let mut cur = self.buckets[b];
        while let Some(i) = cur {
            let e = &self.entries[i];
            if e.key.borrow() == key {
                return Some(&e.value);
            }
            cur = e.next;
        }
        None
    }

    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let b = self.bucket_index(key);
        let mut cur = self.buckets[b];
        while let Some(i) = cur {
            let e = &mut self.entries[i];
            if e.key.borrow() == key {
                return Some(&mut e.value);
            }
            cur = e.next;
        }
        None
    }

    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        if self.used * 4 >= self.buckets.len() * 3 {
            self.resize(self.buckets.len() * 2);
        }
        let b = self.bucket_index(&key);
        let mut cur = self.buckets[b];
        while let Some(i) = cur {
            let e = &mut self.entries[i];
            if e.key == key {
                return Some(std::mem::replace(&mut e.value, value));
            }
            cur = e.next;
        }
        let idx = self.entries.len();
        self.entries.push(Entry {
            key,
            value,
            next: self.buckets[b],
        });
        self.buckets[b] = Some(idx);
        self.used += 1;
        None
    }

    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let b = self.bucket_index(key);
        let mut prev: Option<usize> = None;
        let mut cur = self.buckets[b];
        while let Some(i) = cur {
            let matches = self.entries[i].key.borrow() == key;
            if matches {
                let removed = self.entries.remove(i);
                let next = removed.next.map(|n| if n > i { n - 1 } else { n });
                match prev {
                    Some(p) => self.entries[p].next = next,
                    None => self.buckets[b] = next,
                }
                for e in self.entries.iter_mut() {
                    if let Some(n) = e.next {
                        e.next = Some(if n > i { n - 1 } else { n });
                    }
                }
                self.used -= 1;
                return Some(removed.value);
            }
            prev = Some(i);
            cur = self.entries[i].next;
        }
        None
    }

    pub fn clear(&mut self) {
        self.buckets.iter_mut().for_each(|b| *b = None);
        self.entries.clear();
        self.used = 0;
    }

    fn resize(&mut self, new_bucket_len: usize) {
        let new_bucket_len = new_bucket_len.max(1).next_power_of_two();
        let mut new_buckets = vec![None; new_bucket_len];
        let mask = new_bucket_len - 1;
        for (i, e) in self.entries.iter_mut().enumerate() {
            let mut h = self.hasher.build_hasher();
            e.key.hash(&mut h);
            let b = (h.finish() as usize) & mask;
            e.next = new_buckets[b];
            new_buckets[b] = Some(i);
        }
        self.buckets = new_buckets;
    }

    pub fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter().map(|e| (&e.key, &e.value))
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.entries.iter().map(|e| &e.key)
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.iter().map(|e| &e.value)
    }

    pub fn head(&self) -> Option<usize> {
        self.entries.first().map(|_| 0)
    }

    pub fn chain(&self, bucket: usize) -> impl Iterator<Item = (&K, &V)> + '_ {
        let mut cur = self.buckets.get(bucket).copied().flatten();
        std::iter::from_fn(move || {
            let i = cur?;
            let e = &self.entries[i];
            cur = e.next;
            Some((&e.key, &e.value))
        })
    }

    pub fn entry_index<Q>(&self, key: &Q) -> Option<usize>
    where
        K: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        let b = self.bucket_index(key);
        let mut cur = self.buckets[b];
        while let Some(i) = cur {
            if self.entries[i].key.borrow() == key {
                return Some(i);
            }
            cur = self.entries[i].next;
        }
        None
    }
}

impl<K, V> HashTable<K, V, DefaultHasher>
where
    K: Eq + Hash,
{
    pub fn new(bucket_len: usize) -> Self {
        Self::with_hasher(bucket_len, DefaultHasher::new())
    }
}

impl<K, V, S> IntoIterator for HashTable<K, V, S> {
    type Item = (K, V);
    type IntoIter = std::vec::IntoIter<(K, V)>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries
            .into_iter()
            .map(|e| (e.key, e.value))
            .collect::<Vec<_>>()
            .into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_insert_get_remove() {
        let mut t = HashTable::<String, i32>::new(4);
        assert!(t.is_empty());
        t.insert("a".to_string(), 1);
        t.insert("b".to_string(), 2);
        t.insert("a".to_string(), 3);
        assert_eq!(t.number_of_elements(), 2);
        assert_eq!(t.get("a"), Some(&3));
        assert_eq!(t.get("b"), Some(&2));
        assert_eq!(t.get("c"), None);
        assert_eq!(t.remove("a"), Some(3));
        assert_eq!(t.get("a"), None);
        assert_eq!(t.number_of_elements(), 1);
    }

    #[test]
    fn resize_and_iter() {
        let mut t = HashTable::<u32, u32>::new(2);
        for i in 0..100u32 {
            t.insert(i, i * 2);
        }
        assert_eq!(t.number_of_elements(), 100);
        for i in 0..100u32 {
            assert_eq!(t.get(&i), Some(&(i * 2)));
        }
        let mut collected: Vec<u32> = t.keys().copied().collect();
        collected.sort();
        assert_eq!(collected, (0..100).collect::<Vec<_>>());
        t.clear();
        assert!(t.is_empty());
        assert_eq!(t.table_size(), 0);
    }

    #[test]
    fn chain_iteration() {
        let mut t = HashTable::<u64, u64>::new(8);
        for i in 0..50u64 {
            t.insert(i, i);
        }
        let mut total = 0usize;
        for b in 0..t.bucket_len() {
            for (k, v) in t.chain(b) {
                assert_eq!(*k, *v);
                total += 1;
            }
        }
        assert_eq!(total, 50);
    }
}
