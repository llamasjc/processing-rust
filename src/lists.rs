//! IntList, FloatList, StringList and the IntDict, FloatDict, StringDict
//! dictionaries, as in Processing. Lists are growable arrays with helpers;
//! dicts map text keys to values and keep the order keys were added in.

use crate::Json;
use std::collections::HashMap;
use std::fmt;

fn rand_index(n: usize) -> usize { ((crate::random(1.0) * n as f32) as usize).min(n.saturating_sub(1)) }

fn quote(s: &str) -> String { Json::Str(s.to_string()).to_string() }

// ---------------------------------------------------------------- lists

/// Methods every list has, whatever it holds.
macro_rules! list_common {
    ($name:ident, $t:ty) => {
        impl $name {
            pub fn new() -> $name { $name { data: Vec::new() } }
            /// A list holding these values.
            pub fn from_slice(v: &[$t]) -> $name { $name { data: v.to_vec() } }
            pub fn size(&self) -> usize { self.data.len() }
            pub fn len(&self) -> usize { self.data.len() }
            pub fn is_empty(&self) -> bool { self.data.is_empty() }
            /// The value at index (panics past the end, like Processing throws).
            pub fn get(&self, index: usize) -> $t { self.data[index].clone() }
            /// Sets a value, growing the list if index is past the end.
            pub fn set(&mut self, index: usize, v: $t) {
                if index >= self.data.len() { self.data.resize(index + 1, Default::default()); }
                self.data[index] = v;
            }
            pub fn append(&mut self, v: $t) { self.data.push(v); }
            pub fn push(&mut self, v: $t) { self.data.push(v); }
            /// Adds every value from a slice.
            pub fn append_all(&mut self, v: &[$t]) { self.data.extend_from_slice(v); }
            /// Adds the value only if it isn't in the list yet.
            pub fn append_unique(&mut self, v: $t) { if !self.has_value(v.clone()) { self.data.push(v); } }
            /// Removes and returns the last value.
            pub fn pop(&mut self) -> $t { self.data.pop().unwrap_or_default() }
            /// Puts a value at index, moving the rest along.
            pub fn insert(&mut self, index: usize, v: $t) {
                if index > self.data.len() { self.data.resize(index, Default::default()); }
                self.data.insert(index, v);
            }
            /// Removes the value at index and returns it.
            pub fn remove(&mut self, index: usize) -> $t { self.data.remove(index) }
            /// Removes the first match; its old index, or -1.
            pub fn remove_value(&mut self, v: $t) -> i32 {
                match self.data.iter().position(|x| *x == v) { Some(i) => { self.data.remove(i); i as i32 } None => -1 }
            }
            /// Removes every match; how many went.
            pub fn remove_values(&mut self, v: $t) -> usize {
                let n = self.data.len(); self.data.retain(|x| *x != v); n - self.data.len()
            }
            /// Removes a random value and returns it.
            pub fn remove_choice(&mut self) -> $t {
                if self.data.is_empty() { return Default::default(); }
                let i = rand_index(self.data.len()); self.data.remove(i)
            }
            /// A random value from the list.
            pub fn choice(&self) -> $t {
                if self.data.is_empty() { return Default::default(); }
                self.data[rand_index(self.data.len())].clone()
            }
            pub fn has_value(&self, v: $t) -> bool { self.data.iter().any(|x| *x == v) }
            /// Index of the first match, or -1.
            pub fn index(&self, v: $t) -> i32 { self.data.iter().position(|x| *x == v).map(|i| i as i32).unwrap_or(-1) }
            pub fn clear(&mut self) { self.data.clear(); }
            /// Grows (with zeros or "") or cuts the list to this length.
            pub fn resize(&mut self, n: usize) { self.data.resize(n, Default::default()); }
            pub fn swap(&mut self, a: usize, b: usize) { self.data.swap(a, b); }
            pub fn reverse(&mut self) { self.data.reverse(); }
            pub fn shuffle(&mut self) {
                for i in (1..self.data.len()).rev() { let j = rand_index(i + 1); self.data.swap(i, j); }
            }
            /// Values from start to the end.
            pub fn get_subset(&self, start: usize) -> $name { $name { data: self.data.get(start..).unwrap_or(&[]).to_vec() } }
            /// count values starting at start.
            pub fn get_subset_n(&self, start: usize, count: usize) -> $name {
                let end = (start + count).min(self.data.len());
                $name { data: self.data.get(start.min(end)..end).unwrap_or(&[]).to_vec() }
            }
            pub fn copy(&self) -> $name { self.clone() }
            /// The values as a Vec.
            pub fn array(&self) -> Vec<$t> { self.data.clone() }
            pub fn values(&self) -> Vec<$t> { self.data.clone() }
            pub fn to_array(&self) -> Vec<$t> { self.data.clone() }
            pub fn iter(&self) -> std::slice::Iter<'_, $t> { self.data.iter() }
            pub fn as_slice(&self) -> &[$t] { &self.data }
            pub fn print(&self) { for (i, v) in self.data.iter().enumerate() { println!("{i}: {v}"); } }

            #[inline] pub fn appendUnique(&mut self, v: $t) { self.append_unique(v) }
            #[inline] pub fn removeValue(&mut self, v: $t) -> i32 { self.remove_value(v) }
            #[inline] pub fn removeValues(&mut self, v: $t) -> usize { self.remove_values(v) }
            #[inline] pub fn removeChoice(&mut self) -> $t { self.remove_choice() }
            #[inline] pub fn hasValue(&self, v: $t) -> bool { self.has_value(v) }
            #[inline] pub fn getSubset(&self, start: usize) -> $name { self.get_subset(start) }
            #[inline] pub fn toArray(&self) -> Vec<$t> { self.to_array() }
            #[inline] pub fn toJSON(&self) -> Json { self.to_json() }
        }
        impl From<Vec<$t>> for $name { fn from(data: Vec<$t>) -> $name { $name { data } } }
        impl From<&[$t]> for $name { fn from(v: &[$t]) -> $name { $name { data: v.to_vec() } } }
        impl FromIterator<$t> for $name { fn from_iter<I: IntoIterator<Item = $t>>(i: I) -> $name { $name { data: i.into_iter().collect() } } }
        impl std::ops::Index<usize> for $name { type Output = $t; fn index(&self, i: usize) -> &$t { &self.data[i] } }
        impl std::ops::IndexMut<usize> for $name { fn index_mut(&mut self, i: usize) -> &mut $t { &mut self.data[i] } }
        impl IntoIterator for $name { type Item = $t; type IntoIter = std::vec::IntoIter<$t>; fn into_iter(self) -> Self::IntoIter { self.data.into_iter() } }
        impl<'a> IntoIterator for &'a $name { type Item = &'a $t; type IntoIter = std::slice::Iter<'a, $t>; fn into_iter(self) -> Self::IntoIter { self.data.iter() } }
    };
}

/// Number list methods shared by IntList and FloatList.
macro_rules! list_numeric {
    ($name:ident, $t:ty) => {
        impl $name {
            /// Adds amount to the value at index.
            pub fn add(&mut self, index: usize, amount: $t) { self.data[index] += amount; }
            pub fn sub(&mut self, index: usize, amount: $t) { self.data[index] -= amount; }
            pub fn mult(&mut self, index: usize, amount: $t) { self.data[index] *= amount; }
            pub fn div(&mut self, index: usize, amount: $t) { self.data[index] /= amount; }
            /// The smallest value.
            pub fn min(&self) -> $t { let i = self.min_index(); if i < 0 { Default::default() } else { self.data[i as usize] } }
            pub fn max(&self) -> $t { let i = self.max_index(); if i < 0 { Default::default() } else { self.data[i as usize] } }
            /// Where the smallest value is, or -1 if the list is empty.
            #[allow(clippy::eq_op)]
            pub fn min_index(&self) -> i32 {
                let mut best: Option<usize> = None;
                for (i, v) in self.data.iter().enumerate() {
                    if *v != *v { continue; } // NaN
                    if best.map(|b| *v < self.data[b]).unwrap_or(true) { best = Some(i); }
                }
                best.map(|b| b as i32).unwrap_or(-1)
            }
            #[allow(clippy::eq_op)]
            pub fn max_index(&self) -> i32 {
                let mut best: Option<usize> = None;
                for (i, v) in self.data.iter().enumerate() {
                    if *v != *v { continue; }
                    if best.map(|b| *v > self.data[b]).unwrap_or(true) { best = Some(i); }
                }
                best.map(|b| b as i32).unwrap_or(-1)
            }
            /// Smallest first.
            pub fn sort(&mut self) { self.data.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)); }
            /// Largest first.
            pub fn sort_reverse(&mut self) { self.data.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal)); }
            /// Each value divided by the sum.
            pub fn get_percent(&self) -> FloatList {
                let sum: f64 = self.data.iter().map(|v| *v as f64).sum();
                FloatList { data: self.data.iter().map(|v| (*v as f64 / sum) as f32).collect() }
            }
            /// The values joined with a separator: "1, 2, 3".
            pub fn join(&self, sep: &str) -> String { self.data.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(sep) }
            pub fn to_json(&self) -> Json { Json::Arr(self.data.iter().map(|v| Json::from(*v)).collect()) }

            #[inline] pub fn minIndex(&self) -> i32 { self.min_index() }
            #[inline] pub fn maxIndex(&self) -> i32 { self.max_index() }
            #[inline] pub fn sortReverse(&mut self) { self.sort_reverse() }
            #[inline] pub fn getPercent(&self) -> FloatList { self.get_percent() }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{} size={} [ {} ]", stringify!($name), self.data.len(), self.join(", "))
            }
        }
    };
}

/// A growable list of ints.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IntList { pub data: Vec<i32> }
list_common!(IntList, i32);
list_numeric!(IntList, i32);
impl IntList {
    /// A list of 0..n.
    pub fn range(n: i32) -> IntList { IntList { data: (0..n).collect() } }
    /// Adds one to the value at index, growing the list if needed.
    pub fn increment(&mut self, index: usize) {
        if index >= self.data.len() { self.data.resize(index + 1, 0); }
        self.data[index] += 1;
    }
    pub fn sum(&self) -> i32 { self.data.iter().fold(0i32, |a, b| a.wrapping_add(*b)) }
    pub fn sum_long(&self) -> i64 { self.data.iter().map(|v| *v as i64).sum() }
    /// Each distinct value once, in first-seen order.
    pub fn get_unique(&self) -> Vec<i32> { let mut o = IntList::new(); for v in &self.data { o.append_unique(*v); } o.data }
    #[inline] pub fn sumLong(&self) -> i64 { self.sum_long() }
}

/// A growable list of floats.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FloatList { pub data: Vec<f32> }
list_common!(FloatList, f32);
list_numeric!(FloatList, f32);
impl FloatList {
    pub fn sum(&self) -> f32 { self.sum_double() as f32 }
    pub fn sum_double(&self) -> f64 { self.data.iter().map(|v| *v as f64).sum() }
    /// Replaces the first match; false if there was none.
    pub fn replace_value(&mut self, old: f32, new: f32) -> bool {
        match self.data.iter().position(|x| *x == old) { Some(i) => { self.data[i] = new; true } None => false }
    }
    /// Replaces every match; false if there were none.
    pub fn replace_values(&mut self, old: f32, new: f32) -> bool {
        let mut any = false;
        for x in &mut self.data { if *x == old { *x = new; any = true; } }
        any
    }
    #[inline] pub fn sumDouble(&self) -> f64 { self.sum_double() }
    #[inline] pub fn replaceValue(&mut self, old: f32, new: f32) -> bool { self.replace_value(old, new) }
    #[inline] pub fn replaceValues(&mut self, old: f32, new: f32) -> bool { self.replace_values(old, new) }
}

/// A growable list of strings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StringList { pub data: Vec<String> }
list_common!(StringList, String);
impl StringList {
    /// Builds a list from anything string-like: StringList::of(&["a", "b"]).
    pub fn of<S: AsRef<str>>(v: &[S]) -> StringList { StringList { data: v.iter().map(|s| s.as_ref().to_string()).collect() } }
    /// Alphabetical, A before a.
    pub fn sort(&mut self) { self.data.sort(); }
    pub fn sort_reverse(&mut self) { self.data.sort_by(|a, b| b.cmp(a)); }
    pub fn upper(&mut self) { for s in &mut self.data { *s = s.to_uppercase(); } }
    pub fn lower(&mut self) { for s in &mut self.data { *s = s.to_lowercase(); } }
    pub fn join(&self, sep: &str) -> String { self.data.join(sep) }
    /// Replaces the first match; its index or -1.
    pub fn replace_value(&mut self, old: &str, new: &str) -> i32 {
        match self.data.iter().position(|x| x == old) { Some(i) => { self.data[i] = new.to_string(); i as i32 } None => -1 }
    }
    /// Replaces every match; how many there were.
    pub fn replace_values(&mut self, old: &str, new: &str) -> usize {
        let mut n = 0;
        for x in &mut self.data { if x == old { *x = new.to_string(); n += 1; } }
        n
    }
    /// Each distinct string once, in first-seen order.
    pub fn get_unique(&self) -> Vec<String> { self.get_tally().key_array() }
    /// How many times each string appears.
    pub fn get_tally(&self) -> IntDict { let mut d = IntDict::new(); for s in &self.data { d.increment(s); } d }
    /// Each string mapped to its (last) index.
    pub fn get_order(&self) -> IntDict { let mut d = IntDict::new(); for (i, s) in self.data.iter().enumerate() { d.set(s, i as i32); } d }
    pub fn to_json(&self) -> Json { Json::Arr(self.data.iter().map(|s| Json::from(s.as_str())).collect()) }

    #[inline] pub fn sortReverse(&mut self) { self.sort_reverse() }
    #[inline] pub fn replaceValue(&mut self, old: &str, new: &str) -> i32 { self.replace_value(old, new) }
    #[inline] pub fn replaceValues(&mut self, old: &str, new: &str) -> usize { self.replace_values(old, new) }
    #[inline] pub fn getUnique(&self) -> Vec<String> { self.get_unique() }
    #[inline] pub fn getTally(&self) -> IntDict { self.get_tally() }
    #[inline] pub fn getOrder(&self) -> IntDict { self.get_order() }
}
impl From<Vec<&str>> for StringList { fn from(v: Vec<&str>) -> StringList { StringList::of(&v) } }
impl fmt::Display for StringList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let items: Vec<String> = self.data.iter().map(|s| quote(s)).collect();
        write!(f, "StringList size={} [ {} ]", self.data.len(), items.join(", "))
    }
}

// ---------------------------------------------------------------- dicts

macro_rules! dict_common {
    ($name:ident, $t:ty) => {
        impl $name {
            pub fn new() -> $name { $name { keys: Vec::new(), vals: Vec::new(), idx: HashMap::new() } }
            /// A dict from (key, value) pairs.
            pub fn from_pairs<K: AsRef<str>>(pairs: Vec<(K, $t)>) -> $name {
                let mut d = $name::new(); for (k, v) in pairs { d.set(k.as_ref(), v); } d
            }
            fn reindex(&mut self) { self.idx = self.keys.iter().enumerate().map(|(i, k)| (k.clone(), i)).collect(); }
            pub fn size(&self) -> usize { self.keys.len() }
            pub fn len(&self) -> usize { self.keys.len() }
            pub fn is_empty(&self) -> bool { self.keys.is_empty() }
            /// The key at position i.
            pub fn key(&self, i: usize) -> String { self.keys[i].clone() }
            /// The value at position i.
            pub fn value(&self, i: usize) -> $t { self.vals[i].clone() }
            pub fn keys(&self) -> Vec<String> { self.keys.clone() }
            pub fn values(&self) -> Vec<$t> { self.vals.clone() }
            pub fn key_array(&self) -> Vec<String> { self.keys.clone() }
            pub fn value_array(&self) -> Vec<$t> { self.vals.clone() }
            /// (key, value) pairs in order.
            pub fn entries(&self) -> Vec<(String, $t)> { self.keys.iter().cloned().zip(self.vals.iter().cloned()).collect() }
            /// The value for key (zero or "" if missing).
            pub fn get(&self, key: &str) -> $t { self.idx.get(key).map(|&i| self.vals[i].clone()).unwrap_or_default() }
            /// The value for key, or the default if missing.
            pub fn get_or(&self, key: &str, default: $t) -> $t { self.idx.get(key).map(|&i| self.vals[i].clone()).unwrap_or(default) }
            /// Sets a value; new keys go at the end.
            pub fn set(&mut self, key: &str, v: $t) {
                match self.idx.get(key) {
                    Some(&i) => self.vals[i] = v,
                    None => { self.idx.insert(key.to_string(), self.keys.len()); self.keys.push(key.to_string()); self.vals.push(v); }
                }
            }
            pub fn has_key(&self, key: &str) -> bool { self.idx.contains_key(key) }
            /// Position of key, or -1.
            pub fn index(&self, key: &str) -> i32 { self.idx.get(key).map(|&i| i as i32).unwrap_or(-1) }
            /// Removes key; its value (zero or "" if it wasn't there).
            pub fn remove(&mut self, key: &str) -> $t {
                match self.idx.get(key) { Some(&i) => self.remove_index(i), None => Default::default() }
            }
            /// Removes the entry at position i and returns its value.
            pub fn remove_index(&mut self, i: usize) -> $t {
                self.keys.remove(i); let v = self.vals.remove(i); self.reindex(); v
            }
            /// Replaces the entry at position i.
            pub fn set_index(&mut self, i: usize, key: &str, v: $t) {
                self.keys[i] = key.to_string(); self.vals[i] = v; self.reindex();
            }
            pub fn swap(&mut self, a: usize, b: usize) {
                self.keys.swap(a, b); self.vals.swap(a, b);
                self.idx.insert(self.keys[a].clone(), a); self.idx.insert(self.keys[b].clone(), b);
            }
            pub fn clear(&mut self) { self.keys.clear(); self.vals.clear(); self.idx.clear(); }
            /// Cuts the dict down to n entries.
            pub fn resize(&mut self, n: usize) { self.keys.truncate(n); self.vals.truncate(n); self.reindex(); }
            pub fn copy(&self) -> $name { self.clone() }
            fn sort_impl(&mut self, by_keys: bool, reverse: bool) {
                let mut order: Vec<usize> = (0..self.keys.len()).collect();
                let cmp_v = |a: &$t, b: &$t| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal);
                order.sort_by(|&a, &b| {
                    let ka = self.keys[a].to_lowercase(); let kb = self.keys[b].to_lowercase();
                    let d = if by_keys { ka.cmp(&kb).then_with(|| cmp_v(&self.vals[a], &self.vals[b])) }
                            else { cmp_v(&self.vals[a], &self.vals[b]).then_with(|| ka.cmp(&kb)) };
                    if reverse { d.reverse() } else { d }
                });
                self.keys = order.iter().map(|&i| self.keys[i].clone()).collect();
                self.vals = order.iter().map(|&i| self.vals[i].clone()).collect();
                self.reindex();
            }
            /// Keys A to Z, ignoring case.
            pub fn sort_keys(&mut self) { self.sort_impl(true, false) }
            pub fn sort_keys_reverse(&mut self) { self.sort_impl(true, true) }
            /// Smallest value first.
            pub fn sort_values(&mut self) { self.sort_impl(false, false) }
            pub fn sort_values_reverse(&mut self) { self.sort_impl(false, true) }
            pub fn print(&self) { for (k, v) in self.keys.iter().zip(&self.vals) { println!("{k} = {v}"); } }
            /// A JSON object with the same keys and values.
            pub fn to_json(&self) -> Json {
                let mut o = Json::object();
                for (k, v) in self.keys.iter().zip(&self.vals) { o.set(k, v.clone()); }
                o
            }

            #[inline] pub fn keyArray(&self) -> Vec<String> { self.key_array() }
            #[inline] pub fn valueArray(&self) -> Vec<$t> { self.value_array() }
            #[inline] pub fn hasKey(&self, key: &str) -> bool { self.has_key(key) }
            #[inline] pub fn removeIndex(&mut self, i: usize) -> $t { self.remove_index(i) }
            #[inline] pub fn setIndex(&mut self, i: usize, key: &str, v: $t) { self.set_index(i, key, v) }
            #[inline] pub fn sortKeys(&mut self) { self.sort_keys() }
            #[inline] pub fn sortKeysReverse(&mut self) { self.sort_keys_reverse() }
            #[inline] pub fn sortValues(&mut self) { self.sort_values() }
            #[inline] pub fn sortValuesReverse(&mut self) { self.sort_values_reverse() }
            #[inline] pub fn toJSON(&self) -> Json { self.to_json() }
        }
        impl Default for $name { fn default() -> $name { $name::new() } }
        impl PartialEq for $name { fn eq(&self, o: &$name) -> bool { self.keys == o.keys && self.vals == o.vals } }
    };
}

macro_rules! dict_numeric {
    ($name:ident, $t:ty) => {
        impl $name {
            pub fn add(&mut self, key: &str, amount: $t) { let v = self.get(key); self.set(key, v + amount); }
            pub fn sub(&mut self, key: &str, amount: $t) { let v = self.get(key); self.set(key, v - amount); }
            pub fn mult(&mut self, key: &str, amount: $t) { if let Some(&i) = self.idx.get(key) { self.vals[i] *= amount; } }
            pub fn div(&mut self, key: &str, amount: $t) { if let Some(&i) = self.idx.get(key) { self.vals[i] /= amount; } }
            /// Position of the smallest value, or -1.
            #[allow(clippy::eq_op)]
            pub fn min_index(&self) -> i32 {
                let mut best: Option<usize> = None;
                for (i, v) in self.vals.iter().enumerate() {
                    if *v != *v { continue; }
                    if best.map(|b| *v < self.vals[b]).unwrap_or(true) { best = Some(i); }
                }
                best.map(|b| b as i32).unwrap_or(-1)
            }
            #[allow(clippy::eq_op)]
            pub fn max_index(&self) -> i32 {
                let mut best: Option<usize> = None;
                for (i, v) in self.vals.iter().enumerate() {
                    if *v != *v { continue; }
                    if best.map(|b| *v > self.vals[b]).unwrap_or(true) { best = Some(i); }
                }
                best.map(|b| b as i32).unwrap_or(-1)
            }
            pub fn min_key(&self) -> String { let i = self.min_index(); if i < 0 { String::new() } else { self.keys[i as usize].clone() } }
            pub fn max_key(&self) -> String { let i = self.max_index(); if i < 0 { String::new() } else { self.keys[i as usize].clone() } }
            pub fn min_value(&self) -> $t { let i = self.min_index(); if i < 0 { Default::default() } else { self.vals[i as usize] } }
            pub fn max_value(&self) -> $t { let i = self.max_index(); if i < 0 { Default::default() } else { self.vals[i as usize] } }
            /// Each value divided by the sum, under the same keys.
            pub fn get_percent(&self) -> FloatDict {
                let sum: f64 = self.vals.iter().map(|v| *v as f64).sum();
                let mut o = FloatDict::new();
                for (k, v) in self.keys.iter().zip(&self.vals) { o.set(k, (*v as f64 / sum) as f32); }
                o
            }

            #[inline] pub fn minIndex(&self) -> i32 { self.min_index() }
            #[inline] pub fn maxIndex(&self) -> i32 { self.max_index() }
            #[inline] pub fn minKey(&self) -> String { self.min_key() }
            #[inline] pub fn maxKey(&self) -> String { self.max_key() }
            #[inline] pub fn minValue(&self) -> $t { self.min_value() }
            #[inline] pub fn maxValue(&self) -> $t { self.max_value() }
            #[inline] pub fn getPercent(&self) -> FloatDict { self.get_percent() }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let items: Vec<String> = self.keys.iter().zip(&self.vals).map(|(k, v)| format!("{}: {}", quote(k), v)).collect();
                write!(f, "{} size={} {{ {} }}", stringify!($name), self.keys.len(), items.join(", "))
            }
        }
    };
}

/// Text keys to ints, in the order they were added.
#[derive(Clone, Debug)]
pub struct IntDict { keys: Vec<String>, vals: Vec<i32>, idx: HashMap<String, usize> }
dict_common!(IntDict, i32);
dict_numeric!(IntDict, i32);
impl IntDict {
    /// Adds one to key's count (starting from 0).
    pub fn increment(&mut self, key: &str) { self.add(key, 1); }
    /// Adds every count from another dict.
    pub fn increment_dict(&mut self, other: &IntDict) { for (k, v) in other.keys.iter().zip(&other.vals) { self.add(k, *v); } }
    pub fn sum(&self) -> i32 { self.vals.iter().fold(0i32, |a, b| a.wrapping_add(*b)) }
    pub fn sum_long(&self) -> i64 { self.vals.iter().map(|v| *v as i64).sum() }
    #[inline] pub fn sumLong(&self) -> i64 { self.sum_long() }
}

/// Text keys to floats, in the order they were added.
#[derive(Clone, Debug)]
pub struct FloatDict { keys: Vec<String>, vals: Vec<f32>, idx: HashMap<String, usize> }
dict_common!(FloatDict, f32);
dict_numeric!(FloatDict, f32);
impl FloatDict {
    pub fn sum(&self) -> f32 { self.sum_double() as f32 }
    pub fn sum_double(&self) -> f64 { self.vals.iter().map(|v| *v as f64).sum() }
    #[inline] pub fn sumDouble(&self) -> f64 { self.sum_double() }
}

/// Text keys to text values, in the order they were added.
#[derive(Clone, Debug)]
pub struct StringDict { keys: Vec<String>, vals: Vec<String>, idx: HashMap<String, usize> }
dict_common!(StringDict, String);
impl fmt::Display for StringDict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let items: Vec<String> = self.keys.iter().zip(&self.vals).map(|(k, v)| format!("{}: {}", quote(k), quote(v))).collect();
        write!(f, "StringDict size={} {{ {} }}", self.keys.len(), items.join(", "))
    }
}

impl From<IntList> for Json { fn from(l: IntList) -> Json { l.to_json() } }
impl From<FloatList> for Json { fn from(l: FloatList) -> Json { l.to_json() } }
impl From<StringList> for Json { fn from(l: StringList) -> Json { l.to_json() } }
impl From<IntDict> for Json { fn from(d: IntDict) -> Json { d.to_json() } }
impl From<FloatDict> for Json { fn from(d: FloatDict) -> Json { d.to_json() } }
impl From<StringDict> for Json { fn from(d: StringDict) -> Json { d.to_json() } }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_list() {
        let mut l = IntList::from(vec![5, 3, 9]);
        l.append(1);
        l.append_unique(3);
        assert_eq!(l.size(), 4);
        assert_eq!(l.sum(), 18);
        assert_eq!((l.min(), l.max(), l.min_index(), l.max_index()), (1, 9, 3, 2));
        l.sort();
        assert_eq!(l.array(), vec![1, 3, 5, 9]);
        l.sort_reverse();
        assert_eq!(l.array(), vec![9, 5, 3, 1]);
        l.increment(6);
        assert_eq!(l.array(), vec![9, 5, 3, 1, 0, 0, 1]);
        assert_eq!(l.remove_values(0), 2);
        assert_eq!(l.remove_value(3), 2);
        assert_eq!(l.index(42), -1);
        l.add(0, 1); l.mult(1, 2); l.sub(2, 1); l.div(0, 2);
        assert_eq!(l.array(), vec![5, 10, 0, 1]);
        l.insert(1, 7);
        assert_eq!(l.get_subset_n(1, 2).array(), vec![7, 10]);
        assert_eq!(l.to_string(), "IntList size=5 [ 5, 7, 10, 0, 1 ]");
        assert_eq!(l.to_json().to_string(), "[5,7,10,0,1]");
        let p = IntList::from(vec![1, 3]).get_percent();
        assert_eq!(p.array(), vec![0.25, 0.75]);
        let mut s = IntList::range(10);
        s.shuffle();
        s.sort();
        assert_eq!(s, IntList::range(10));
        let c = s.remove_choice();
        assert_eq!(s.size(), 9);
        assert!(!s.has_value(c));
    }

    #[test]
    fn float_list() {
        let mut l = FloatList::from(vec![1.5, f32::NAN, -2.0]);
        assert_eq!(l.min(), -2.0);
        assert_eq!(l.max_index(), 0);
        assert!(l.replace_value(1.5, 4.0));
        l.remove(1);
        assert_eq!(l.sum(), 2.0);
        assert_eq!(l.to_string(), "FloatList size=2 [ 4, -2 ]");
    }

    #[test]
    fn string_list() {
        let mut l = StringList::of(&["b", "a", "b", "c"]);
        let t = l.get_tally();
        assert_eq!(t.get("b"), 2);
        assert_eq!(l.get_unique(), vec!["b", "a", "c"]);
        assert_eq!(l.get_order().get("b"), 2);
        l.upper();
        l.sort();
        assert_eq!(l.join(","), "A,B,B,C");
        assert_eq!(l.replace_values("B", "x"), 2);
        assert_eq!(l.to_string(), "StringList size=4 [ \"A\", \"x\", \"x\", \"C\" ]");
    }

    #[test]
    fn dicts() {
        let mut d = IntDict::new();
        d.set("cat", 3); d.set("Ant", 3); d.increment("bee"); d.add("cat", 2);
        assert_eq!(d.keys(), vec!["cat", "Ant", "bee"]);
        assert_eq!(d.sum(), 9);
        assert_eq!((d.max_key(), d.min_value()), ("cat".to_string(), 1));
        d.sort_values();
        assert_eq!(d.keys(), vec!["bee", "Ant", "cat"]);
        d.sort_keys_reverse();
        assert_eq!(d.keys(), vec!["cat", "bee", "Ant"]);
        assert_eq!(d.index("bee"), 1);
        assert_eq!(d.remove("bee"), 1);
        assert_eq!(d.index("Ant"), 1);
        assert_eq!(d.to_string(), "IntDict size=2 { \"cat\": 5, \"Ant\": 3 }");
        assert_eq!(d.to_json().get_int("cat"), 5);
        assert_eq!(d.get_percent().get("Ant"), 3.0 / 8.0);
        d.swap(0, 1);
        assert_eq!(d.get("cat"), 5);
        assert_eq!(d.key(1), "cat");

        let mut f = FloatDict::new();
        f.set("x", 1.5); f.mult("x", 2.0);
        assert_eq!(f.get("x"), 3.0);
        assert_eq!(f.get_or("y", -1.0), -1.0);

        let mut s = StringDict::new();
        s.set("b", "two".into()); s.set("a", "one".into());
        s.sort_keys();
        assert_eq!(s.value_array(), vec!["one", "two"]);
        assert_eq!(s.to_string(), "StringDict size=2 { \"a\": \"one\", \"b\": \"two\" }");
    }
}
