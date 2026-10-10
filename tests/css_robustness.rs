//! Level 1b: the indexer must never panic and must always return sane spans,
//! because an editor feeds it half-typed CSS on every keystroke.

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
                it.next(); // any single escaped char
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

#[test]
fn random_mutations_of_fixtures() {
    // Deterministic xorshift so failures reproduce.
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let alphabet: Vec<&str> = vec![
        ".", "{", "}", "(", ")", ";", ":", "\"", "'", "\\", "/", "*", "@", "-", " ", "\n", "\r\n",
        "😀", "é", "日", ".a", "@media (x) {", "/*", "*/", "url(", "&", ">", ",", "[", "]", "#", "\0",
    ];
    let fixtures = all_fixtures();
    let mut ix = CssIndexer::new();
    for round in 0..3000 {
        let (name, src) = &fixtures[(next() as usize) % fixtures.len()];
        let mut s = src.clone();
        for _ in 0..(1 + next() % 4) {
            let boundaries: Vec<usize> = s.char_indices().map(|(i, _)| i).chain([s.len()]).collect();
            let at = boundaries[(next() as usize) % boundaries.len()];
            if next() % 3 == 0 && at < s.len() {
                let end = boundaries.iter().copied().find(|&b| b > at).unwrap();
                s.replace_range(at..end, "");
            } else {
                s.insert_str(at, alphabet[(next() as usize) % alphabet.len()]);
            }
        }
        check(&s, &ix.index(&s), &format!("{name} mutation round {round}"));
    }
}

#[test]
fn hostile_inputs() {
    let deep_blocks = "{".repeat(5000);
    let deep_nesting = ".a{".repeat(2000) + &"}".repeat(2000);
    let many_selectors = (0..20_000).map(|i| format!(".c{i},")).collect::<String>() + "{}";
    let long_ident = format!(".{} {{}}", "x".repeat(200_000));
    let inputs: Vec<(&str, &str)> = vec![
        ("empty", ""),
        ("just dot", "."),
        ("dot backslash", ".\\"),
        ("backslashes", "\\\\\\\\\\\\"),
        ("unterminated string", ".a { content: \"abc"),
        ("unterminated comment", ".a {} /* .b {}"),
        ("unterminated url", ".a { background: url(abc"),
        ("nul bytes", ".a\0b {} \0 .c {}"),
        ("lone braces", "}}}}{{{{"),
        ("at-rule soup", "@media @supports @layer { @keyframes {"),
        ("bom", "\u{feff}.a {}"),
        ("deep blocks", deep_blocks.as_str()),
        ("deep nesting", deep_nesting.as_str()),
        ("many selectors", many_selectors.as_str()),
        ("long identifier", long_ident.as_str()),
    ];
    let mut ix = CssIndexer::new();
    for (name, src) in inputs {
        check(src, &ix.index(src), name);
    }
}
