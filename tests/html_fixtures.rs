//! Level 1: HTML class-usage extraction against annotated fixture files.

mod common;
use common::*;
use css_class_lsp::languages::Language;

#[test]
fn every_html_fixture_matches_its_annotations() {
    let dir = fixtures_dir().join("html");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "html"))
        .collect();
    files.sort();
    assert!(files.len() >= 2, "fixtures missing? found {}", files.len());

    let mut observer = Observer::new();
    let mut failures = Vec::new();
    for path in &files {
        let src = std::fs::read_to_string(path).unwrap();
        let want = parse_expectations_in(&src, Syntax::Html);
        assert!(!want.is_empty(), "{} has no <!--~ markers", path.display());
        let got = describe(&src, &observer.observe(Language::Html, &src));
        if want != got {
            let fmt = |e: &Expected| format!("L{} {}", e.line + 1, e.raw);
            let mut d = Vec::new();
            d.extend(want.iter().filter(|e| !got.contains(e)).map(|e| format!("  - {}", fmt(e))));
            d.extend(got.iter().filter(|e| !want.contains(e)).map(|e| format!("  + {}", fmt(e))));
            failures.push(format!("{}\n{}", path.file_name().unwrap().to_string_lossy(), d.join("\n")));
        }
    }
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n\n"));
}

#[test]
fn typing_prefixes_never_panic() {
    // Cheap typing simulation over every html fixture.
    let mut observer = Observer::new();
    let dir = fixtures_dir().join("html");
    for entry in std::fs::read_dir(dir).unwrap() {
        let src = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        for (i, _) in src.char_indices() {
            let _ = observer.observe(Language::Html, &src[..i]);
            let _ = Language::Html.class_context_at(&src[..i], i);
        }
    }
}
