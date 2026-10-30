//! Level 1: invariants that only CSS has. A class token is an identifier, so
//! its name must equal the unescaped source text and must not contain an
//! unescaped delimiter, even when the CSS is half-typed. (The checks every
//! language shares are in `robustness.rs`.)

use csscls::languages::Language;
use csscls::languages::css::unescape_ident;

mod common;
use common::{Observed, Observer};

fn all_fixtures() -> Vec<(String, String)> {
    let dir = common::fixtures_dir().join("css");
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "css"))
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read_to_string(p).unwrap(),
            )
        })
        .collect();
    out.sort();
    assert!(out.len() >= 10);
    out
}

/// Structural invariants that must hold for ANY input.
fn check(src: &str, defs: &[Observed], what: &str) {
    let mut prev_end = 0u32;
    for d in defs {
        let (s, e) = (d.name_span.start, d.name_span.end);
        assert!(
            s < e && e as usize <= src.len(),
            "{what}: bad span {s}..{e} (len {})",
            src.len()
        );
        assert!(
            s >= prev_end,
            "{what}: spans overlap or are unsorted at {s}"
        );
        prev_end = e;
        let raw = src
            .get(d.name_span.range())
            .unwrap_or_else(|| panic!("{what}: span {s}..{e} not on char boundaries"));
        assert_eq!(
            d.name.as_str(),
            &*unescape_ident(raw),
            "{what}: name != unescape(raw) for {raw:?}"
        );
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

fn rule_span_ok(src: &str, d: &Observed) -> bool {
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
    let mut ix = Observer::new();
    for (name, src) in all_fixtures() {
        let a = ix.observe(Language::Css, &src);
        check(&src, &a, &name);
        assert_eq!(
            a,
            ix.observe(Language::Css, &src),
            "{name}: indexing twice differs"
        );
    }
}

#[test]
fn every_prefix_and_suffix_of_every_fixture() {
    // Simulates typing (prefixes) and deleting from the top (suffixes).
    let mut ix = Observer::new();
    for (name, src) in all_fixtures() {
        for (i, _) in src.char_indices().chain([(src.len(), ' ')]) {
            let prefix = &src[..i];
            check(
                prefix,
                &ix.observe(Language::Css, prefix),
                &format!("{name} prefix {i}"),
            );
            let suffix = &src[i..];
            check(
                suffix,
                &ix.observe(Language::Css, suffix),
                &format!("{name} suffix {i}"),
            );
        }
    }
}
