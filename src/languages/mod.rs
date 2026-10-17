//! The language seam.
//!
//! Everything specific to a file type lives in one module here (`css`, `html`,
//! and later `jsx`, `svelte`, `vue`). The rest of the crate never matches on a
//! file type: it asks `Language` to detect, analyze, find the completion
//! context, and render a class name. Adding a language means adding a module
//! and the arms below, nothing else.

pub mod css;
pub mod html;

use crate::analysis::{FileAnalysis, Span};
use crate::strings::Strings;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Css,
    Html,
}

pub const ALL: [Language; 2] = [Language::Css, Language::Html];

/// The class-name token under the cursor, for completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassContext {
    /// The whole token the cursor is in (empty when between tokens).
    pub replace: Span,
    /// Token text before the cursor.
    pub prefix: Span,
}

/// Per-language parsing state that is expensive to create and reused across files.
pub struct Engines {
    css: css::CssIndexer,
}

impl Default for Engines {
    fn default() -> Self {
        Self::new()
    }
}

impl Engines {
    pub fn new() -> Self {
        Self { css: css::CssIndexer::new() }
    }

    /// Why a language cannot analyze files at all (a grammar that failed to load), if so.
    pub fn load_error(&self) -> Option<&str> {
        self.css.load_error()
    }
}

impl Language {
    /// Decide from the editor's language id first, then the file extension.
    pub fn detect(language_id: Option<&str>, path: Option<&Path>) -> Option<Self> {
        if let Some(id) = language_id {
            if let Some(language) = ALL.into_iter().find(|language| language.language_ids().contains(&id)) {
                return Some(language);
            }
        }
        let extension = path?.extension()?.to_str()?.to_ascii_lowercase();
        ALL.into_iter()
            .find(|language| language.extensions().contains(&extension.as_str()))
    }

    /// Glob patterns for the file watcher, one per extension of every language.
    pub fn watch_globs() -> Vec<String> {
        ALL.into_iter()
            .flat_map(|language| language.extensions().iter().copied())
            .map(|extension| format!("**/*.{extension}"))
            .collect()
    }

    /// Fill `out` with this file's occurrences, interning names into `strings`.
    pub fn analyze(self, engines: &mut Engines, text: &str, strings: &mut Strings, out: &mut FileAnalysis) {
        match self {
            Self::Css => css::analyze(&mut engines.css, text, strings, out),
            Self::Html => html::analyze(text, strings, out),
        }
        out.finish();
    }

    /// If `offset` is where a class name is being typed, the token being edited.
    pub fn class_context_at(self, text: &str, offset: usize) -> Option<ClassContext> {
        match self {
            Self::Css => None,
            Self::Html => html::class_context_at(text, offset),
        }
    }

    /// How `new_name` must be written in this language to replace `original_token`.
    pub fn render_name(self, original_token: &str, new_name: &str) -> String {
        match self {
            Self::Css => css::render_name(original_token, new_name),
            Self::Html => new_name.to_owned(),
        }
    }

    fn language_ids(self) -> &'static [&'static str] {
        match self {
            Self::Css => &["css"],
            Self::Html => &["html"],
        }
    }

    fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Css => &["css"],
            Self::Html => &["html", "htm"],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> Option<&Path> {
        Some(Path::new(text))
    }

    #[test]
    fn detection_prefers_language_id_then_extension() {
        assert_eq!(Language::detect(Some("css"), None), Some(Language::Css));
        assert_eq!(Language::detect(Some("html"), path("x.txt")), Some(Language::Html));
        assert_eq!(Language::detect(None, path("a/b.HTM")), Some(Language::Html));
        assert_eq!(Language::detect(Some("plaintext"), path("a.css")), Some(Language::Css));
        assert_eq!(Language::detect(Some("javascript"), path("a.js")), None);
        assert_eq!(Language::detect(None, None), None);
    }

    #[test]
    fn watch_globs_cover_every_extension() {
        assert_eq!(Language::watch_globs(), ["**/*.css", "**/*.html", "**/*.htm"]);
    }

    #[test]
    fn html_names_are_written_plainly() {
        assert_eq!(Language::Html.render_name("sm:flex", "md:flex"), "md:flex");
    }
}
