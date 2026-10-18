//! The set of indexed files and an index from class name to its occurrences.
//!
//! No references escape this module as long-lived values. Files and
//! occurrences are named by small `Copy` handles that carry a generation, so a
//! handle to a file that was since re-indexed or removed resolves to `None`
//! instead of to someone else's data.
//!
//! The per-name index is intrusive: every occurrence carries `previous` and
//! `next` links to the other occurrences with the same name and role, and
//! `NameLists` holds each list's head. Re-indexing a file unlinks its old
//! occurrences in constant time each, with no per-name vectors to maintain.

mod visibility;

use crate::analysis::{FileAnalysis, Occurrence, Role, Scope};
use crate::languages::{Engines, Language};
use crate::line_index::LineIndex;
use crate::strings::{NameId, Strings};
use crate::uri::file_key;
use lsp_types::Uri;
use rustc_hash::FxHashMap;
use std::path::PathBuf;

pub use visibility::{
    Candidate, Classification, Reach, UsageVerdict, UsageVerdicts, Verdict, VisibleNames,
};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FileId {
    index: u32,
    generation: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct OccurrenceId {
    file: FileId,
    index: u32,
}

impl OccurrenceId {
    /// The file this occurrence is in.
    pub fn file(self) -> FileId {
        self.file
    }
}

/// Links in one name's list of occurrences.
#[derive(Clone, Copy, Default)]
struct Links {
    previous: Option<OccurrenceId>,
    next: Option<OccurrenceId>,
}

#[derive(Clone, Copy, Default)]
struct List {
    head: Option<OccurrenceId>,
    count: u32,
}

#[derive(Clone, Copy, Default)]
struct NameLists {
    definitions: List,
    usages: List,
}

pub struct FileEntry {
    /// The URI to report locations with (the client's spelling wins over ours).
    pub uri: Uri,
    pub text: String,
    pub line_index: LineIndex,
    pub language: Language,
    pub analysis: FileAnalysis,
    /// `Some(version)` while the client has the document open; the buffer then
    /// takes precedence over what is on disk.
    pub open_version: Option<i32>,
    /// Parallel to `analysis.occurrences`.
    links: Vec<Links>,
}

struct Slot {
    generation: u32,
    entry: Option<FileEntry>,
}

pub struct Workspace {
    engines: Engines,
    strings: Strings,
    slots: Vec<Slot>,
    free_slots: Vec<u32>,
    ids: FxHashMap<String, FileId>,
    /// Indexed by `NameId`. Private definitions (CSS embedded in a document)
    /// are not in these lists: only their own file can see them.
    names: Vec<NameLists>,
    /// Workspace folders, for resolving root-relative URLs like `/css/site.css`.
    roots: Vec<PathBuf>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

impl Workspace {
    pub fn new() -> Self {
        Self {
            engines: Engines::new(),
            strings: Strings::new(),
            slots: Vec::new(),
            free_slots: Vec::new(),
            ids: FxHashMap::default(),
            names: Vec::new(),
            roots: Vec::new(),
        }
    }

    /// Insert or replace a file, re-indexing it.
    pub fn upsert(
        &mut self,
        uri: Uri,
        text: String,
        language: Language,
        open_version: Option<i32>,
    ) {
        let key = file_key(&uri);
        let mut analysis = FileAnalysis::default();
        language.analyze(&mut self.engines, &text, &mut self.strings, &mut analysis);
        let line_index = LineIndex::new(&text);

        let slot_index = match self.ids.get(&key).copied() {
            Some(existing) => {
                self.unlink_file(existing);
                existing.index
            }
            None => self.allocate_slot(),
        };
        let Some(slot) = self.slots.get_mut(slot_index as usize) else {
            return;
        };
        slot.generation = slot.generation.wrapping_add(1);
        let file = FileId {
            index: slot_index,
            generation: slot.generation,
        };
        let occurrence_count = analysis.occurrences.len();
        slot.entry = Some(FileEntry {
            uri,
            text,
            line_index,
            language,
            analysis,
            open_version,
            links: vec![Links::default(); occurrence_count],
        });
        self.ids.insert(key, file);
        for index in 0..occurrence_count {
            self.link(OccurrenceId {
                file,
                index: index as u32,
            });
        }
    }

    pub fn remove(&mut self, key: &str) -> bool {
        let Some(file) = self.ids.remove(key) else {
            return false;
        };
        self.unlink_file(file);
        if let Some(slot) = self.slots.get_mut(file.index as usize) {
            slot.generation = slot.generation.wrapping_add(1);
            slot.entry = None;
            self.free_slots.push(file.index);
        }
        true
    }

    pub fn file(&self, id: FileId) -> Option<&FileEntry> {
        let slot = self.slots.get(id.index as usize)?;
        if slot.generation != id.generation {
            return None;
        }
        slot.entry.as_ref()
    }

    /// The current handle for the file with this key.
    pub fn file_id(&self, key: &str) -> Option<FileId> {
        self.ids
            .get(key)
            .copied()
            .filter(|id| self.file(*id).is_some())
    }

    pub fn file_by_key(&self, key: &str) -> Option<&FileEntry> {
        self.file(*self.ids.get(key)?)
    }

    /// Handles of every file the client currently has open.
    pub fn open_file_ids(&self) -> Vec<FileId> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| {
                slot.entry
                    .as_ref()
                    .is_some_and(|entry| entry.open_version.is_some())
            })
            .map(|(index, slot)| FileId {
                index: index as u32,
                generation: slot.generation,
            })
            .collect()
    }

    /// The id of an interned name, if it has been seen anywhere.
    pub fn name_id(&self, text: &str) -> Option<NameId> {
        self.strings.lookup(text)
    }

    pub fn is_open(&self, key: &str) -> bool {
        self.file_by_key(key)
            .is_some_and(|file| file.open_version.is_some())
    }

    pub fn file_count(&self) -> usize {
        self.ids.len()
    }

    /// Why analysis is disabled for some language, if a grammar failed to load.
    pub fn load_error(&self) -> Option<&str> {
        self.engines.load_error()
    }

    pub fn resolve(&self, id: OccurrenceId) -> Option<(&FileEntry, &Occurrence)> {
        let file = self.file(id.file)?;
        let occurrence = file.analysis.occurrences.get(id.index as usize)?;
        Some((file, occurrence))
    }

    /// The occurrence under `offset` in the file with this key, whichever role it has.
    pub fn occurrence_at(&self, key: &str, offset: usize) -> Option<OccurrenceId> {
        let file = *self.ids.get(key)?;
        let index = self.file(file)?.analysis.occurrence_at(offset)?;
        Some(OccurrenceId {
            file,
            index: index as u32,
        })
    }

    /// Every occurrence of `name` with this role, ordered by (uri, position).
    pub fn occurrences_named(&self, role: Role, name: NameId) -> Vec<OccurrenceId> {
        let Some(list) = self.list(name, role) else {
            return Vec::new();
        };
        let mut found = Vec::with_capacity(list.count as usize);
        let mut cursor = list.head;
        while let Some(id) = cursor {
            found.push(id);
            // A corrupted list must never loop forever.
            if found.len() > list.count as usize {
                break;
            }
            cursor = self.links_of(id).and_then(|links| links.next);
        }
        self.sort_by_location(&mut found);
        found
    }

    /// Distinct names defined in CSS, with their rule counts, sorted by name.
    pub fn class_names(&self) -> Vec<(NameId, u32)> {
        let mut names: Vec<(NameId, u32)> = self
            .names
            .iter()
            .enumerate()
            .filter(|(_, lists)| lists.definitions.count > 0)
            .map(|(index, lists)| (NameId::from_index(index), lists.definitions.count))
            .collect();
        names.sort_by(|(left, _), (right, _)| self.name(*left).cmp(self.name(*right)));
        names
    }

    pub fn name(&self, id: NameId) -> &str {
        self.strings.get(id)
    }

    /// The at-rule chain around an occurrence, outermost first.
    pub fn context_names(&self, file: &FileEntry, occurrence: &Occurrence) -> Vec<&str> {
        file.analysis
            .context_of(occurrence)
            .iter()
            .map(|id| self.strings.get(*id))
            .collect()
    }

    /// Order by (uri, position) so results are deterministic.
    fn sort_by_location(&self, ids: &mut [OccurrenceId]) {
        ids.sort_by_key(|id| {
            self.resolve(*id)
                .map(|(file, occurrence)| (file.uri.as_str(), occurrence.name_span.start))
        });
    }

    fn allocate_slot(&mut self) -> u32 {
        if let Some(index) = self.free_slots.pop() {
            return index;
        }
        self.slots.push(Slot {
            generation: 0,
            entry: None,
        });
        (self.slots.len() - 1) as u32
    }

    fn list(&self, name: NameId, role: Role) -> Option<List> {
        let lists = self.names.get(name.index())?;
        Some(match role {
            Role::Definition => lists.definitions,
            Role::Usage => lists.usages,
        })
    }

    fn list_mut(&mut self, name: NameId, role: Role) -> Option<&mut List> {
        let lists = self.names.get_mut(name.index())?;
        Some(match role {
            Role::Definition => &mut lists.definitions,
            Role::Usage => &mut lists.usages,
        })
    }

    fn links_of(&self, id: OccurrenceId) -> Option<Links> {
        self.file(id.file)?.links.get(id.index as usize).copied()
    }

    fn update_links(&mut self, id: OccurrenceId, change: impl FnOnce(&mut Links)) {
        let Some(slot) = self.slots.get_mut(id.file.index as usize) else {
            return;
        };
        if slot.generation != id.file.generation {
            return;
        }
        if let Some(links) = slot
            .entry
            .as_mut()
            .and_then(|entry| entry.links.get_mut(id.index as usize))
        {
            change(links);
        }
    }

    /// Put `id` at the head of its name's list.
    fn link(&mut self, id: OccurrenceId) {
        let Some((_, occurrence)) = self.resolve(id) else {
            return;
        };
        if occurrence.role == Role::Definition && occurrence.scope == Scope::Private {
            return;
        }
        let (role, name) = (occurrence.role, occurrence.name);
        if self.names.len() <= name.index() {
            self.names.resize(name.index() + 1, NameLists::default());
        }
        let Some(list) = self.list_mut(name, role) else {
            return;
        };
        let old_head = list.head;
        list.head = Some(id);
        list.count += 1;
        self.update_links(id, |links| {
            *links = Links {
                previous: None,
                next: old_head,
            }
        });
        if let Some(old_head) = old_head {
            self.update_links(old_head, |links| links.previous = Some(id));
        }
    }

    /// Remove `id` from its name's list.
    fn unlink(&mut self, id: OccurrenceId) {
        let Some((_, occurrence)) = self.resolve(id) else {
            return;
        };
        if occurrence.role == Role::Definition && occurrence.scope == Scope::Private {
            return;
        }
        let (role, name) = (occurrence.role, occurrence.name);
        let Some(links) = self.links_of(id) else {
            return;
        };
        match links.previous {
            Some(previous) => self.update_links(previous, |other| other.next = links.next),
            None => {
                if let Some(list) = self.list_mut(name, role) {
                    list.head = links.next;
                }
            }
        }
        if let Some(next) = links.next {
            self.update_links(next, |other| other.previous = links.previous);
        }
        if let Some(list) = self.list_mut(name, role) {
            list.count = list.count.saturating_sub(1);
        }
        self.update_links(id, |own| *own = Links::default());
    }

    fn unlink_file(&mut self, file: FileId) {
        let count = self
            .file(file)
            .map_or(0, |entry| entry.analysis.occurrences.len());
        for index in 0..count {
            self.unlink(OccurrenceId {
                file,
                index: index as u32,
            });
        }
    }
}

#[cfg(test)]
impl Workspace {
    /// Walk every list both ways and compare with a scan of all files.
    fn assert_consistent(&self) {
        for (name_index, lists) in self.names.iter().enumerate() {
            let name = NameId::from_index(name_index);
            for (role, list) in [
                (Role::Definition, lists.definitions),
                (Role::Usage, lists.usages),
            ] {
                let mut walked = Vec::new();
                let mut previous = None;
                let mut cursor = list.head;
                while let Some(id) = cursor {
                    let (_, occurrence) = self.resolve(id).expect("list entries resolve");
                    assert_eq!(
                        (occurrence.name, occurrence.role),
                        (name, role),
                        "wrong list for {id:?}"
                    );
                    let links = self.links_of(id).expect("links exist");
                    assert_eq!(links.previous, previous, "broken previous link at {id:?}");
                    walked.push(id);
                    assert!(
                        walked.len() <= list.count as usize,
                        "list longer than its count"
                    );
                    previous = Some(id);
                    cursor = links.next;
                }
                assert_eq!(
                    walked.len(),
                    list.count as usize,
                    "count mismatch for name {name_index}"
                );

                let mut scanned = Vec::new();
                for (&_, &file) in self.ids.iter() {
                    let entry = self.file(file).expect("live file");
                    for (index, occurrence) in entry.analysis.occurrences.iter().enumerate() {
                        let is_private_definition = occurrence.role == Role::Definition
                            && occurrence.scope == Scope::Private;
                        if occurrence.name == name
                            && occurrence.role == role
                            && !is_private_definition
                        {
                            scanned.push(OccurrenceId {
                                file,
                                index: index as u32,
                            });
                        }
                    }
                }
                let mut walked_sorted = walked.clone();
                walked_sorted.sort_by_key(|id| (id.file.index, id.index));
                scanned.sort_by_key(|id| (id.file.index, id.index));
                assert_eq!(
                    walked_sorted, scanned,
                    "list differs from a scan for name {name_index}"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn uri(text: &str) -> Uri {
        Uri::from_str(text).expect("valid test uri")
    }

    fn names_of(workspace: &Workspace, role: Role, name: &str) -> usize {
        workspace
            .strings
            .lookup(name)
            .map_or(0, |id| workspace.occurrences_named(role, id).len())
    }

    #[test]
    fn upsert_replaces_and_unlinks_stale_names() {
        let mut workspace = Workspace::new();
        workspace.upsert(
            uri("file:///w/a.css"),
            ".x {} .y {}".into(),
            Language::Css,
            None,
        );
        workspace.upsert(uri("file:///w/b.css"), ".x {}".into(), Language::Css, None);
        assert_eq!(names_of(&workspace, Role::Definition, "x"), 2);

        workspace.upsert(uri("file:///w/a.css"), ".z {}".into(), Language::Css, None);
        assert_eq!(names_of(&workspace, Role::Definition, "x"), 1);
        assert_eq!(names_of(&workspace, Role::Definition, "y"), 0);
        assert_eq!(names_of(&workspace, Role::Definition, "z"), 1);
        assert_eq!(workspace.file_count(), 2);
        workspace.assert_consistent();
    }

    #[test]
    fn definitions_and_usages_are_separate_lists() {
        let mut workspace = Workspace::new();
        workspace.upsert(uri("file:///w/a.css"), ".x {}".into(), Language::Css, None);
        workspace.upsert(
            uri("file:///w/i.html"),
            r#"<p class="x y">"#.into(),
            Language::Html,
            None,
        );
        assert_eq!(names_of(&workspace, Role::Definition, "x"), 1);
        assert_eq!(names_of(&workspace, Role::Usage, "x"), 1);
        assert_eq!(names_of(&workspace, Role::Usage, "y"), 1);
        assert_eq!(names_of(&workspace, Role::Definition, "y"), 0);
        let names: Vec<&str> = workspace
            .class_names()
            .iter()
            .map(|(id, _)| workspace.name(*id))
            .collect();
        assert_eq!(names, ["x"]);

        workspace.upsert(
            uri("file:///w/i.html"),
            r#"<p class="y">"#.into(),
            Language::Html,
            None,
        );
        assert_eq!(names_of(&workspace, Role::Usage, "x"), 0);
        workspace.assert_consistent();
    }

    #[test]
    fn private_definitions_stay_out_of_the_shared_lists() {
        let mut workspace = Workspace::new();
        let page = "<style>.only { }</style><p class=\"only\">";
        workspace.upsert(
            uri("file:///w/page.html"),
            page.into(),
            Language::Html,
            None,
        );
        assert_eq!(names_of(&workspace, Role::Definition, "only"), 0);
        assert_eq!(names_of(&workspace, Role::Usage, "only"), 1);
        assert!(workspace.class_names().is_empty());
        workspace.assert_consistent();

        workspace.upsert(
            uri("file:///w/page.html"),
            "<p>".into(),
            Language::Html,
            None,
        );
        workspace.assert_consistent();
        workspace.remove("/w/page.html");
        workspace.assert_consistent();
    }

    #[test]
    fn removing_a_file_frees_its_slot_and_invalidates_old_handles() {
        let mut workspace = Workspace::new();
        workspace.upsert(uri("file:///w/a.css"), ".x {}".into(), Language::Css, None);
        let old_handle = workspace
            .occurrence_at("/w/a.css", 1)
            .expect("class under cursor");
        assert!(workspace.remove("/w/a.css"));
        assert!(!workspace.remove("/w/a.css"));
        assert_eq!(names_of(&workspace, Role::Definition, "x"), 0);

        workspace.upsert(uri("file:///w/b.css"), ".q {}".into(), Language::Css, None);
        assert_eq!(workspace.slots.len(), 1, "slot reused, not appended");
        assert!(
            workspace.resolve(old_handle).is_none(),
            "stale handle must not reach the new file"
        );
        workspace.assert_consistent();
    }

    #[test]
    fn reindexing_a_file_invalidates_handles_into_its_old_contents() {
        let mut workspace = Workspace::new();
        workspace.upsert(uri("file:///w/a.css"), ".x {}".into(), Language::Css, None);
        let before = workspace
            .occurrence_at("/w/a.css", 1)
            .expect("class under cursor");
        workspace.upsert(
            uri("file:///w/a.css"),
            ".y {}".into(),
            Language::Css,
            Some(1),
        );
        assert!(workspace.resolve(before).is_none());
        assert!(
            workspace
                .occurrence_at("/w/a.css", 1)
                .and_then(|id| workspace.resolve(id))
                .is_some()
        );
    }

    #[test]
    fn same_file_two_uri_spellings_is_one_entry() {
        let mut workspace = Workspace::new();
        workspace.upsert(
            uri("file:///w/a%20b.css"),
            ".x {}".into(),
            Language::Css,
            None,
        );
        workspace.upsert(
            uri("file:///w/a%20b.css"),
            ".y {}".into(),
            Language::Css,
            Some(1),
        );
        assert_eq!(workspace.file_count(), 1);
        assert!(workspace.is_open("/w/a b.css"));
    }

    #[test]
    fn occurrences_are_ordered_by_uri_then_position() {
        let mut workspace = Workspace::new();
        workspace.upsert(uri("file:///w/b.css"), ".x {}".into(), Language::Css, None);
        workspace.upsert(
            uri("file:///w/a.css"),
            ".x {}\n.x:hover {}".into(),
            Language::Css,
            None,
        );
        let name = workspace.strings.lookup("x").expect("interned");
        let order: Vec<(String, u32)> = workspace
            .occurrences_named(Role::Definition, name)
            .iter()
            .filter_map(|id| workspace.resolve(*id))
            .map(|(file, occurrence)| (file.uri.as_str().to_owned(), occurrence.name_span.start))
            .collect();
        assert_eq!(
            order,
            vec![
                ("file:///w/a.css".to_owned(), 1),
                ("file:///w/a.css".to_owned(), 7),
                ("file:///w/b.css".to_owned(), 1),
            ]
        );
    }

    #[test]
    fn lists_stay_consistent_under_a_random_mix_of_edits() {
        // Deterministic sequence of upserts and removals across a few files
        // sharing a small vocabulary, so lists are long and heavily relinked.
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut next_number = move |bound: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % bound
        };
        let vocabulary = ["a", "b", "c", "d", "e"];
        let mut workspace = Workspace::new();
        for step in 0..400 {
            let file_number = next_number(6);
            let path = format!("file:///w/f{file_number}.css");
            if next_number(5) == 0 {
                workspace.remove(&format!("/w/f{file_number}.css"));
            } else {
                let mut source = String::new();
                for _ in 0..next_number(8) {
                    source.push_str(&format!(".{} {{}}\n", vocabulary[next_number(5) as usize]));
                }
                let language = if next_number(4) == 0 {
                    Language::Html
                } else {
                    Language::Css
                };
                if language == Language::Html {
                    source = format!(r#"<p class="{}">"#, vocabulary[next_number(5) as usize]);
                }
                workspace.upsert(uri(&path), source, language, None);
            }
            if step % 10 == 0 {
                workspace.assert_consistent();
            }
        }
        workspace.assert_consistent();
    }
}
