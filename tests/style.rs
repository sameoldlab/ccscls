//! Mechanical style rules from STYLE.md, enforced by `cargo test`.
//!
//! 1. No comment at the end of a line of code (all Rust files).
//! 2. No `unwrap`/`expect`/`panic!`-family in library code (`src/`, outside test modules).
//! 3. No single-letter bindings in `src/` (outside test modules), except `i`.
//!
//! Rule 3 has a ratchet: files listed in `PENDING_NAME_CLEANUP` are skipped.
//! The list may only shrink.

use std::path::{Path, PathBuf};

const PENDING_NAME_CLEANUP: &[&str] = &[];

const PANIC_MARKERS: &[&str] = &[".unwrap()", ".expect(", "panic!(", "unreachable!(", "todo!(", "unimplemented!("];

struct CodeLine {
    number: usize,
    /// The line with string/char literal contents blanked and comments removed.
    code: String,
    has_trailing_comment: bool,
}

struct Lexer {
    lines: Vec<CodeLine>,
    code: String,
    seen_code: bool,
    trailing: bool,
    number: usize,
}

impl Lexer {
    fn end_line(&mut self) {
        self.lines.push(CodeLine {
            number: self.number,
            code: std::mem::take(&mut self.code),
            has_trailing_comment: self.trailing,
        });
        self.seen_code = false;
        self.trailing = false;
        self.number += 1;
    }
}

enum State {
    Code,
    BlockComment(u32),
    Str,
    RawStr(usize),
}

fn is_ident_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn analyze(source: &str) -> Vec<CodeLine> {
    let chars: Vec<char> = source.chars().collect();
    let mut lexer = Lexer { lines: Vec::new(), code: String::new(), seen_code: false, trailing: false, number: 1 };
    let mut state = State::Code;
    let mut cursor = 0;
    let at = |index: usize| chars.get(index).copied();
    while cursor < chars.len() {
        let ch = chars[cursor];
        match state {
            State::Code => {
                if ch == '\n' {
                    lexer.end_line();
                    cursor += 1;
                } else if ch == '/' && at(cursor + 1) == Some('/') {
                    lexer.trailing |= lexer.seen_code;
                    while cursor < chars.len() && chars[cursor] != '\n' {
                        cursor += 1;
                    }
                } else if ch == '/' && at(cursor + 1) == Some('*') {
                    lexer.trailing |= lexer.seen_code;
                    state = State::BlockComment(1);
                    cursor += 2;
                } else if let Some((hashes, length)) = raw_string_start(&chars, cursor) {
                    lexer.code.push_str("\"\"");
                    lexer.seen_code = true;
                    state = State::RawStr(hashes);
                    cursor += length;
                } else if ch == 'b'
                    && matches!(at(cursor + 1), Some('\'' | '"'))
                    && (cursor == 0 || !is_ident_char(chars[cursor - 1]))
                {
                    // Byte-literal prefix: part of the literal, not a name.
                    cursor += 1;
                } else if ch == '"' {
                    lexer.code.push_str("\"\"");
                    lexer.seen_code = true;
                    state = State::Str;
                    cursor += 1;
                } else if ch == '\'' {
                    match char_literal_length(&chars, cursor) {
                        Some(length) => {
                            lexer.code.push_str("''");
                            cursor += length;
                        }
                        None => {
                            lexer.code.push(ch);
                            cursor += 1;
                        }
                    }
                    lexer.seen_code = true;
                } else {
                    lexer.code.push(ch);
                    lexer.seen_code |= !ch.is_whitespace();
                    cursor += 1;
                }
            }
            State::BlockComment(depth) => {
                if ch == '/' && at(cursor + 1) == Some('*') {
                    state = State::BlockComment(depth + 1);
                    cursor += 2;
                } else if ch == '*' && at(cursor + 1) == Some('/') {
                    state = if depth == 1 { State::Code } else { State::BlockComment(depth - 1) };
                    cursor += 2;
                } else {
                    if ch == '\n' {
                        lexer.end_line();
                    }
                    cursor += 1;
                }
            }
            State::Str => {
                if ch == '\\' {
                    if at(cursor + 1) == Some('\n') {
                        lexer.end_line();
                    }
                    cursor += 2;
                } else {
                    if ch == '"' {
                        state = State::Code;
                    } else if ch == '\n' {
                        lexer.end_line();
                    }
                    cursor += 1;
                }
            }
            State::RawStr(hashes) => {
                let closes = ch == '"' && (1..=hashes).all(|offset| at(cursor + offset) == Some('#'));
                if closes {
                    state = State::Code;
                    cursor += 1 + hashes;
                } else {
                    if ch == '\n' {
                        lexer.end_line();
                    }
                    cursor += 1;
                }
            }
        }
    }
    lexer.end_line();
    lexer.lines
}

/// `r"`, `r#"`, `br"`... at `cursor` (not in the middle of an identifier):
/// returns (hash count, characters consumed through the opening quote).
fn raw_string_start(chars: &[char], cursor: usize) -> Option<(usize, usize)> {
    if cursor > 0 && is_ident_char(chars[cursor - 1]) {
        return None;
    }
    let mut position = cursor;
    if chars.get(position) == Some(&'b') {
        position += 1;
    }
    if chars.get(position) != Some(&'r') {
        return None;
    }
    position += 1;
    let mut hashes = 0;
    while chars.get(position) == Some(&'#') {
        hashes += 1;
        position += 1;
    }
    (chars.get(position) == Some(&'"')).then_some((hashes, position + 1 - cursor))
}

/// Length of a char literal starting at `cursor` (`'x'`, `'\n'`), or None for a lifetime.
fn char_literal_length(chars: &[char], cursor: usize) -> Option<usize> {
    match chars.get(cursor + 1)? {
        '\\' => (cursor + 2..(cursor + 14).min(chars.len()))
            .find(|&index| chars[index] == '\'')
            .map(|index| index + 1 - cursor),
        _ if chars.get(cursor + 2) == Some(&'\'') => Some(3),
        _ => None,
    }
}

fn identifiers(text: &str) -> Vec<&str> {
    text.split(|ch: char| !is_ident_char(ch))
        .filter(|word| !word.is_empty())
        .collect()
}

fn is_binding_name(word: &str) -> bool {
    let first = word.chars().next().unwrap_or('A');
    (first.is_lowercase() || first == '_') && !matches!(word, "mut" | "ref" | "in" | "if" | "else" | "as")
}

/// Single-letter names introduced by `let`, `for`, closure and `fn` parameter lists on this line.
fn single_letter_bindings(code: &str) -> Vec<String> {
    let mut patterns: Vec<&str> = Vec::new();

    for (index, _) in code.match_indices("let ") {
        let boundary_before = index == 0 || !is_ident_char(code[..index].chars().next_back().unwrap_or(' '));
        if !boundary_before {
            continue;
        }
        let rest = &code[index + 4..];
        let end = rest
            .char_indices()
            .find(|&(offset, ch)| {
                let next = rest[offset + ch.len_utf8()..].chars().next();
                (ch == '=' && next != Some('=') && next != Some('>')) || ch == ';' || (ch == ':' && next != Some(':'))
            })
            .map_or(rest.len(), |(offset, _)| offset);
        patterns.push(&rest[..end]);
    }
    if let Some(index) = code.find("for ") {
        if let Some(end) = code[index..].find(" in ") {
            patterns.push(&code[index + 4..index + end]);
        }
    }
    if let Some(index) = code.find("fn ") {
        if let Some(open) = code[index..].find('(') {
            let params = &code[index + open + 1..];
            let params = params.split(')').next().unwrap_or("");
            for part in params.split(',') {
                if let Some((name, _)) = part.split_once(':') {
                    patterns.push(name);
                }
            }
        }
    }
    let pipe_positions: Vec<usize> = code.match_indices('|').map(|(index, _)| index).collect();
    for pair in pipe_positions.chunks(2) {
        if let [open, close] = pair {
            let before = code[..*open].trim_end();
            let starts_closure = before.is_empty()
                || before.ends_with(['(', ',', '=', '{'])
                || before.ends_with("move");
            let between = &code[open + 1..*close];
            if starts_closure && !between.contains(['=', ';', '{']) {
                for part in between.split(',') {
                    patterns.push(part.split(':').next().unwrap_or(""));
                }
            }
        }
    }

    patterns
        .into_iter()
        .flat_map(identifiers)
        .filter(|word| is_binding_name(word) && word.chars().count() == 1 && *word != "i" && *word != "_")
        .map(str::to_owned)
        .collect()
}

fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if !matches!(path.file_name().and_then(|name| name.to_str()), Some("fixtures" | "target")) {
                rust_files(&path, found);
            }
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

fn crate_files(subdirs: &[&str]) -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut paths = Vec::new();
    for subdir in subdirs {
        rust_files(&root.join(subdir), &mut paths);
    }
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let relative = path.strip_prefix(root).ok()?.to_string_lossy().replace('\\', "/");
            Some((relative, std::fs::read_to_string(&path).ok()?))
        })
        .collect()
}

/// Lines before the first `#[cfg(test)]`: the library code proper.
fn library_lines(source: &str) -> Vec<CodeLine> {
    analyze(source)
        .into_iter()
        .take_while(|line| !line.code.contains("#[cfg(test)]"))
        .collect()
}

#[test]
fn no_comment_at_the_end_of_a_line_of_code() {
    let mut violations = Vec::new();
    for (path, source) in crate_files(&["src", "examples", "tests"]) {
        for line in analyze(&source) {
            if line.has_trailing_comment {
                violations.push(format!("{path}:{}", line.number));
            }
        }
    }
    assert!(violations.is_empty(), "move these comments to the line above:\n{}", violations.join("\n"));
}

#[test]
fn library_code_does_not_panic_on_purpose() {
    let mut violations = Vec::new();
    for (path, source) in crate_files(&["src"]) {
        for line in library_lines(&source) {
            for marker in PANIC_MARKERS {
                if line.code.contains(marker) {
                    violations.push(format!("{path}:{}: {marker}", line.number));
                }
            }
        }
    }
    assert!(violations.is_empty(), "an LSP server must not crash in normal use:\n{}", violations.join("\n"));
}

#[test]
fn library_code_has_no_single_letter_names() {
    let mut violations = Vec::new();
    for (path, source) in crate_files(&["src"]) {
        if PENDING_NAME_CLEANUP.contains(&path.as_str()) {
            continue;
        }
        for line in library_lines(&source) {
            for name in single_letter_bindings(&line.code) {
                violations.push(format!("{path}:{}: `{name}`", line.number));
            }
        }
    }
    assert!(violations.is_empty(), "use descriptive names:\n{}", violations.join("\n"));
}

#[test]
fn pending_list_only_names_files_that_still_have_violations() {
    for path in PENDING_NAME_CLEANUP {
        let (_, source) = crate_files(&["src"]).into_iter().find(|(name, _)| name == path).expect("listed file exists");
        let still_violating = library_lines(&source).iter().any(|line| !single_letter_bindings(&line.code).is_empty());
        assert!(still_violating, "{path} is clean now: remove it from PENDING_NAME_CLEANUP");
    }
}

#[test]
fn lexer_understands_strings_comments_and_literals() {
    let flagged = |source: &str| analyze(source).iter().any(|line| line.has_trailing_comment);
    assert!(flagged("let url = 1; // note\n"));
    assert!(flagged("let url = 1; /* note */\n"));
    assert!(!flagged("// note\nlet url = \"http://example.com\";\n"));
    assert!(!flagged("let text = r#\"// not a comment\"#;\n"));
    assert!(!flagged("let quote = '\"'; let tick = '\\'';\n"));
    assert!(!flagged("fn lifetime<'a>(text: &'a str) -> &'a str { text }\n"));
    assert!(!flagged("let multi = \"first\nsecond // still a string\";\n"));
    assert!(!flagged("/* block\n   comment */\nlet name = 1;\n"));
}

#[test]
fn name_detector_finds_the_usual_suspects() {
    let names = |code: &str| single_letter_bindings(code);
    assert_eq!(names("let x = 1;"), ["x"]);
    assert_eq!(names("let (a, mut b) = pair;"), ["a", "b"]);
    assert_eq!(names("for c in chars {"), ["c"]);
    assert_eq!(names("items.iter().map(|d| d.len())"), ["d"]);
    assert_eq!(names("fn run(p: Params, count: usize)"), ["p"]);
    assert_eq!(names("let Some(n) = value else { return };"), ["n"]);
    assert!(names("let index = 1; for i in 0..3 {}").is_empty());
    assert!(names("let value = left | right;").is_empty());
    assert!(names("let kind: Vec<A> = Vec::new();").is_empty());
    // The detector runs on lexed code, where literals are already blanked.
    let lexed = analyze("if let [b'%', high, low, ..] = &bytes[cursor..] {\n");
    assert!(names(&lexed[0].code).is_empty(), "{:?}", lexed[0].code);
}
