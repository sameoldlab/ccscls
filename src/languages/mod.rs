//! The language seam.
//!
//! Everything specific to a file type lives in one module here (`css`, `html`,
//! and later `jsx`, `svelte`, `vue`). The rest of the crate never matches on a
//! file type: it asks `Language` to detect, analyze, find the completion
//! context, and render a class name. Adding a language means adding a module
//! and the arms below, nothing else.

pub mod css;
pub mod html;
pub mod svelte;

use crate::analysis::{FileAnalysis, Import, Role, Span};
use crate::strings::Strings;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Css,
    Html,
    Svelte,
}

pub const ALL: [Language; 3] = [Language::Css, Language::Html, Language::Svelte];

/// The class-name token under the cursor, for completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassContext {
    /// The whole token the cursor is in (empty when between tokens).
    pub replace: Span,
    /// Token text before the cursor.
    pub prefix: Span,
}

/// A stretch of a file written in another language (CSS inside `<style>`).
/// The handler that finds it reports it; the core analyzes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub language: Language,
    pub span: Span,
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
        Self {
            css: css::CssIndexer::new(),
        }
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
            if let Some(language) = ALL
                .into_iter()
                .find(|language| language.language_ids().contains(&id))
            {
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

    /// Fill `out` with this file's occurrences and imports, interning names into
    /// `strings`. Embedded regions are analyzed here and their definitions are
    /// marked private to the file.
    pub fn analyze(
        self,
        engines: &mut Engines,
        text: &str,
        strings: &mut Strings,
        out: &mut FileAnalysis,
    ) {
        let regions = match self {
            Self::Css => {
                css::analyze(&mut engines.css, text, strings, out);
                Vec::new()
            }
            Self::Html => html::analyze(text, strings, out),
            Self::Svelte => svelte::analyze(text, strings, out),
        };
        for region in regions {
            region
                .language
                .analyze_region(engines, text, region.span, strings, out);
        }
        out.finish();
    }

    /// Analyze `text[span]` as this language and merge the result into `out`,
    /// with spans moved back into the whole file's coordinates.
    fn analyze_region(
        self,
        engines: &mut Engines,
        text: &str,
        span: Span,
        strings: &mut Strings,
        out: &mut FileAnalysis,
    ) {
        let Some(region_text) = text.get(span.range()) else {
            return;
        };
        let mut inner = FileAnalysis::default();
        match self {
            Self::Css => css::analyze(&mut engines.css, region_text, strings, &mut inner),
            // Markup nested inside another file is not supported.
            Self::Html | Self::Svelte => return,
        }
        for occurrence in &inner.occurrences {
            let name_span = occurrence.name_span.shifted(span.start);
            let rule_span = occurrence.rule_span.shifted(span.start);
            let context = inner.context_of(occurrence);
            match occurrence.role {
                Role::Definition => out.add_private(
                    occurrence.role,
                    occurrence.name,
                    name_span,
                    rule_span,
                    context,
                ),
                Role::Usage => out.add(
                    occurrence.role,
                    occurrence.name,
                    name_span,
                    rule_span,
                    context,
                ),
            }
        }
        for import in &inner.imports {
            out.imports.push(Import {
                span: import.span.shifted(span.start),
                ..import.clone()
            });
        }
    }

    /// If `offset` is where a class name is being typed, the token being edited.
    pub fn class_context_at(self, text: &str, offset: usize) -> Option<ClassContext> {
        match self {
            Self::Css => None,
            Self::Html => html::class_context_at(text, offset),
            Self::Svelte => svelte::class_context_at(text, offset),
        }
    }

    /// Whether files of this language use class names (as opposed to defining them).
    pub fn is_markup(self) -> bool {
        matches!(self, Self::Html | Self::Svelte)
    }

    /// How `new_name` must be written in this language to replace `original_token`.
    pub fn render_name(self, original_token: &str, new_name: &str) -> String {
        match self {
            Self::Css => css::render_name(original_token, new_name),
            Self::Html | Self::Svelte => new_name.to_owned(),
        }
    }

    fn language_ids(self) -> &'static [&'static str] {
        match self {
            Self::Css => &["css"],
            Self::Html => &["html"],
            Self::Svelte => &["svelte"],
        }
    }

    fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Css => &["css"],
            Self::Html => &["html", "htm"],
            Self::Svelte => &["svelte"],
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
        assert_eq!(
            Language::detect(Some("html"), path("x.txt")),
            Some(Language::Html)
        );
        assert_eq!(
            Language::detect(None, path("a/b.HTM")),
            Some(Language::Html)
        );
        assert_eq!(
            Language::detect(Some("plaintext"), path("a.css")),
            Some(Language::Css)
        );
        assert_eq!(Language::detect(Some("javascript"), path("a.js")), None);
        assert_eq!(Language::detect(None, None), None);
        assert_eq!(
            Language::detect(Some("svelte"), None),
            Some(Language::Svelte)
        );
        assert_eq!(
            Language::detect(None, path("src/App.svelte")),
            Some(Language::Svelte)
        );
    }

    #[test]
    fn only_markup_languages_use_class_names() {
        assert!(Language::Html.is_markup());
        assert!(Language::Svelte.is_markup());
        assert!(!Language::Css.is_markup());
    }

    #[test]
    fn svelte_style_blocks_become_private_definitions() {
        let source = "<div class=\"x\"></div>\n<style>\n  .only { }\n</style>\n";
        let mut engines = Engines::new();
        let mut strings = Strings::new();
        let mut out = FileAnalysis::default();
        Language::Svelte.analyze(&mut engines, source, &mut strings, &mut out);
        let found: Vec<(Role, crate::analysis::Scope, &str)> = out
            .occurrences
            .iter()
            .map(|occurrence| {
                (
                    occurrence.role,
                    occurrence.scope,
                    strings.get(occurrence.name),
                )
            })
            .collect();
        assert_eq!(
            found,
            vec![
                (Role::Usage, crate::analysis::Scope::Shared, "x"),
                (Role::Definition, crate::analysis::Scope::Private, "only"),
            ]
        );
        assert!(
            out.open_world,
            "a component's CSS also comes from outside it"
        );
    }

    #[test]
    fn watch_globs_cover_every_extension() {
        assert_eq!(
            Language::watch_globs(),
            ["**/*.css", "**/*.html", "**/*.htm", "**/*.svelte"]
        );
    }

    #[test]
    fn style_blocks_become_private_definitions_at_the_right_offsets() {
        let source = "<p class=\"x\">\n<style>\n.only { }\n@import \"extra.css\";\n</style>";
        let mut engines = Engines::new();
        let mut strings = Strings::new();
        let mut out = FileAnalysis::default();
        Language::Html.analyze(&mut engines, source, &mut strings, &mut out);

        let found: Vec<(Role, crate::analysis::Scope, &str, &str)> = out
            .occurrences
            .iter()
            .map(|occurrence| {
                (
                    occurrence.role,
                    occurrence.scope,
                    strings.get(occurrence.name),
                    source.get(occurrence.name_span.range()).unwrap_or("?"),
                )
            })
            .collect();
        assert_eq!(
            found,
            vec![
                (Role::Usage, crate::analysis::Scope::Shared, "x", "x"),
                (
                    Role::Definition,
                    crate::analysis::Scope::Private,
                    "only",
                    "only"
                ),
            ]
        );
        let imports: Vec<&str> = out
            .imports
            .iter()
            .map(|import| source.get(import.span.range()).unwrap_or("?"))
            .collect();
        assert_eq!(imports, ["extra.css"]);
    }

    #[test]
    fn html_names_are_written_plainly() {
        assert_eq!(Language::Html.render_name("sm:flex", "md:flex"), "md:flex");
    }
}
