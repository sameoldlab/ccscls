//! Level 1b: typing simulation. An editor feeds the indexer half-typed CSS on
//! every keystroke, so every prefix/suffix of every fixture must index without
//! panicking and with sane spans. (Deliberately NOT adversarial: no fuzzing.)

use css_class_lsp::css::{ClassDef, CssIndexer, unescape_ident};

mod common;

fn all_fixtures() -> Vec<(String, String)> {
    let dir = common::fixtures_dir().join("css");
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "css"))
        .map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), std::fs::read_to_string(p).unwrap()))
        .collect();
    out.sort();
    assert!(out.len() >= 10);
    out
}

/// Structural invariants that must hold for ANY input.
fn check(src: &str, defs: &[ClassDef], what: &str) {
    let mut prev_end = 0u32;
    for d in defs {
        let (s, e) = (d.name_span.start, d.name_span.end);
        assert!(s < e && e as usize <= src.len(), "{what}: bad span {s}..{e} (len {})", src.len());
        assert!(s >= prev_end, "{what}: spans overlap or are unsorted at {s}");
        prev_end = e;
        let raw = src.get(d.name_span.range()).unwrap_or_else(|| panic!("{what}: span {s}..{e} not on char boundaries"));
        assert_eq!(&*d.name, &*unescape_ident(raw), "{what}: name != unescape(raw) for {raw:?}");
        assert!(
            rule_span_ok(src, d),
            "{what}: rule span {:?} does not contain name span {:?}",
            d.rule_span,
            d.name_span
        );
        if let Some(bad) = unescaped_delimiter(raw) {
            panic!("{what}: class token {raw:?} contains unescaped {bad:?}");
        }
    }
}

fn rule_span_ok(src: &str, d: &ClassDef) -> bool {
    d.rule_span.start <= d.name_span.start
        && d.name_span.end <= d.rule_span.end
        && src.get(d.rule_span.range()).is_some()
}

/// A class identifier may contain these only behind a backslash. (One space
/// right after a hex escape is part of the escape and is allowed.)
fn unescaped_delimiter(raw: &str) -> Option<char> {
    let mut it = raw.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\' {
            let mut hex = 0;
            while hex < 6 && it.peek().is_some_and(|d| d.is_ascii_hexdigit()) {
                it.next();
                hex += 1;
            }
            if hex == 0 {
                // any single escaped char
                it.next();
            } else if it.peek().is_some_and(|d| d.is_ascii_whitespace()) {
                it.next();
            }
        } else if c.is_whitespace() || "{}();,\"'/*".contains(c) {
            return Some(c);
        }
    }
    None
}

#[test]
fn fixtures_satisfy_invariants_and_are_deterministic() {
    let mut ix = CssIndexer::new();
    for (name, src) in all_fixtures() {
        let a = ix.index(&src);
        check(&src, &a, &name);
        assert_eq!(a, ix.index(&src), "{name}: indexing twice differs");
    }
}

#[test]
fn every_prefix_and_suffix_of_every_fixture() {
    // Simulates typing (prefixes) and deleting from the top (suffixes).
    let mut ix = CssIndexer::new();
    for (name, src) in all_fixtures() {
        for (i, _) in src.char_indices().chain([(src.len(), ' ')]) {
            let prefix = &src[..i];
            check(prefix, &ix.index(prefix), &format!("{name} prefix {i}"));
            let suffix = &src[i..];
            check(suffix, &ix.index(suffix), &format!("{name} suffix {i}"));
        }
    }
}
