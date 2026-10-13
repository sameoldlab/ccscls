//! The set of indexed files (CSS definitions, markup usages) and name maps.

use crate::css::{ClassDef, CssIndexer, Span, class_at};
use crate::line_index::LineIndex;
use crate::markup::{self, Usage};
use crate::uri::file_key;
use lsp_types::Uri;
use rustc_hash::FxHashMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Css,
    Html,
}

impl FileKind {
    /// Decide from the editor's language id first, then the file extension.
    pub fn detect(language_id: Option<&str>, path: Option<&Path>) -> Option<Self> {
        match language_id {
            Some("css") => return Some(Self::Css),
            Some("html") => return Some(Self::Html),
            _ => {}
        }
        let ext = path?.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "css" => Some(Self::Css),
            "html" | "htm" => Some(Self::Html),
            _ => None,
        }
    }
}

pub struct FileEntry {
    /// The URI to report locations with (the client's spelling wins over ours).
    pub uri: Uri,
    pub text: String,
    pub line_index: LineIndex,
    pub kind: FileKind,
    /// CSS files: class tokens in selectors. Sorted by position.
    pub defs: Vec<ClassDef>,
    /// Markup files: class names in `class` attributes. Sorted by position.
    pub usages: Vec<Usage>,
    /// `Some(version)` while the client has the document open; the buffer then
    /// takes precedence over what is on disk.
    pub open_version: Option<i32>,
}

pub struct DefRef<'a> {
    pub file: &'a FileEntry,
    pub def: &'a ClassDef,
}

pub struct UsageRef<'a> {
    pub file: &'a FileEntry,
    pub usage: &'a Usage,
}

/// A class name under the cursor, from either side.
pub struct Target<'a> {
    pub file: &'a FileEntry,
    pub name: &'a str,
    pub span: Span,
}

type NameMap = FxHashMap<Box<str>, Vec<(u32, u32)>>;

#[derive(Default)]
pub struct Workspace {
    indexer: CssIndexer,
    slots: Vec<Option<FileEntry>>,
    free: Vec<u32>,
    ids: FxHashMap<String, u32>,
    /// class name -> (file slot, index into that file's `defs`)
    def_names: NameMap,
    /// class name -> (file slot, index into that file's `usages`)
    use_names: NameMap,
}

impl Workspace {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace a file, re-indexing it.
    pub fn upsert(&mut self, uri: Uri, text: String, kind: FileKind, open_version: Option<i32>) {
        let key = file_key(&uri);
        let (defs, usages) = match kind {
            FileKind::Css => (self.indexer.index(&text), Vec::new()),
            FileKind::Html => (Vec::new(), markup::usages(&text)),
        };
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
            self.def_names.entry(d.name.clone()).or_default().push((id, i as u32));
        }
        for (i, u) in usages.iter().enumerate() {
            self.use_names.entry(u.name.clone()).or_default().push((id, i as u32));
        }
        self.slots[id as usize] = Some(FileEntry {
            uri,
            text,
            line_index,
            kind,
            defs,
            usages,
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

    /// The class name under `offset`, whichever kind of file it is in.
    pub fn target_at(&self, key: &str, offset: usize) -> Option<Target<'_>> {
        let file = self.get(key)?;
        match file.kind {
            FileKind::Css => {
                let d = class_at(&file.defs, offset)?;
                Some(Target { file, name: &d.name, span: d.name_span })
            }
            FileKind::Html => {
                let u = markup::usage_at(&file.usages, offset)?;
                Some(Target { file, name: &u.name, span: u.span })
            }
        }
    }

    /// Every CSS occurrence of `name`, ordered by (uri, position).
    pub fn defs_named(&self, name: &str) -> Vec<DefRef<'_>> {
        let mut out: Vec<DefRef<'_>> = self
            .def_names
            .get(name)
            .into_iter()
            .flatten()
            .filter_map(|&(f, i)| {
                let file = self.slots[f as usize].as_ref()?;
                Some(DefRef { file, def: file.defs.get(i as usize)? })
            })
            .collect();
        out.sort_by_key(|d| (d.file.uri.as_str(), d.def.name_span.start));
        out
    }

    /// Every markup usage of `name`, ordered by (uri, position).
    pub fn usages_named(&self, name: &str) -> Vec<UsageRef<'_>> {
        let mut out: Vec<UsageRef<'_>> = self
            .use_names
            .get(name)
            .into_iter()
            .flatten()
            .filter_map(|&(f, i)| {
                let file = self.slots[f as usize].as_ref()?;
                Some(UsageRef { file, usage: file.usages.get(i as usize)? })
            })
            .collect();
        out.sort_by_key(|u| (u.file.uri.as_str(), u.usage.span.start));
        out
    }

    /// Distinct class names defined in CSS with their rule counts, sorted by name.
    pub fn class_names(&self) -> Vec<(&str, usize)> {
        let mut v: Vec<(&str, usize)> = self.def_names.iter().map(|(k, v)| (&**k, v.len())).collect();
        v.sort_unstable();
        v
    }

    pub fn file_count(&self) -> usize {
        self.ids.len()
    }

    /// Why CSS indexing is disabled, if the grammar failed to load.
    pub fn css_load_error(&self) -> Option<&str> {
        self.indexer.load_error()
    }

    fn unlink(&mut self, id: u32) {
        let Some(old) = self.slots[id as usize].as_ref() else {
            return;
        };
        let mut def_names: Vec<&str> = old.defs.iter().map(|d| &*d.name).collect();
        let mut use_names: Vec<&str> = old.usages.iter().map(|u| &*u.name).collect();
        for names in [&mut def_names, &mut use_names] {
            names.sort_unstable();
            names.dedup();
        }
        for (map, names) in [(&mut self.def_names, def_names), (&mut self.use_names, use_names)] {
            for name in names {
                if let Some(v) = map.get_mut(name) {
                    v.retain(|&(f, _)| f != id);
                    if v.is_empty() {
                        map.remove(name);
                    }
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
        ws.upsert(uri("file:///w/a.css"), ".x {} .y {}".into(), FileKind::Css, None);
        ws.upsert(uri("file:///w/b.css"), ".x {}".into(), FileKind::Css, None);
        assert_eq!(ws.defs_named("x").len(), 2);

        ws.upsert(uri("file:///w/a.css"), ".z {}".into(), FileKind::Css, None);
        assert_eq!(ws.defs_named("x").len(), 1);
        assert!(ws.defs_named("y").is_empty());
        assert_eq!(ws.defs_named("z").len(), 1);
        assert_eq!(ws.file_count(), 2);
    }

    #[test]
    fn usages_and_defs_are_separate_maps() {
        let mut ws = Workspace::new();
        ws.upsert(uri("file:///w/a.css"), ".x {}".into(), FileKind::Css, None);
        ws.upsert(uri("file:///w/i.html"), r#"<p class="x y">"#.into(), FileKind::Html, None);
        assert_eq!(ws.defs_named("x").len(), 1);
        assert_eq!(ws.usages_named("x").len(), 1);
        assert_eq!(ws.usages_named("y").len(), 1);
        assert!(ws.defs_named("y").is_empty());
        assert_eq!(ws.class_names(), vec![("x", 1)]);

        ws.upsert(uri("file:///w/i.html"), r#"<p class="y">"#.into(), FileKind::Html, None);
        assert!(ws.usages_named("x").is_empty());
    }

    #[test]
    fn remove_frees_slot_for_reuse() {
        let mut ws = Workspace::new();
        ws.upsert(uri("file:///w/a.css"), ".x {}".into(), FileKind::Css, None);
        assert!(ws.remove("/w/a.css"));
        assert!(!ws.remove("/w/a.css"));
        assert!(ws.defs_named("x").is_empty());
        ws.upsert(uri("file:///w/b.css"), ".q {}".into(), FileKind::Css, None);
        assert_eq!(ws.defs_named("q").len(), 1);
        assert_eq!(ws.slots.len(), 1, "slot reused, not appended");
    }

    #[test]
    fn same_file_two_uri_spellings_is_one_entry() {
        let mut ws = Workspace::new();
        ws.upsert(uri("file:///w/a%20b.css"), ".x {}".into(), FileKind::Css, None);
        ws.upsert(uri("file:///w/a%20b.css"), ".y {}".into(), FileKind::Css, Some(1));
        assert_eq!(ws.file_count(), 1);
        assert!(ws.is_open("/w/a b.css"));
    }

    #[test]
    fn defs_are_ordered_by_uri_then_position() {
        let mut ws = Workspace::new();
        ws.upsert(uri("file:///w/b.css"), ".x {}".into(), FileKind::Css, None);
        ws.upsert(uri("file:///w/a.css"), ".x {}\n.x:hover {}".into(), FileKind::Css, None);
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

    #[test]
    fn kind_detection() {
        fn p(s: &str) -> Option<&Path> {
            Some(Path::new(s))
        }
        assert_eq!(FileKind::detect(Some("css"), None), Some(FileKind::Css));
        assert_eq!(FileKind::detect(Some("html"), p("x.txt")), Some(FileKind::Html));
        assert_eq!(FileKind::detect(None, p("a/b.HTM")), Some(FileKind::Html));
        assert_eq!(FileKind::detect(Some("plaintext"), p("a.css")), Some(FileKind::Css));
        assert_eq!(FileKind::detect(Some("javascript"), p("a.js")), None);
        assert_eq!(FileKind::detect(None, None), None);
    }
}
