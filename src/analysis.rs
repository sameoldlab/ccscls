//! Language-neutral analysis results.
//!
//! Every language handler turns file text into a flat list of occurrences of
//! class names: definitions (a class token in a CSS selector) and usages (a
//! class name in markup). A file may hold both kinds. Names are interned ids;
//! nothing here owns a string.

use crate::strings::NameId;
use std::ops::Range;

/// Half-open byte range into the UTF-8 source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn of(range: Range<usize>) -> Self {
        Self { start: range.start as u32, end: range.end as u32 }
    }

    pub fn range(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// A class token in a CSS selector.
    Definition,
    /// A class name in markup.
    Usage,
}

#[derive(Debug, Clone)]
pub struct Occurrence {
    pub role: Role,
    /// The unescaped name, as it would be written in an HTML `class` attribute.
    pub name: NameId,
    /// The token exactly as written in the source (escapes intact, no leading `.`).
    pub name_span: Span,
    /// Definitions: the enclosing style rule, used for hover. Usages: same as `name_span`.
    pub rule_span: Span,
    /// This occurrence's at-rule chain is `FileAnalysis::contexts[context_start..][..context_len]`.
    pub context_start: u32,
    pub context_len: u32,
}

#[derive(Debug, Default)]
pub struct FileAnalysis {
    /// Sorted by `name_span.start` once `finish` has run.
    pub occurrences: Vec<Occurrence>,
    /// Flat storage for at-rule chains (outermost first), shared between occurrences.
    pub contexts: Vec<NameId>,
}

impl FileAnalysis {
    pub fn add(&mut self, role: Role, name: NameId, name_span: Span, rule_span: Span, context: &[NameId]) {
        let context_start = self.context_start_for(context);
        self.occurrences.push(Occurrence {
            role,
            name,
            name_span,
            rule_span,
            context_start,
            context_len: context.len() as u32,
        });
    }

    pub fn context_of(&self, occurrence: &Occurrence) -> &[NameId] {
        let start = occurrence.context_start as usize;
        self.contexts
            .get(start..start + occurrence.context_len as usize)
            .unwrap_or(&[])
    }

    /// Restore source order. Handlers may emit in any order.
    pub fn finish(&mut self) {
        let sorted = self
            .occurrences
            .windows(2)
            .all(|pair| pair[0].name_span.start <= pair[1].name_span.start);
        if !sorted {
            self.occurrences.sort_by_key(|occurrence| occurrence.name_span.start);
        }
    }

    /// Index of the occurrence under `offset` (end-inclusive, so a cursor right
    /// after the last character still counts). Requires `finish` to have run.
    pub fn occurrence_at(&self, offset: usize) -> Option<usize> {
        let starting_at_or_before = self
            .occurrences
            .partition_point(|occurrence| occurrence.name_span.start as usize <= offset);
        let index = starting_at_or_before.checked_sub(1)?;
        let candidate = self.occurrences.get(index)?;
        (offset <= candidate.name_span.end as usize).then_some(index)
    }

    /// Reuse the previous occurrence's chain when identical (neighbours in the
    /// same `@media` block share it); otherwise append.
    fn context_start_for(&mut self, context: &[NameId]) -> u32 {
        if let Some(previous) = self.occurrences.last() {
            if self.context_of(previous) == context {
                return previous.context_start;
            }
        }
        let start = self.contexts.len() as u32;
        self.contexts.extend_from_slice(context);
        start
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strings::Strings;

    fn analysis_with(names: &[(&str, u32, u32)]) -> (FileAnalysis, Strings) {
        let mut strings = Strings::new();
        let mut analysis = FileAnalysis::default();
        for &(name, start, end) in names {
            let id = strings.intern(name);
            let span = Span { start, end };
            analysis.add(Role::Definition, id, span, span, &[]);
        }
        analysis.finish();
        (analysis, strings)
    }

    #[test]
    fn occurrence_at_is_end_inclusive() {
        let (analysis, _) = analysis_with(&[("ab", 1, 3), ("c", 8, 9)]);
        assert_eq!(analysis.occurrence_at(0), None);
        assert_eq!(analysis.occurrence_at(1), Some(0));
        assert_eq!(analysis.occurrence_at(3), Some(0));
        assert_eq!(analysis.occurrence_at(4), None);
        assert_eq!(analysis.occurrence_at(8), Some(1));
        assert_eq!(analysis.occurrence_at(9), Some(1));
        assert_eq!(analysis.occurrence_at(10), None);
        assert_eq!(FileAnalysis::default().occurrence_at(0), None);
    }

    #[test]
    fn finish_sorts_by_position() {
        let (analysis, strings) = analysis_with(&[("late", 20, 24), ("early", 1, 6)]);
        let names: Vec<&str> = analysis.occurrences.iter().map(|occurrence| strings.get(occurrence.name)).collect();
        assert_eq!(names, ["early", "late"]);
    }

    #[test]
    fn identical_neighbouring_context_chains_are_stored_once() {
        let mut strings = Strings::new();
        let media = strings.intern("@media (min-width: 1px)");
        let layer = strings.intern("@layer base");
        let mut analysis = FileAnalysis::default();
        let span = Span { start: 0, end: 1 };
        let name = strings.intern("x");
        analysis.add(Role::Definition, name, span, span, &[media]);
        analysis.add(Role::Definition, name, span, span, &[media]);
        analysis.add(Role::Definition, name, span, span, &[layer, media]);
        analysis.add(Role::Definition, name, span, span, &[]);
        assert_eq!(analysis.contexts, vec![media, layer, media]);
        assert_eq!(analysis.context_of(&analysis.occurrences[1]), &[media]);
        assert_eq!(analysis.context_of(&analysis.occurrences[2]), &[layer, media]);
        assert!(analysis.context_of(&analysis.occurrences[3]).is_empty());
    }
}
