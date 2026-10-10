//! Level 1: parse fixture files and compare against the expectations embedded
//! in them. No LSP involved.

mod common;
use common::*;
use css_class_lsp::css::CssIndexer;

#[test]
fn every_css_fixture_matches_its_annotations() {
    let dir = fixtures_dir().join("css");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "css"))
        .collect();
    files.sort();
    assert!(files.len() >= 10, "fixtures missing? found {}", files.len());

    let mut indexer = CssIndexer::new();
    let mut failures = Vec::new();
    for path in &files {
        let src = std::fs::read_to_string(path).unwrap();
        let want = parse_expectations(&src);
        assert!(!want.is_empty(), "{} has no /*~ markers", path.display());
        let got = describe(&src, &indexer.index(&src));
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
    let fmt = |e: &Expected| {
        format!("L{} {} => {}  ctx={:?}", e.line + 1, e.raw, e.name, e.ctx)
    };
    let mut out = Vec::new();
    for e in want.iter().filter(|e| !got.contains(e)) {
        out.push(format!("  - {}", fmt(e)));
    }
    for e in got.iter().filter(|e| !want.contains(e)) {
        out.push(format!("  + {}", fmt(e)));
    }
    if out.is_empty() {
        out.push("  (same entries, different order or multiplicity)".into());
    }
    out.join("\n")
}
