//! Which CSS a file can see, and how sure we are.
//!
//! Every class used in a file gets one of three verdicts:
//!
//! * **Verified**: a chain of resolved edges (`<link>`, `@import`) connects the
//!   file to a rule that defines the class, or the file defines it itself in a
//!   `<style>` block. The chain is reported so it can be shown and re-checked.
//! * **Ambient**: some stylesheet defines the class, but nothing proves it
//!   reaches this file.
//! * **Undefined**: no stylesheet defines it.
//!
//! Verified must be sound; ambient may be broad. A file is also marked
//! **open-world** when something that could add CSS to it is unknown (an
//! import we cannot resolve, a script, a fragment with no `<head>`), so the
//! caller never reports "unlinked" with more confidence than we have.
//!
//! Import targets are resolved when asked, by joining paths and looking the
//! result up among indexed files. Nothing is cached, so creating or deleting a
//! stylesheet needs no invalidation: the next question just finds a different answer.

use super::{FileEntry, FileId, Workspace};
use crate::analysis::{Import, Role, Scope};
use crate::strings::NameId;
use crate::uri::{percent_decode, uri_to_path};
use rustc_hash::FxHashSet;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Verified,
    Ambient,
    Undefined,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    pub verdict: Verdict,
    /// Verified: the files from the origin to the first one that defines the
    /// name, inclusive. Empty otherwise.
    pub chain: Vec<FileId>,
    /// Ambient: the stylesheets that define the name but are not reachable.
    pub defined_in: Vec<FileId>,
    /// Whether something that could add CSS to the origin is unknown to us.
    pub open_world: bool,
}

#[derive(Debug)]
struct Visit {
    file: FileId,
    /// Index of the visit that led here; `None` for the origin.
    parent: Option<usize>,
}

/// Every file reachable from an origin, nearest first.
#[derive(Debug)]
pub struct Reach {
    /// Breadth-first order. Index 0 is the origin.
    visits: Vec<Visit>,
    pub open_world: bool,
}

impl Reach {
    pub fn visited_files(&self) -> Vec<FileId> {
        self.visits.iter().map(|visit| visit.file).collect()
    }

    /// The path of files from the origin to the file visited at `index`.
    pub fn chain_to(&self, index: usize) -> Vec<FileId> {
        let mut chain = Vec::new();
        let mut cursor = Some(index);
        while let Some(position) = cursor {
            let Some(visit) = self.visits.get(position) else {
                break;
            };
            chain.push(visit.file);
            cursor = visit.parent;
        }
        chain.reverse();
        chain
    }
}

impl Workspace {
    /// Workspace folders, used to resolve root-relative URLs like `/css/site.css`.
    pub fn set_roots(&mut self, roots: Vec<PathBuf>) {
        self.roots = roots;
    }

    /// Walk the import graph from `origin`. Cycles are cut; an import that
    /// cannot be resolved makes the result open-world.
    pub fn reach(&self, origin: FileId) -> Reach {
        let Some(origin_entry) = self.file(origin) else {
            return Reach { visits: Vec::new(), open_world: true };
        };
        let mut open_world = origin_entry.analysis.open_world;
        let mut visits = vec![Visit { file: origin, parent: None }];
        let mut seen = FxHashSet::default();
        seen.insert(origin);

        let mut next = 0;
        while next < visits.len() {
            let current = visits[next].file;
            if let Some(entry) = self.file(current) {
                for import in &entry.analysis.imports {
                    match self.resolve_import(entry, import) {
                        Some(target) => {
                            if seen.insert(target) {
                                visits.push(Visit { file: target, parent: Some(next) });
                            }
                        }
                        None => open_world = true,
                    }
                }
            }
            next += 1;
        }
        Reach { visits, open_world }
    }

    /// How sure we are that `name`, used in `origin`, refers to a real rule.
    pub fn classify(&self, origin: FileId, name: NameId) -> Classification {
        let reach = self.reach(origin);

        let defined_privately_here = self.file(origin).is_some_and(|entry| {
            entry.analysis.occurrences.iter().any(|occurrence| {
                occurrence.role == Role::Definition
                    && occurrence.scope == Scope::Private
                    && occurrence.name == name
            })
        });
        if defined_privately_here {
            return Classification {
                verdict: Verdict::Verified,
                chain: vec![origin],
                defined_in: Vec::new(),
                open_world: reach.open_world,
            };
        }

        let definitions = self.occurrences_named(Role::Definition, name);
        let defining_files: FxHashSet<FileId> = definitions.iter().map(|id| id.file).collect();
        for (index, visit) in reach.visits.iter().enumerate() {
            if defining_files.contains(&visit.file) {
                return Classification {
                    verdict: Verdict::Verified,
                    chain: reach.chain_to(index),
                    defined_in: Vec::new(),
                    open_world: reach.open_world,
                };
            }
        }

        // Occurrences are ordered by file, so equal files are adjacent.
        let mut defined_in: Vec<FileId> = definitions.iter().map(|id| id.file).collect();
        defined_in.dedup();
        let verdict = if defined_in.is_empty() { Verdict::Undefined } else { Verdict::Ambient };
        Classification {
            verdict,
            chain: Vec::new(),
            defined_in,
            open_world: reach.open_world,
        }
    }

    /// The indexed file an import points at, if it can be determined.
    fn resolve_import(&self, importer: &FileEntry, import: &Import) -> Option<FileId> {
        let written = self.strings.get(import.specifier);
        let specifier = written.split(['?', '#']).next().unwrap_or("");
        let is_external = specifier.is_empty()
            || specifier.starts_with("//")
            || specifier.contains("://")
            || specifier.starts_with("data:");
        if is_external {
            return None;
        }
        let decoded = percent_decode(specifier);

        let candidates: Vec<PathBuf> = match decoded.strip_prefix('/') {
            Some(root_relative) => self.roots.iter().map(|root| root.join(root_relative)).collect(),
            None => {
                let importer_path = uri_to_path(&importer.uri)?;
                vec![importer_path.parent()?.join(&decoded)]
            }
        };
        candidates.iter().find_map(|candidate| {
            let key = normalize_path(candidate).to_string_lossy().into_owned();
            self.file_id(&key)
        })
    }
}

/// Resolve `.` and `..` components without touching the file system.
fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::languages::Language;
    use lsp_types::Uri;
    use std::str::FromStr;

    fn uri(path: &str) -> Uri {
        Uri::from_str(&format!("file://{path}")).expect("valid test uri")
    }

    fn add_css(workspace: &mut Workspace, path: &str, text: &str) {
        workspace.upsert(uri(path), text.to_owned(), Language::Css, None);
    }

    fn add_html(workspace: &mut Workspace, path: &str, text: &str) {
        workspace.upsert(uri(path), text.to_owned(), Language::Html, None);
    }

    fn page(head: &str, body: &str) -> String {
        format!("<!doctype html><html><head>{head}</head><body>{body}</body></html>")
    }

    fn link(href: &str) -> String {
        format!(r#"<link rel="stylesheet" href="{href}">"#)
    }

    fn classify_name(workspace: &Workspace, page_path: &str, name: &str) -> Classification {
        let origin = workspace.file_id(page_path).expect("page is indexed");
        let name = workspace.strings.lookup(name).expect("name appears somewhere");
        workspace.classify(origin, name)
    }

    fn paths(workspace: &Workspace, files: &[FileId]) -> Vec<String> {
        files
            .iter()
            .filter_map(|id| workspace.file(*id))
            .map(|file| {
                uri_to_path(&file.uri)
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect()
    }

    fn site_with_one_page(head: &str, body: &str) -> Workspace {
        let mut workspace = Workspace::new();
        add_css(&mut workspace, "/w/css/site.css", ".btn { }");
        add_html(&mut workspace, "/w/index.html", &page(head, body));
        workspace
    }

    #[test]
    fn a_link_verifies_and_reports_the_chain() {
        let workspace = site_with_one_page(&link("css/site.css"), r#"<p class="btn missing">"#);
        let btn = classify_name(&workspace, "/w/index.html", "btn");
        assert_eq!(btn.verdict, Verdict::Verified);
        assert_eq!(paths(&workspace, &btn.chain), ["/w/index.html", "/w/css/site.css"]);
        assert!(!btn.open_world);

        let missing = classify_name(&workspace, "/w/index.html", "missing");
        assert_eq!(missing.verdict, Verdict::Undefined);
        assert!(missing.chain.is_empty() && missing.defined_in.is_empty());
    }

    #[test]
    fn import_chains_are_followed_and_reported() {
        let mut workspace = Workspace::new();
        add_css(&mut workspace, "/w/css/base.css", ".btn { }");
        add_css(&mut workspace, "/w/css/main.css", "@import \"base.css\";");
        add_html(&mut workspace, "/w/index.html", &page(&link("css/main.css"), r#"<p class="btn">"#));
        let btn = classify_name(&workspace, "/w/index.html", "btn");
        assert_eq!(btn.verdict, Verdict::Verified);
        assert_eq!(
            paths(&workspace, &btn.chain),
            ["/w/index.html", "/w/css/main.css", "/w/css/base.css"]
        );
    }

    #[test]
    fn a_defined_but_unlinked_class_is_ambient() {
        let mut workspace = site_with_one_page(&link("css/site.css"), r#"<p class="card">"#);
        add_css(&mut workspace, "/w/css/other.css", ".card { }");
        let card = classify_name(&workspace, "/w/index.html", "card");
        assert_eq!(card.verdict, Verdict::Ambient);
        assert!(card.chain.is_empty());
        assert_eq!(paths(&workspace, &card.defined_in), ["/w/css/other.css"]);
        assert!(!card.open_world, "a closed document can say 'not linked' with confidence");
    }

    #[test]
    fn inline_style_verifies_for_its_own_page_and_is_invisible_elsewhere() {
        let mut workspace = Workspace::new();
        add_html(
            &mut workspace,
            "/w/a.html",
            &page("<style>.only { }</style>", r#"<p class="only">"#),
        );
        add_html(&mut workspace, "/w/b.html", &page("", r#"<p class="only">"#));

        let own = classify_name(&workspace, "/w/a.html", "only");
        assert_eq!(own.verdict, Verdict::Verified);
        assert_eq!(paths(&workspace, &own.chain), ["/w/a.html"]);

        let other = classify_name(&workspace, "/w/b.html", "only");
        assert_eq!(other.verdict, Verdict::Undefined, "private CSS must not be ambient elsewhere");
    }

    #[test]
    fn import_cycles_terminate() {
        let mut workspace = Workspace::new();
        add_css(&mut workspace, "/w/a.css", "@import \"b.css\";\n.a { }");
        add_css(&mut workspace, "/w/b.css", "@import \"a.css\";\n.b { }");
        add_html(&mut workspace, "/w/index.html", &page(&link("a.css"), r#"<p class="a b">"#));

        let class_a = classify_name(&workspace, "/w/index.html", "a");
        assert_eq!(paths(&workspace, &class_a.chain), ["/w/index.html", "/w/a.css"]);
        let class_b = classify_name(&workspace, "/w/index.html", "b");
        assert_eq!(paths(&workspace, &class_b.chain), ["/w/index.html", "/w/a.css", "/w/b.css"]);
    }

    #[test]
    fn diamonds_visit_each_file_once_by_the_nearest_route() {
        let mut workspace = Workspace::new();
        add_css(&mut workspace, "/w/a.css", "@import \"c.css\";\n.a { }");
        add_css(&mut workspace, "/w/b.css", "@import \"c.css\";\n.b { }");
        add_css(&mut workspace, "/w/c.css", ".x { }");
        let head = format!("{}{}", link("a.css"), link("b.css"));
        add_html(&mut workspace, "/w/index.html", &page(&head, r#"<p class="x">"#));

        let origin = workspace.file_id("/w/index.html").expect("page is indexed");
        assert_eq!(workspace.reach(origin).visited_files().len(), 4);
        let class_x = classify_name(&workspace, "/w/index.html", "x");
        assert_eq!(paths(&workspace, &class_x.chain), ["/w/index.html", "/w/a.css", "/w/c.css"]);
    }

    #[test]
    fn external_and_unresolved_links_make_the_page_open_world() {
        let mut workspace = Workspace::new();
        add_css(&mut workspace, "/w/css/site.css", ".btn { }");
        let head = link("https://cdn.example.com/x.css");
        add_html(&mut workspace, "/w/index.html", &page(&head, r#"<p class="btn">"#));
        let btn = classify_name(&workspace, "/w/index.html", "btn");
        assert_eq!(btn.verdict, Verdict::Ambient);
        assert!(btn.open_world, "an unknown stylesheet might define it");
    }

    #[test]
    fn an_external_import_inside_a_stylesheet_opens_the_world_too() {
        let mut workspace = Workspace::new();
        let css = "@import url(\"https://fonts.googleapis.com/css?family=Foo\");\n.btn { }";
        add_css(&mut workspace, "/w/css/site.css", css);
        add_html(&mut workspace, "/w/index.html", &page(&link("css/site.css"), r#"<p class="btn">"#));
        let btn = classify_name(&workspace, "/w/index.html", "btn");
        assert_eq!(btn.verdict, Verdict::Verified);
        assert!(btn.open_world);
    }

    #[test]
    fn creating_the_target_later_resolves_the_link_without_any_invalidation() {
        let mut workspace = Workspace::new();
        add_html(&mut workspace, "/w/index.html", &page(&link("css/late.css"), r#"<p class="late">"#));
        let before = classify_name(&workspace, "/w/index.html", "late");
        assert_eq!(before.verdict, Verdict::Undefined);
        assert!(before.open_world);

        add_css(&mut workspace, "/w/css/late.css", ".late { }");
        let after = classify_name(&workspace, "/w/index.html", "late");
        assert_eq!(after.verdict, Verdict::Verified);
        assert!(!after.open_world);
    }

    #[test]
    fn removing_the_target_degrades_the_verdict() {
        let mut workspace = site_with_one_page(&link("css/site.css"), r#"<p class="btn">"#);
        assert_eq!(classify_name(&workspace, "/w/index.html", "btn").verdict, Verdict::Verified);
        assert!(workspace.remove("/w/css/site.css"));
        let after = classify_name(&workspace, "/w/index.html", "btn");
        assert_eq!(after.verdict, Verdict::Undefined);
        assert!(after.open_world, "a broken link is an unknown");
    }

    #[test]
    fn fragments_and_scripts_are_open_world_even_when_linked() {
        let mut workspace = Workspace::new();
        add_css(&mut workspace, "/w/css/site.css", ".btn { }");
        let fragment = format!("{}<p class=\"btn\">", link("css/site.css"));
        add_html(&mut workspace, "/w/fragment.html", &fragment);
        let fragment_verdict = classify_name(&workspace, "/w/fragment.html", "btn");
        assert_eq!(fragment_verdict.verdict, Verdict::Verified);
        assert!(fragment_verdict.open_world, "a partial may get CSS from the page around it");

        let scripted = page(&link("css/site.css"), "<script src=\"app.js\"></script><p class=\"btn\">");
        add_html(&mut workspace, "/w/scripted.html", &scripted);
        let scripted_verdict = classify_name(&workspace, "/w/scripted.html", "btn");
        assert!(scripted_verdict.open_world, "a script can add stylesheets");
    }

    #[test]
    fn root_relative_parent_relative_and_query_string_hrefs_resolve() {
        let mut workspace = Workspace::new();
        workspace.set_roots(vec![PathBuf::from("/w")]);
        add_css(&mut workspace, "/w/css/site.css", ".btn { }");
        for (path, href) in [
            ("/w/pages/root.html", "/css/site.css"),
            ("/w/pages/parent.html", "../css/site.css"),
            ("/w/pages/query.html", "../css/site.css?v=3#top"),
            ("/w/pages/dot.html", "./../css/./site.css"),
        ] {
            add_html(&mut workspace, path, &page(&link(href), r#"<p class="btn">"#));
            let btn = classify_name(&workspace, path, "btn");
            assert_eq!(btn.verdict, Verdict::Verified, "href = {href}");
            assert!(!btn.open_world, "href = {href}");
        }
    }

    #[test]
    fn links_that_are_not_stylesheets_do_not_count() {
        let mut workspace = Workspace::new();
        let head = r#"<link rel="icon" href="favicon.ico">"#;
        add_html(&mut workspace, "/w/index.html", &page(head, r#"<p class="x">"#));
        let origin = workspace.file_id("/w/index.html").expect("page is indexed");
        let imports = workspace.file(origin).map_or(usize::MAX, |entry| entry.analysis.imports.len());
        assert_eq!(imports, 0);
        assert!(!classify_name(&workspace, "/w/index.html", "x").open_world);
    }

    #[test]
    fn paths_normalize_without_touching_the_file_system() {
        assert_eq!(normalize_path(Path::new("/w/pages/../css/./x.css")), PathBuf::from("/w/css/x.css"));
        assert_eq!(normalize_path(Path::new("/../x")), PathBuf::from("/x"));
        assert_eq!(normalize_path(Path::new("/w//a")), PathBuf::from("/w/a"));
    }
}
