//! Level 1: Svelte class usages against annotated fixture files.

mod common;
use common::*;
use csscls::languages::Language;

#[test]
fn every_svelte_fixture_matches_its_annotations() {
    let dir = fixtures_dir().join("svelte");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "svelte"))
        .collect();
    files.sort();
    assert!(files.len() >= 2, "fixtures missing? found {}", files.len());

    let mut observer = Observer::new();
    let mut failures = Vec::new();
    for path in &files {
        let src = std::fs::read_to_string(path).unwrap();
        let want = parse_expectations_in(&src, Syntax::Html);
        assert!(!want.is_empty(), "{} has no <!--~ markers", path.display());
        let got = describe(&src, &observer.observe(Language::Svelte, &src));
        if want != got {
            let fmt = |e: &Expected| format!("L{} {}", e.line + 1, e.raw);
            let mut d = Vec::new();
            d.extend(
                want.iter()
                    .filter(|e| !got.contains(e))
                    .map(|e| format!("  - {}", fmt(e))),
            );
            d.extend(
                got.iter()
                    .filter(|e| !want.contains(e))
                    .map(|e| format!("  + {}", fmt(e))),
            );
            if d.is_empty() {
                d.push("  (same entries, different order or multiplicity)".to_owned());
            }
            failures.push(format!(
                "{}\n{}",
                path.file_name().unwrap().to_string_lossy(),
                d.join("\n")
            ));
        }
    }
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n\n"));
}

#[test]
fn typing_prefixes_never_panic() {
    let mut observer = Observer::new();
    let dir = fixtures_dir().join("svelte");
    for entry in std::fs::read_dir(dir).unwrap() {
        let src = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        for (i, _) in src.char_indices() {
            let _ = observer.observe(Language::Svelte, &src[..i]);
            let _ = Language::Svelte.class_context_at(&src[..i], i);
        }
    }
}
