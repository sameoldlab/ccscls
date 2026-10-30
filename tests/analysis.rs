//! Level 1: what each language handler finds in annotated fixture files. No
//! LSP involved. The expectations are written in the fixtures themselves
//! (`/*~ ... */` in CSS, `<!--~ ... -->` in HTML and Svelte).

mod common;
use common::*;
use csscls::languages::Language;

fn check_fixture_directory(
    language: Language,
    directory: &str,
    extension: &str,
    syntax: Syntax,
    minimum_files: usize,
) {
    let mut files: Vec<_> = std::fs::read_dir(fixtures_dir().join(directory))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|found| found == extension))
        .collect();
    files.sort();
    assert!(
        files.len() >= minimum_files,
        "fixtures missing? found {} in {directory}",
        files.len()
    );

    let mut observer = Observer::new();
    let mut failures = Vec::new();
    for path in &files {
        let source = std::fs::read_to_string(path).unwrap();
        let want = parse_expectations_in(&source, syntax);
        assert!(!want.is_empty(), "{} has no markers", path.display());
        let got = describe(&source, &observer.observe(language, &source));
        if want != got {
            failures.push(format!(
                "{}\n{}",
                path.file_name().unwrap().to_string_lossy(),
                diff(&want, &got)
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n\n"));
}

/// Expected-only lines are `-`, found-only lines are `+`.
fn diff(want: &[Expected], got: &[Expected]) -> String {
    let format = |entry: &Expected| {
        format!(
            "L{} {} => {}  ctx={:?}",
            entry.line + 1,
            entry.raw,
            entry.name,
            entry.ctx
        )
    };
    let mut out = Vec::new();
    for entry in want.iter().filter(|entry| !got.contains(entry)) {
        out.push(format!("  - {}", format(entry)));
    }
    for entry in got.iter().filter(|entry| !want.contains(entry)) {
        out.push(format!("  + {}", format(entry)));
    }
    if out.is_empty() {
        out.push("  (same entries, different order or multiplicity)".into());
    }
    out.join("\n")
}

// ============================================================================
// css
// ============================================================================

#[test]
fn every_css_fixture_matches_its_annotations() {
    check_fixture_directory(Language::Css, "css", "css", Syntax::Css, 10);
}

// ============================================================================
// html
// ============================================================================

#[test]
fn every_html_fixture_matches_its_annotations() {
    check_fixture_directory(Language::Html, "html", "html", Syntax::Html, 2);
}

#[test]
fn usages_in_multiline_value_have_correct_spans() {
    let src = "<div class=\"one\n   two\">";
    let u = Observer::new().observe(Language::Html, src);
    assert_eq!(u.len(), 2);
    assert_eq!(&src[u[0].name_span.range()], "one");
    assert_eq!(&src[u[1].name_span.range()], "two");
}

// ============================================================================
// svelte
// ============================================================================

#[test]
fn every_svelte_fixture_matches_its_annotations() {
    check_fixture_directory(Language::Svelte, "svelte", "svelte", Syntax::Html, 2);
}
