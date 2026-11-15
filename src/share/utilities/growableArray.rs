#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrowableArray<E> {
    data: Vec<E>,
}

pub struct GrowableArrayIter<'a, E> {
    data: &'a [E],
    index: usize,
}

impl<'a, E> Iterator for GrowableArrayIter<'a, E> {
    type Item = &'a E;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.data.len() {
            return None;
        }
        let item = &self.data[self.index];
        self.index += 1;
        Some(item)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.data.len() - self.index;
        (remaining, Some(remaining))
    }
}

impl<'a, E> ExactSizeIterator for GrowableArrayIter<'a, E> {}

impl<'a, E> IntoIterator for &'a GrowableArray<E> {
    type Item = &'a E;
    type IntoIter = GrowableArrayIter<'a, E>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<E> GrowableArray<E> {
    pub fn new() -> Self {
        Self { data: Vec::new() }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
        }
    }

    pub fn from_vec(data: Vec<E>) -> Self {
        Self { data }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn length(&self) -> usize {
        self.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn is_nonempty(&self) -> bool {
        !self.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.data.capacity()
    }

    pub fn is_full(&self) -> bool {
        self.len() == self.capacity()
    }

    pub fn push(&mut self, elem: E) {
        self.data.push(elem);
    }

    pub fn append(&mut self, elem: E) -> usize {
        let idx = self.data.len();
        self.data.push(elem);
        idx
    }

    pub fn pop(&mut self) -> Option<E> {
        self.data.pop()
    }

    pub fn at(&self, index: usize) -> Option<&E> {
        self.data.get(index)
    }

    pub fn at_mut(&mut self, index: usize) -> Option<&mut E> {
        self.data.get_mut(index)
    }

    pub fn operator_index(&self, index: usize) -> &E {
        &self.data[index]
    }

    pub fn operator_index_mut(&mut self, index: usize) -> &mut E {
        &mut self.data[index]
    }

    pub fn first(&self) -> Option<&E> {
        self.data.first()
    }

    pub fn last(&self) -> Option<&E> {
        self.data.last()
    }

    pub fn top(&self) -> Option<&E> {
        self.data.last()
    }

    pub fn clear(&mut self) {
        self.data.clear();
    }

    pub fn trunc_to(&mut self, length: usize) {
        assert!(length <= self.data.len(), "cannot increase length");
        self.data.truncate(length);
    }

    pub fn as_slice(&self) -> &[E] {
        &self.data
    }

    pub fn as_mut_slice(&mut self) -> &mut [E] {
        &mut self.data
    }

    pub fn iter(&self) -> GrowableArrayIter<'_, E> {
        GrowableArrayIter {
            data: &self.data,
            index: 0,
        }
    }

    pub fn at_put(&mut self, i: usize, elem: E) {
        self.data[i] = elem;
    }

    pub fn adr_at(&self, i: usize) -> usize {
        self.data.as_ptr() as usize + i * size_of::<E>()
    }

    pub fn contains(&self, elem: &E) -> bool
    where
        E: PartialEq,
    {
        self.data.iter().any(|e| e == elem)
    }

    pub fn find(&self, elem: &E) -> Option<usize>
    where
        E: PartialEq,
    {
        self.data.iter().position(|e| e == elem)
    }

    pub fn find_from_end(&self, elem: &E) -> Option<usize>
    where
        E: PartialEq,
    {
        self.data.iter().rposition(|e| e == elem)
    }

    pub fn find_if<F>(&self, predicate: F) -> Option<usize>
    where
        F: Fn(&E) -> bool,
    {
        self.data.iter().position(|e| predicate(e))
    }

    pub fn find_from_end_if<F>(&self, predicate: F) -> Option<usize>
    where
        F: Fn(&E) -> bool,
    {
        self.data.iter().rposition(|e| predicate(e))
    }

    pub fn remove(&mut self, elem: &E) -> bool
    where
        E: PartialEq,
    {
        match self.find(elem) {
            Some(i) => {
                self.remove_at(i);
                true
            }
            None => false,
        }
    }

    pub fn remove_if_existing(&mut self, elem: &E) -> bool
    where
        E: PartialEq,
    {
        self.remove(elem)
    }

    pub fn remove_at(&mut self, index: usize) {
        self.data.remove(index);
    }

    pub fn remove_till(&mut self, idx: usize) {
        self.remove_range(0, idx);
    }

    pub fn remove_range(&mut self, start: usize, end: usize) {
        assert!(start < end && end <= self.data.len());
        self.data.drain(start..end);
    }

    pub fn sort_by<F>(&mut self, comparator: F)
    where
        F: FnMut(&E, &E) -> std::cmp::Ordering,
    {
        self.data.sort_by(comparator);
    }

    pub fn sort_unstable_by<F>(&mut self, comparator: F)
    where
        F: FnMut(&E, &E) -> std::cmp::Ordering,
    {
        self.data.sort_unstable_by(comparator);
    }

    pub fn sort_by_key<K, F>(&mut self, f: F)
    where
        F: FnMut(&E) -> K,
        K: Ord,
    {
        self.data.sort_by_key(f);
    }

    pub fn find_sorted<K, F>(&self, key: &K, compare: F) -> (usize, bool)
    where
        F: Fn(&K, &E) -> std::cmp::Ordering,
    {
        let mut min = 0usize;
        let mut max = self.data.len().wrapping_sub(1);
        let mut found = false;

        while self.data.len() > 0 && max >= min {
            let mid = min + (max - min) / 2;
            match compare(key, &self.data[mid]) {
                std::cmp::Ordering::Greater => min = mid + 1,
                std::cmp::Ordering::Less => {
                    if mid == 0 {
                        break;
                    }
                    max = mid - 1;
                }
                std::cmp::Ordering::Equal => {
                    found = true;
                    return (mid, found);
                }
            }
        }
        (min, found)
    }

    pub fn find_sorted_by<F>(&self, key: &E, compare: F) -> (usize, bool)
    where
        F: Fn(&E, &E) -> std::cmp::Ordering,
    {
        self.find_sorted(key, compare)
    }

    pub fn reserve(&mut self, new_capacity: usize) {
        self.data.reserve(new_capacity.saturating_sub(self.data.capacity()));
    }

    pub fn shrink_to_fit(&mut self) {
        self.data.shrink_to_fit();
    }

    pub fn swap(&mut self, other: &mut Self) {
        std::mem::swap(&mut self.data, &mut other.data);
    }

    pub fn append_array(&mut self, other: &GrowableArray<E>)
    where
        E: Clone,
    {
        self.data.extend_from_slice(other.as_slice());
    }
}

impl<E: Clone> GrowableArray<E> {
    pub fn delete_at(&mut self, index: usize) {
        let last = self.data.len() - 1;
        self.data[index] = self.data[last].clone();
        self.data.pop();
    }

    pub fn append_if_missing(&mut self, elem: &E) -> bool
    where
        E: PartialEq,
    {
        let missed = !self.contains(elem);
        if missed {
            self.append(elem.clone());
        }
        missed
    }

    pub fn at_grow(&mut self, i: usize, fill: E) -> &mut E {
        assert!(i <= isize::MAX as usize, "negative index");
        if i >= self.data.len() {
            if i >= self.data.capacity() {
                let new_cap = (i + 1).next_power_of_two();
                self.data.reserve(new_cap - self.data.capacity());
            }
            self.data.resize(i + 1, fill.clone());
        }
        &mut self.data[i]
    }

    pub fn at_put_grow(&mut self, i: usize, elem: E, fill: E) {
        if i >= self.data.len() {
            if i >= self.data.capacity() {
                let new_cap = (i + 1).next_power_of_two();
                self.data.reserve(new_cap - self.data.capacity());
            }
            self.data.resize(i + 1, fill);
        }
        self.data[i] = elem;
    }

    pub fn insert_before(&mut self, idx: usize, elem: E) {
        assert!(idx <= self.data.len());
        self.data.insert(idx, elem);
    }

    pub fn insert_before_all(&mut self, idx: usize, array: &GrowableArray<E>) {
        assert!(idx <= self.data.len());
        for (j, e) in array.iter().enumerate() {
            self.data.insert(idx + j, e.clone());
        }
    }

    pub fn append_all(&mut self, l: &GrowableArray<E>) {
        for e in l.iter() {
            self.append(e.clone());
        }
    }

    pub fn insert_sorted<F>(&mut self, key: E, compare: F) -> usize
    where
        F: Fn(&E, &E) -> std::cmp::Ordering,
        E: PartialEq,
    {
        let (location, found) = self.find_sorted(&key, |k, e| compare(k, e));
        if !found {
            self.insert_before(location, key);
        }
        location
    }
}

impl<E: Clone> Default for GrowableArray<E> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_ops() {
        let mut a: GrowableArray<i32> = GrowableArray::new();
        assert!(a.is_empty());
        a.push(1);
        a.push(2);
        a.push(3);
        assert_eq!(a.len(), 3);
        assert_eq!(a.pop(), Some(3));
        assert_eq!(a.first(), Some(&1));
        assert_eq!(a.last(), Some(&2));
    }

    #[test]
    fn find_remove() {
        let mut a = GrowableArray::from_vec(vec![10, 20, 30, 20]);
        assert_eq!(a.find(&20), Some(1));
        assert_eq!(a.find_from_end(&20), Some(3));
        assert_eq!(a.find_if(|x| *x > 15), Some(1));
        assert!(a.remove(&20));
        assert_eq!(a.as_slice(), &[10, 30, 20]);
        a.remove_at(0);
        assert_eq!(a.as_slice(), &[30, 20]);
        a.delete_at(0);
        assert_eq!(a.as_slice(), &[20]);
        a.remove_range(0, 1);
        assert!(a.is_empty());
    }

    #[test]
    fn grow_ops() {
        let mut a: GrowableArray<i32> = GrowableArray::new();
        *a.at_grow(5, -1) = 42;
        assert_eq!(a.len(), 6);
        assert_eq!(a.as_slice(), &[-1, -1, -1, -1, -1, 42]);
        a.at_put_grow(9, 7, 0);
        assert_eq!(a.len(), 10);
        assert_eq!(a.at(9), Some(&7));
        a.insert_before(0, 100);
        assert_eq!(a.at(0), Some(&100));
        a.insert_before_all(1, &GrowableArray::from_vec(vec![1, 2]));
        assert_eq!(a.as_slice()[1..3], [1, 2]);
    }

    #[test]
    fn sorted_ops() {
        let mut a = GrowableArray::from_vec(vec![1, 3, 5, 7]);
        let (pos, found) = a.find_sorted(&4, |k, e| k.cmp(e));
        assert!(!found);
        assert_eq!(pos, 2);
        let (pos, found) = a.find_sorted(&5, |k, e| k.cmp(e));
        assert!(found);
        assert_eq!(pos, 2);
        a.insert_sorted(4, |k, e| k.cmp(e));
        assert_eq!(a.as_slice(), &[1, 3, 4, 5, 7]);
        a.sort_by(|x, y| y.cmp(x));
        assert_eq!(a.as_slice(), &[7, 5, 4, 3, 1]);
    }

    #[test]
    fn iter_and_append() {
        let mut a = GrowableArray::from_vec(vec![1, 2]);
        let mut b = GrowableArray::from_vec(vec![3, 4]);
        a.append_all(&b);
        assert_eq!(a.len(), 4);
        let sum: i32 = a.iter().sum();
        assert_eq!(sum, 10);
        assert!(a.append_if_missing(&9));
        assert!(!a.append_if_missing(&9));
        a.swap(&mut b);
        assert_eq!(a.len(), 4);
    }
}
