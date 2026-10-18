//! CSS: class tokens in selectors become definitions.
//!
//! A "definition" is every class token that appears in a selector
//! (`.a.b:hover > .c`, `:is(.x, .y)`, `:not(.z)`, nested `&.q`, ...). They are
//! all indexed because rename must touch each one.

use crate::analysis::{FileAnalysis, Import, Role, Span};
use crate::strings::{NameId, Strings};
use std::borrow::Cow;
use std::ops::Range;
use tree_sitter::{Node, Parser};

pub struct CssIndexer {
    /// `None` when the grammar could not be loaded; analysis then finds nothing.
    parser: Option<Parser>,
    load_error: Option<String>,
    /// Reused while building one definition's at-rule chain.
    context_scratch: Vec<NameId>,
}

impl Default for CssIndexer {
    fn default() -> Self {
        Self::new()
    }
}

impl CssIndexer {
    pub fn new() -> Self {
        let mut parser = Parser::new();
        match parser.set_language(&tree_sitter_css::LANGUAGE.into()) {
            Ok(()) => Self {
                parser: Some(parser),
                load_error: None,
                context_scratch: Vec::new(),
            },
            Err(error) => Self {
                parser: None,
                load_error: Some(error.to_string()),
                context_scratch: Vec::new(),
            },
        }
    }

    /// Why the grammar failed to load (a tree-sitter ABI mismatch), if it did.
    /// The server reports this at startup instead of crashing.
    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }
}

/// Never fails: broken CSS yields whatever classes could be recovered.
pub(super) fn analyze(
    indexer: &mut CssIndexer,
    src: &str,
    strings: &mut Strings,
    out: &mut FileAnalysis,
) {
    let CssIndexer {
        parser,
        context_scratch,
        ..
    } = indexer;
    let Some(parser) = parser.as_mut() else {
        return;
    };
    let masked = mask_non_ascii(src);
    let Some(tree) = parser.parse(masked.as_bytes(), None) else {
        return;
    };
    collect(tree.root_node(), src, strings, context_scratch, out);
}

/// Replace every non-ASCII byte with `_`, preserving byte offsets.
///
/// The CSS spec makes every code point >= U+0080 a valid identifier character,
/// but tree-sitter-css only accepts a Latin subset (`.café` parses, `.привет`,
/// `.日本語` and `.😀` do not). `_` is an identifier character, so masking is
/// exactly equivalent for parsing, and inside strings/comments it is harmless.
/// Real text is always read back from the original source.
pub fn mask_non_ascii(src: &str) -> Cow<'_, str> {
    if src.is_ascii() {
        return Cow::Borrowed(src);
    }
    let mut masked = String::with_capacity(src.len());
    for byte in src.bytes() {
        masked.push(if byte >= 0x80 { '_' } else { byte as char });
    }
    Cow::Owned(masked)
}

fn collect(
    root: Node,
    src: &str,
    strings: &mut Strings,
    scratch: &mut Vec<NameId>,
    out: &mut FileAnalysis,
) {
    let mut cursor = root.walk();
    'walk: loop {
        let node = cursor.node();
        let is_class_token = node.kind() == "class_name"
            && node
                .parent()
                .is_some_and(|parent| parent.kind() == "class_selector");
        if is_class_token {
            add_definition(node, src, strings, scratch, out);
        }
        if matches!(node.kind(), "import_statement" | "at_rule") {
            add_import(node, src, strings, out);
        }
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                continue 'walk;
            }
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
}

fn add_definition(
    node: Node,
    src: &str,
    strings: &mut Strings,
    scratch: &mut Vec<NameId>,
    out: &mut FileAnalysis,
) {
    if node.is_missing() || node.byte_range().is_empty() {
        return;
    }
    let mut name_range = node.byte_range();
    let Some(raw) = src.get(name_range.clone()) else {
        return;
    };
    // Do not trust the grammar for token boundaries: on garbage like `.w;-1`
    // it can swallow delimiters. Keep only the spec-valid identifier prefix.
    let valid_len = valid_ident_prefix_len(raw);
    let Some(raw) = raw.get(..valid_len).filter(|valid| !valid.is_empty()) else {
        return;
    };
    name_range.end = name_range.start + valid_len;

    scratch.clear();
    let mut rule_range = None;
    let mut ancestor = node.parent();
    while let Some(current) = ancestor {
        match current.kind() {
            "rule_set" if rule_range.is_none() => rule_range = Some(current.byte_range()),
            "media_statement" | "supports_statement" | "at_rule" => {
                if let Some(prelude_id) = intern_prelude(current, src, strings) {
                    scratch.push(prelude_id);
                }
            }
            _ => {}
        }
        ancestor = current.parent();
    }
    scratch.reverse();

    let rule_range = rule_range.unwrap_or_else(|| name_range.clone());
    let name = strings.intern(&unescape_ident(raw));
    out.add(
        Role::Definition,
        name,
        Span::of(name_range),
        Span::of(rule_range),
        scratch,
    );
}

fn add_import(node: Node, src: &str, strings: &mut Strings, out: &mut FileAnalysis) {
    let Some(text) = src.get(node.byte_range()) else {
        return;
    };
    let Some((specifier_range, remainder)) = parse_import(text) else {
        return;
    };
    let Some(specifier) = text
        .get(specifier_range.clone())
        .filter(|specifier| !specifier.is_empty())
    else {
        return;
    };
    let start = node.start_byte() + specifier_range.start;
    let specifier_id = strings.intern(specifier);
    let condition = if remainder.is_empty() {
        None
    } else {
        Some(strings.intern_collapsed(remainder))
    };
    out.imports.push(Import {
        specifier: specifier_id,
        span: Span::of(start..start + specifier.len()),
        condition,
    });
}

/// Split `@import "a.css" print;` into the range of the specifier text within
/// `text` and the remainder (media query, `layer(..)`, `supports(..)`) without
/// the semicolon. `None` when `text` is not an `@import` with a specifier.
fn parse_import(text: &str) -> Option<(Range<usize>, &str)> {
    const KEYWORD_LENGTH: usize = "@import".len();
    let keyword = text.get(..KEYWORD_LENGTH)?;
    if !keyword.eq_ignore_ascii_case("@import") {
        return None;
    }
    let after_keyword = text.get(KEYWORD_LENGTH..)?;
    let body = after_keyword.trim_start();
    let body_start = KEYWORD_LENGTH + (after_keyword.len() - body.len());
    let first = body.chars().next()?;

    let (specifier_range, remainder_start) = if first == '"' || first == '\'' {
        let closing = body.get(1..)?.find(first)? + 1;
        (body_start + 1..body_start + closing, closing + 1)
    } else if body
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("url("))
    {
        let closing = body.find(')')?;
        let inner = body.get(4..closing)?;
        let leading = inner.len() - inner.trim_start().len();
        let trimmed = inner.trim();
        let first_char = trimmed.chars().next();
        let is_quoted = trimmed.len() >= 2
            && matches!(first_char, Some('"' | '\''))
            && first_char == trimmed.chars().next_back();
        let (start, end) = if is_quoted {
            (4 + leading + 1, 4 + leading + trimmed.len() - 1)
        } else {
            (4 + leading, 4 + leading + trimmed.len())
        };
        (body_start + start..body_start + end, closing + 1)
    } else {
        return None;
    };

    let remainder = body
        .get(remainder_start..)?
        .trim()
        .trim_end_matches(';')
        .trim();
    Some((specifier_range, remainder))
}

/// `@media (min-width: 600px)` for a `media_statement`: source text up to the
/// block, whitespace-collapsed and interned.
fn intern_prelude(node: Node, src: &str, strings: &mut Strings) -> Option<NameId> {
    let mut cursor = node.walk();
    let block = node
        .children(&mut cursor)
        .find(|child| child.kind() == "block")?;
    let text = src.get(node.start_byte()..block.start_byte())?;
    Some(strings.intern_collapsed(text))
}

/// How `new_name` must be written in a selector to replace `original_token`.
///
/// If the old token ended with the whitespace that terminates a hex escape
/// (`.x\1F600 {}`), that space is part of the token and is replaced with it,
/// so a space is put back to keep the next token separate.
pub fn render_name(original_token: &str, new_name: &str) -> String {
    let mut rendered = escape_ident(new_name);
    if original_token.ends_with(|ch: char| ch.is_ascii_whitespace()) {
        rendered.push(' ');
    }
    rendered
}

/// Serialize `name` as a CSS identifier: `md:flex` -> `md\:flex`, `10` -> `\31 0`.
pub fn escape_ident(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len() + 4);
    let starts_with_dash = name.starts_with('-');
    for (index, ch) in name.chars().enumerate() {
        let is_edge_digit = ch.is_ascii_digit() && (index == 0 || (index == 1 && starts_with_dash));
        if ch == '\0' {
            escaped.push('\u{FFFD}');
        } else if ch.is_ascii_control() || is_edge_digit {
            escaped.push_str(&format!("\\{:x} ", ch as u32));
        } else if ch == '-' && name.len() == 1 {
            escaped.push_str("\\-");
        } else if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || !ch.is_ascii() {
            escaped.push(ch);
        } else {
            escaped.push('\\');
            escaped.push(ch);
        }
    }
    escaped
}

/// Length in bytes of the longest prefix of `text` made of CSS identifier code
/// points: ASCII alphanumerics, `-`, `_`, anything non-ASCII, and escapes
/// (a backslash plus 1-6 hex digits and one optional whitespace, or a backslash
/// plus any other non-newline character).
pub fn valid_ident_prefix_len(text: &str) -> usize {
    let mut chars = text.char_indices().peekable();
    let mut end = 0;
    while let Some((offset, ch)) = chars.next() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || !ch.is_ascii() {
            end = offset + ch.len_utf8();
        } else if ch == '\\' {
            let Some(&(_, escaped)) = chars.peek() else {
                break;
            };
            if escaped == '\n' || escaped == '\r' {
                break;
            }
            let mut hex_digits = 0;
            while hex_digits < 6 {
                match chars.next_if(|&(_, next)| next.is_ascii_hexdigit()) {
                    Some((digit_offset, digit)) => {
                        end = digit_offset + digit.len_utf8();
                        hex_digits += 1;
                    }
                    None => break,
                }
            }
            if hex_digits == 0 {
                if let Some((escaped_offset, escaped_char)) = chars.next() {
                    end = escaped_offset + escaped_char.len_utf8();
                }
            } else if let Some((space_offset, space)) =
                chars.next_if(|&(_, next)| matches!(next, ' ' | '\t' | '\x0c'))
            {
                end = space_offset + space.len_utf8();
            }
        } else {
            break;
        }
    }
    end
}

/// Resolve CSS escapes in an identifier: `sm\:flex` -> `sm:flex`,
/// `\31 0` -> `10`, `\1F600 ` -> `😀`.
pub fn unescape_ident(raw: &str) -> Cow<'_, str> {
    if !raw.contains('\\') {
        return Cow::Borrowed(raw);
    }
    let mut unescaped = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            unescaped.push(ch);
            continue;
        }
        match chars.peek().copied() {
            None => unescaped.push('\u{FFFD}'),
            Some(first) if first.is_ascii_hexdigit() => {
                let mut value = 0u32;
                let mut digit_count = 0;
                while digit_count < 6 {
                    match chars.peek().and_then(|next| next.to_digit(16)) {
                        Some(digit_value) => {
                            value = value * 16 + digit_value;
                            chars.next();
                            digit_count += 1;
                        }
                        None => break,
                    }
                }
                // One whitespace after a hex escape is part of the escape.
                if chars.peek() == Some(&'\r') {
                    chars.next();
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                } else if matches!(chars.peek(), Some(' ' | '\t' | '\n' | '\x0c')) {
                    chars.next();
                }
                unescaped.push(
                    char::from_u32(value)
                        .filter(|&decoded| decoded != '\0')
                        .unwrap_or('\u{FFFD}'),
                );
            }
            Some(_) => {
                if let Some(escaped) = chars.next() {
                    unescaped.push(escaped);
                }
            }
        }
    }
    Cow::Owned(unescaped)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (unescaped name, raw token) for every definition found in `source`.
    fn definitions(source: &str) -> Vec<(String, String)> {
        let mut indexer = CssIndexer::new();
        let mut strings = Strings::new();
        let mut out = FileAnalysis::default();
        analyze(&mut indexer, source, &mut strings, &mut out);
        out.finish();
        out.occurrences
            .iter()
            .filter_map(|occurrence| {
                let raw = source.get(occurrence.name_span.range())?;
                Some((strings.get(occurrence.name).to_owned(), raw.to_owned()))
            })
            .collect()
    }

    #[test]
    fn unescape_cases() {
        let cases = [
            ("plain", "plain"),
            (r"sm\:flex", "sm:flex"),
            (r"w-1\/2", "w-1/2"),
            (r"a\.b", "a.b"),
            // A hex escape eats one trailing space.
            (r"\31 0", "10"),
            // ...but only one.
            (r"\31  0", "1 0"),
            // 'x' is not a hex digit, so it ends the escape.
            (r"\32xl\:p-4", "2xl:p-4"),
            (r"\1F600 ", "😀"),
            (r"\e9", "é"),
            // NUL, a surrogate, and a value beyond U+10FFFF all become U+FFFD.
            (r"\0", "\u{FFFD}"),
            (r"\D800", "\u{FFFD}"),
            (r"\110000", "\u{FFFD}"),
            (r"trailing\", "trailing\u{FFFD}"),
            (r"\\", "\\"),
        ];
        for (raw, expected) in cases {
            assert_eq!(unescape_ident(raw), expected, "raw = {raw:?}");
        }
    }

    #[test]
    fn valid_prefix_stops_at_delimiters_but_keeps_escapes() {
        let cases = [
            ("btn {", "btn"),
            (r"w;-1\/2", "w"),
            (r"sm\:flex{", r"sm\:flex"),
            // The second space follows a plain character: not part of the token.
            (r"\31 0 {", r"\31 0"),
            // The space right after a hex escape is part of the token.
            (r"x\1F600 {", r"x\1F600 "),
            ("a,b", "a"),
            ("日本語 x", "日本語"),
            ("", ""),
        ];
        for (raw, expected) in cases {
            assert_eq!(
                &raw[..valid_ident_prefix_len(raw)],
                expected,
                "raw = {raw:?}"
            );
        }
    }

    #[test]
    fn mask_preserves_offsets_and_ascii() {
        let source = "a😀é日b";
        let masked = mask_non_ascii(source);
        assert_eq!(masked.len(), source.len());
        assert!(masked.is_ascii());
        assert_eq!(masked.get(..1), Some("a"));
        assert_eq!(masked.get(masked.len() - 1..), Some("b"));
        assert!(matches!(mask_non_ascii("abc"), Cow::Borrowed(_)));
    }

    #[test]
    fn hex_escape_terminator_space_belongs_to_the_name_span() {
        // One whitespace after a hex escape is part of the escape, so the token
        // is `x\1F600 ` (with the space). Rename re-inserts a space for it.
        let found = definitions(r".x\1F600 {}");
        assert_eq!(found, vec![("x😀".to_owned(), r"x\1F600 ".to_owned())]);
    }

    #[test]
    fn parse_import_splits_specifier_and_condition() {
        let cases = [
            (r#"@import "a.css";"#, Some(("a.css", ""))),
            ("@import url(b.css) screen;", Some(("b.css", "screen"))),
            (
                r#"@import url("c.css") layer(base) supports(display: grid);"#,
                Some(("c.css", "layer(base) supports(display: grid)")),
            ),
            ("@import 'd.css' print;", Some(("d.css", "print"))),
            (r#"@IMPORT "e.css""#, Some(("e.css", ""))),
            (r#"@import url( "f.css" );"#, Some(("f.css", ""))),
            ("@import   \"g.css\"   ;", Some(("g.css", ""))),
            ("@media screen { }", None),
            ("@import ;", None),
            ("@import", None),
            ("@important \"x.css\"", None),
            ("", None),
        ];
        for (source, expected) in cases {
            let parsed = parse_import(source).map(|(range, remainder)| (&source[range], remainder));
            assert_eq!(parsed, expected, "source = {source:?}");
        }
    }

    #[test]
    fn imports_are_extracted_with_exact_spans() {
        let source =
            "@import \"a.css\";\n@import url(b.css) screen;\n.x { }\n@import 'd.css' print;\n";
        let mut indexer = CssIndexer::new();
        let mut strings = Strings::new();
        let mut out = FileAnalysis::default();
        analyze(&mut indexer, source, &mut strings, &mut out);

        let found: Vec<(&str, &str, Option<&str>)> = out
            .imports
            .iter()
            .map(|import| {
                (
                    strings.get(import.specifier),
                    source.get(import.span.range()).unwrap_or("?"),
                    import.condition.map(|condition| strings.get(condition)),
                )
            })
            .collect();
        assert_eq!(
            found,
            vec![
                ("a.css", "a.css", None),
                ("b.css", "b.css", Some("screen")),
                ("d.css", "d.css", Some("print")),
            ]
        );
    }

    #[test]
    fn at_rule_chains_are_recorded_outermost_first() {
        let mut indexer = CssIndexer::new();
        let mut strings = Strings::new();
        let mut out = FileAnalysis::default();
        let source = "@layer base { @media (min-width:   1px) { .a { } } }";
        analyze(&mut indexer, source, &mut strings, &mut out);
        let chain: Vec<&str> = out.contexts.iter().map(|id| strings.get(*id)).collect();
        assert_eq!(chain, ["@layer base", "@media (min-width: 1px)"]);
    }

    #[test]
    fn escaping_follows_the_cssom_rules() {
        let cases = [
            ("btn", "btn"),
            ("md:flex", r"md\:flex"),
            ("w-1/2", r"w-1\/2"),
            ("10", r"\31 0"),
            ("2xl:p-4", r"\32 xl\:p-4"),
            ("-1", r"-\31 "),
            ("-", r"\-"),
            ("--custom", "--custom"),
            ("_under", "_under"),
            ("日本語", "日本語"),
            ("a b", r"a\ b"),
            ("[x]", r"\[x\]"),
            ("tab\there", "tab\\9 here"),
        ];
        for (name, expected) in cases {
            assert_eq!(escape_ident(name), expected, "name = {name:?}");
        }
    }

    #[test]
    fn escaping_round_trips_through_unescape() {
        for name in [
            "btn",
            "md:flex",
            "w-1/2",
            "10",
            "2xl:p-4",
            "-1",
            "-",
            "a b",
            "[x]",
            "😀",
            "tab\there",
        ] {
            assert_eq!(unescape_ident(&escape_ident(name)), name, "name = {name:?}");
        }
    }

    #[test]
    fn rename_keeps_the_terminator_space_of_a_hex_escape() {
        assert_eq!(render_name(r"x\1F600 ", "y"), "y ");
        assert_eq!(render_name("btn", "button"), "button");
        assert_eq!(render_name("sm:flex", "md:flex"), r"md\:flex");
    }
}
