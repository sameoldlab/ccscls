//! Interned strings: one pool and small ids, instead of a heap allocation per item.
//!
//! Names are only ever added. Class names that stop being used stay in the pool
//! (a few bytes each); reclaiming them would mean compacting and renumbering
//! every id, which is not worth it at this scale.

use rustc_hash::FxHasher;
use std::hash::Hasher;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NameId(u32);

impl NameId {
    pub fn index(self) -> usize {
        self.0 as usize
    }

    pub fn from_index(index: usize) -> Self {
        Self(index as u32)
    }
}

const INITIAL_TABLE_SIZE: usize = 256;

pub struct Strings {
    bytes: String,
    /// (start, length) into `bytes`, indexed by `NameId`.
    ranges: Vec<(u32, u32)>,
    /// Open-addressing hash table holding `NameId + 1`; zero means empty.
    /// Its length is a power of two and it is kept at most half full.
    table: Vec<u32>,
    scratch: String,
}

impl Default for Strings {
    fn default() -> Self {
        Self::new()
    }
}

impl Strings {
    pub fn new() -> Self {
        Self {
            bytes: String::new(),
            ranges: Vec::new(),
            table: vec![0; INITIAL_TABLE_SIZE],
            scratch: String::new(),
        }
    }

    /// The text of `id`, or `""` for an id this pool never issued.
    pub fn get(&self, id: NameId) -> &str {
        let Some(&(start, length)) = self.ranges.get(id.index()) else {
            return "";
        };
        self.bytes
            .get(start as usize..start as usize + length as usize)
            .unwrap_or("")
    }

    pub fn lookup(&self, text: &str) -> Option<NameId> {
        let mask = self.table.len().saturating_sub(1);
        let mut slot = hash_of(text) as usize & mask;
        loop {
            let stored = *self.table.get(slot)?;
            if stored == 0 {
                return None;
            }
            let id = NameId(stored - 1);
            if self.get(id) == text {
                return Some(id);
            }
            slot = (slot + 1) & mask;
        }
    }

    pub fn intern(&mut self, text: &str) -> NameId {
        if let Some(existing) = self.lookup(text) {
            return existing;
        }
        let start = self.bytes.len();
        self.bytes.push_str(text);
        let id = NameId(self.ranges.len() as u32);
        self.ranges.push((start as u32, text.len() as u32));
        if self.ranges.len() * 2 > self.table.len() {
            self.grow_table();
        } else {
            self.place(id);
        }
        id
    }

    /// Intern `text` with every run of whitespace collapsed to a single space.
    pub fn intern_collapsed(&mut self, text: &str) -> NameId {
        let mut collapsed = std::mem::take(&mut self.scratch);
        collapsed.clear();
        for word in text.split_whitespace() {
            if !collapsed.is_empty() {
                collapsed.push(' ');
            }
            collapsed.push_str(word);
        }
        let id = self.intern(&collapsed);
        self.scratch = collapsed;
        id
    }

    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    fn place(&mut self, id: NameId) {
        let mask = self.table.len().saturating_sub(1);
        let mut slot = hash_of(self.get(id)) as usize & mask;
        while self.table.get(slot).is_some_and(|&stored| stored != 0) {
            slot = (slot + 1) & mask;
        }
        if let Some(entry) = self.table.get_mut(slot) {
            *entry = id.0 + 1;
        }
    }

    fn grow_table(&mut self) {
        let new_size = self.table.len() * 2;
        self.table = vec![0; new_size];
        for index in 0..self.ranges.len() {
            self.place(NameId(index as u32));
        }
    }
}

fn hash_of(text: &str) -> u64 {
    let mut hasher = FxHasher::default();
    hasher.write(text.as_bytes());
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_text_same_id_different_text_different_id() {
        let mut strings = Strings::new();
        let first = strings.intern("btn");
        let second = strings.intern("card");
        assert_ne!(first, second);
        assert_eq!(strings.intern("btn"), first);
        assert_eq!(strings.get(first), "btn");
        assert_eq!(strings.get(second), "card");
        assert_eq!(strings.len(), 2);
    }

    #[test]
    fn survives_growth_and_unicode() {
        let mut strings = Strings::new();
        let ids: Vec<NameId> = (0..5000).map(|number| strings.intern(&format!("class-{number}-日本語"))).collect();
        for (number, id) in ids.iter().enumerate() {
            assert_eq!(strings.get(*id), format!("class-{number}-日本語"));
            assert_eq!(strings.lookup(&format!("class-{number}-日本語")), Some(*id));
        }
        assert_eq!(strings.lookup("never interned"), None);
    }

    #[test]
    fn empty_string_and_unknown_ids_are_safe() {
        let mut strings = Strings::new();
        let empty = strings.intern("");
        assert_eq!(strings.get(empty), "");
        assert_eq!(strings.get(NameId::from_index(999)), "");
        assert_eq!(strings.intern(""), empty);
    }

    #[test]
    fn collapsed_interning_normalises_whitespace() {
        let mut strings = Strings::new();
        let tidy = strings.intern("@media screen and (max-width: 500px)");
        let messy = strings.intern_collapsed("@media screen\n   and  (max-width:\t500px)");
        assert_eq!(tidy, messy);
    }
}
