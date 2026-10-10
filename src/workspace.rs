//! The set of indexed CSS files plus a name -> definitions map.

use crate::css::{ClassDef, CssIndexer, class_at};
use crate::line_index::LineIndex;
use crate::uri::file_key;
use lsp_types::Uri;
use rustc_hash::FxHashMap;

pub struct FileEntry {
    /// The URI to report locations with (the client's spelling wins over ours).
    pub uri: Uri,
    pub text: String,
    pub line_index: LineIndex,
    /// Sorted by position.
    pub defs: Vec<ClassDef>,
    /// `Some(version)` while the client has the document open; the buffer then
    /// takes precedence over what is on disk.
    pub open_version: Option<i32>,
}

pub struct DefRef<'a> {
    pub file: &'a FileEntry,
    pub def: &'a ClassDef,
}

#[derive(Default)]
pub struct Workspace {
    indexer: CssIndexer,
    slots: Vec<Option<FileEntry>>,
    free: Vec<u32>,
    ids: FxHashMap<String, u32>,
    /// class name -> (file slot, index into that file's `defs`)
    by_name: FxHashMap<Box<str>, Vec<(u32, u32)>>,
}

impl Workspace {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace a file, re-indexing it.
    pub fn upsert(&mut self, uri: Uri, text: String, open_version: Option<i32>) {
        let key = file_key(&uri);
        let defs = self.indexer.index(&text);
        let line_index = LineIndex::new(&text);

        let id = match self.ids.get(&key) {
            Some(&id) => {
                self.unlink(id);
                id
            }
            None => {
                let id = match self.free.pop() {
                    Some(id) => id,
                    None => {
                        self.slots.push(None);
                        (self.slots.len() - 1) as u32
                    }
                };
                self.ids.insert(key, id);
                id
            }
        };
        for (i, d) in defs.iter().enumerate() {
            self.by_name
                .entry(d.name.clone())
                .or_default()
                .push((id, i as u32));
        }
        self.slots[id as usize] = Some(FileEntry {
            uri,
            text,
            line_index,
            defs,
            open_version,
        });
    }

    pub fn remove(&mut self, key: &str) -> bool {
        let Some(id) = self.ids.remove(key) else {
            return false;
        };
        self.unlink(id);
        self.slots[id as usize] = None;
        self.free.push(id);
        true
    }

    pub fn get(&self, key: &str) -> Option<&FileEntry> {
        let id = *self.ids.get(key)?;
        self.slots[id as usize].as_ref()
    }

    pub fn is_open(&self, key: &str) -> bool {
        self.get(key).is_some_and(|f| f.open_version.is_some())
    }

    pub fn class_at(&self, key: &str, offset: usize) -> Option<&ClassDef> {
        class_at(&self.get(key)?.defs, offset)
    }

    /// Every definition of `name`, ordered by (uri, position) so results are deterministic.
    pub fn defs_named(&self, name: &str) -> Vec<DefRef<'_>> {
        let mut out: Vec<DefRef<'_>> = self
            .by_name
            .get(name)
            .into_iter()
            .flatten()
            .filter_map(|&(file, idx)| {
                let file = self.slots[file as usize].as_ref()?;
                Some(DefRef {
                    file,
                    def: file.defs.get(idx as usize)?,
                })
            })
            .collect();
        out.sort_by(|a, b| {
            (a.file.uri.as_str(), a.def.name_span.start)
                .cmp(&(b.file.uri.as_str(), b.def.name_span.start))
        });
        out
    }

    pub fn file_count(&self) -> usize {
        self.ids.len()
    }

    fn unlink(&mut self, id: u32) {
        let Some(old) = self.slots[id as usize].as_ref() else {
            return;
        };
        let mut names: Vec<&str> = old.defs.iter().map(|d| &*d.name).collect();
        names.sort_unstable();
        names.dedup();
        for name in names {
            if let Some(v) = self.by_name.get_mut(name) {
                v.retain(|&(f, _)| f != id);
                if v.is_empty() {
                    self.by_name.remove(name);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn uri(s: &str) -> Uri {
        Uri::from_str(s).unwrap()
    }

    #[test]
    fn upsert_replaces_and_unlinks_stale_names() {
        let mut ws = Workspace::new();
        ws.upsert(uri("file:///w/a.css"), ".x {} .y {}".into(), None);
        ws.upsert(uri("file:///w/b.css"), ".x {}".into(), None);
        assert_eq!(ws.defs_named("x").len(), 2);

        ws.upsert(uri("file:///w/a.css"), ".z {}".into(), None);
        assert_eq!(ws.defs_named("x").len(), 1);
        assert!(ws.defs_named("y").is_empty());
        assert_eq!(ws.defs_named("z").len(), 1);
        assert_eq!(ws.file_count(), 2);
    }

    #[test]
    fn remove_frees_slot_for_reuse() {
        let mut ws = Workspace::new();
        ws.upsert(uri("file:///w/a.css"), ".x {}".into(), None);
        assert!(ws.remove("/w/a.css"));
        assert!(!ws.remove("/w/a.css"));
        assert!(ws.defs_named("x").is_empty());
        ws.upsert(uri("file:///w/b.css"), ".q {}".into(), None);
        assert_eq!(ws.defs_named("q").len(), 1);
        assert_eq!(ws.slots.len(), 1, "slot reused, not appended");
    }

    #[test]
    fn same_file_two_uri_spellings_is_one_entry() {
        let mut ws = Workspace::new();
        ws.upsert(uri("file:///w/a%20b.css"), ".x {}".into(), None);
        ws.upsert(uri("file:///w/a%20b.css"), ".y {}".into(), Some(1));
        assert_eq!(ws.file_count(), 1);
        assert!(ws.is_open("/w/a b.css"));
    }

    #[test]
    fn defs_are_ordered_by_uri_then_position() {
        let mut ws = Workspace::new();
        ws.upsert(uri("file:///w/b.css"), ".x {}".into(), None);
        ws.upsert(uri("file:///w/a.css"), ".x {}\n.x:hover {}".into(), None);
        let got: Vec<(String, u32)> = ws
            .defs_named("x")
            .iter()
            .map(|d| (d.file.uri.as_str().to_owned(), d.def.name_span.start))
            .collect();
        assert_eq!(
            got,
            vec![
                ("file:///w/a.css".into(), 1),
                ("file:///w/a.css".into(), 7),
                ("file:///w/b.css".into(), 1),
            ]
        );
    }
}
