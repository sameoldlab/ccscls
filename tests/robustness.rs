//! Level 1b for every language handler: whatever an editor feeds the indexer
//! (half-typed text, damaged text, hostile text) must index without panicking,
//! without hanging, and with spans that point at real text.
//!
//! Three kinds of input, all deterministic:
//!   1. every prefix and suffix of every fixture (typing and deleting),
//!   2. seeded random damage to every fixture,
//!   3. a few hostile shapes, each under a time limit.

use csscls::languages::Language;
use std::sync::mpsc;
use std::time::Duration;

mod common;
use common::{Observed, Observer};

const HOSTILE_TIME_LIMIT: Duration = Duration::from_secs(20);
const DAMAGED_VARIANTS_PER_FIXTURE: usize = 300;

fn language_cases() -> [(Language, &'static str, &'static str); 3] {
    [
        (Language::Css, "css", "css"),
        (Language::Html, "html", "html"),
        (Language::Svelte, "svelte", "svelte"),
    ]
}

fn fixtures(directory: &str, extension: &str) -> Vec<(String, String)> {
    let mut found: Vec<_> = std::fs::read_dir(common::fixtures_dir().join(directory))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|found| found == extension))
        .map(|path| {
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read_to_string(path).unwrap(),
            )
        })
        .collect();
    found.sort();
    assert!(!found.is_empty(), "no .{extension} fixtures in {directory}");
    found
}

/// What must hold for ANY input, in any language.
fn check_spans(source: &str, observed: &[Observed], what: &str) {
    for item in observed {
        let (start, end) = (item.name_span.start, item.name_span.end);
        assert!(
            start < end && end as usize <= source.len(),
            "{what}: bad name span {start}..{end} (len {})",
            source.len()
        );
        assert!(
            source.get(item.name_span.range()).is_some(),
            "{what}: name span {start}..{end} is not on char boundaries"
        );
        assert!(!item.name.is_empty(), "{what}: empty name at {start}");
        assert!(
            item.rule_span.start <= item.name_span.start
                && item.name_span.end <= item.rule_span.end
                && source.get(item.rule_span.range()).is_some(),
            "{what}: rule span {:?} does not contain name span {:?}",
            item.rule_span,
            item.name_span
        );
    }
}

fn check_input(observer: &mut Observer, language: Language, source: &str, what: &str) {
    let first = observer.observe(language, source);
    check_spans(source, &first, what);
    assert_eq!(
        first,
        observer.observe(language, source),
        "{what}: indexing twice gives different answers"
    );
}

#[test]
fn every_prefix_and_suffix_of_every_fixture() {
    let mut observer = Observer::new();
    for (language, directory, extension) in language_cases() {
        for (name, source) in fixtures(directory, extension) {
            for (index, _) in source.char_indices().chain([(source.len(), ' ')]) {
                let prefix = &source[..index];
                let suffix = &source[index..];
                check_input(
                    &mut observer,
                    language,
                    prefix,
                    &format!("{name} prefix {index}"),
                );
                check_input(
                    &mut observer,
                    language,
                    suffix,
                    &format!("{name} suffix {index}"),
                );
            }
        }
    }
}

/// A small seeded generator: no dependency, and the same damage on every run.
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound.max(1) as u64) as usize
    }
}

/// Pieces that are likely to confuse a scanner.
const TROUBLE: [&str; 18] = [
    "{", "}", "\"", "'", "`", "<", ">", "</", "/*", "*/", "<!--", "-->", "\\", ";", "(", ")",
    "{#if x}", "class=\"",
];

fn round_down_to_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn damage(random: &mut Seeded, source: &str) -> String {
    let mut text = source.to_owned();
    for _ in 0..1 + random.below(4) {
        let at = round_down_to_boundary(&text, random.below(text.len() + 1));
        match random.below(3) {
            0 => text.insert_str(at, TROUBLE[random.below(TROUBLE.len())]),
            1 => {
                let to = round_down_to_boundary(&text, at + random.below(20));
                text.replace_range(at..to, "");
            }
            _ => {
                let to = round_down_to_boundary(&text, at + random.below(40));
                let copy = text[at..to].to_owned();
                text.insert_str(at, &copy);
            }
        }
    }
    text
}

#[test]
fn seeded_random_damage_to_every_fixture() {
    let mut observer = Observer::new();
    let mut random = Seeded(0x5EED_0F_C55_C15);
    for (language, directory, extension) in language_cases() {
        for (name, source) in fixtures(directory, extension) {
            for variant in 0..DAMAGED_VARIANTS_PER_FIXTURE {
                let damaged = damage(&mut random, &source);
                check_input(
                    &mut observer,
                    language,
                    &damaged,
                    &format!("{name} damaged variant {variant}: {damaged:?}"),
                );
            }
        }
    }
}

/// Runs `source` through `language` on its own thread; fails the test if it
/// does not finish in time. (A hung thread is leaked, which is fine for a failing test.)
fn index_within_limit(language: Language, source: String, what: &str) {
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        let mut observer = Observer::new();
        let observed = observer.observe(language, &source);
        check_spans(&source, &observed, "hostile input");
        let _ = done.send(());
    });
    finished
        .recv_timeout(HOSTILE_TIME_LIMIT)
        .unwrap_or_else(|_| panic!("{what}: not finished after {HOSTILE_TIME_LIMIT:?}"));
}

#[test]
fn hostile_css_finishes() {
    for (what, source) in [
        ("many open braces", "{".repeat(5_000)),
        ("deeply nested rules", ".a{".repeat(2_000)),
        ("many selectors", ".c{}".repeat(20_000)),
        (
            "one very long identifier",
            format!(".{} {{}}", "a".repeat(200_000)),
        ),
        ("many escapes", ".\\".repeat(20_000)),
    ] {
        index_within_limit(Language::Css, source, what);
    }
}

#[test]
fn hostile_markup_finishes() {
    for language in [Language::Html, Language::Svelte] {
        for (what, source) in [
            ("many open tags", "<div ".repeat(20_000)),
            ("many unclosed class values", "<p class=\"".repeat(20_000)),
            (
                "one huge class value",
                format!("<p class=\"{}\">", "a ".repeat(100_000)),
            ),
            (
                "many open braces",
                format!("<p class={}", "{".repeat(20_000)),
            ),
            (
                "nested expressions",
                format!("<p class={}", "{[".repeat(5_000)),
            ),
            ("many comments", "<!--".repeat(20_000)),
            ("many styles", "<style>.a{}</style>".repeat(5_000)),
        ] {
            index_within_limit(language, source, &format!("{language:?}: {what}"));
        }
    }
}

#[test]
fn the_completion_context_never_panics_while_typing() {
    for (language, directory, extension) in language_cases() {
        for (_name, source) in fixtures(directory, extension) {
            for (index, _) in source.char_indices().chain([(source.len(), ' ')]) {
                let _ = language.class_context_at(&source[..index], index);
            }
        }
    }
}
