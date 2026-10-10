//! CSS source text -> class definitions.
//!
//! A "definition" here is every class token that appears in a selector
//! (`.a.b:hover > .c`, `:is(.x, .y)`, `:not(.z)`, nested `&.q`, ...). They are
//! all indexed because rename must touch each one.

use std::borrow::Cow;
use std::ops::Range;
use tree_sitter::{Node, Parser};

/// Half-open byte range into the UTF-8 source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn range(self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassDef {
    /// Unescaped class name, as it would be written in an HTML `class` attribute.
    pub name: Box<str>,
    /// The identifier exactly as written in the source: no leading `.`, escapes intact.
    pub name_span: Span,
    /// The enclosing style rule (the innermost `rule_set`), used for hover.
    /// Falls back to `name_span` when error recovery left no enclosing rule.
    pub rule_span: Span,
    /// Enclosing at-rule preludes, outermost first, e.g. `["@media (min-width: 600px)", "@layer base"]`.
    pub context: Vec<Box<str>>,
}

pub struct CssIndexer {
    parser: Parser,
}

impl Default for CssIndexer {
    fn default() -> Self {
        Self::new()
    }
}

impl CssIndexer {
    pub fn new() -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_css::LANGUAGE.into())
            .expect("tree-sitter-css grammar is compatible with the tree-sitter runtime");
        Self { parser }
    }

    /// Never fails: broken CSS yields whatever classes could be recovered.
    /// The result is sorted by position.
    pub fn index(&mut self, src: &str) -> Vec<ClassDef> {
        let masked = mask_non_ascii(src);
        let Some(tree) = self.parser.parse(masked.as_bytes(), None) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        collect(tree.root_node(), src, &mut out);
        out.sort_by_key(|d| d.name_span.start);
        out
    }
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
    let bytes: Vec<u8> = src
        .bytes()
        .map(|b| if b >= 0x80 { b'_' } else { b })
        .collect();
    Cow::Owned(String::from_utf8(bytes).expect("masked text is pure ASCII"))
}

/// Find the class under `offset` (end-inclusive, so a cursor right after the
/// last character still counts). `defs` must be sorted by position.
pub fn class_at(defs: &[ClassDef], offset: usize) -> Option<&ClassDef> {
    let idx = defs.partition_point(|d| d.name_span.start as usize <= offset);
    let def = defs.get(idx.checked_sub(1)?)?;
    (offset <= def.name_span.end as usize).then_some(def)
}

fn collect(root: Node, src: &str, out: &mut Vec<ClassDef>) {
    let mut cur = root.walk();
    'walk: loop {
        let node = cur.node();
        if node.kind() == "class_name"
            && node.parent().is_some_and(|p| p.kind() == "class_selector")
        {
            if let Some(def) = make_def(node, src) {
                out.push(def);
            }
        }
        if cur.goto_first_child() {
            continue;
        }
        loop {
            if cur.goto_next_sibling() {
                continue 'walk;
            }
            if !cur.goto_parent() {
                break 'walk;
            }
        }
    }
}

fn make_def(node: Node, src: &str) -> Option<ClassDef> {
    if node.is_missing() || node.byte_range().is_empty() {
        return None;
    }
    let mut name_range = node.byte_range();
    let raw = src.get(name_range.clone())?;
    // Do not trust the grammar for token boundaries: on garbage like `.w;-1`
    // it can swallow delimiters. Keep only the spec-valid identifier prefix.
    let valid = valid_ident_prefix_len(raw);
    if valid == 0 {
        return None;
    }
    name_range.end = name_range.start + valid;
    let raw = &raw[..valid];

    let mut rule_range = None;
    let mut at_rules: Vec<Box<str>> = Vec::new();
    let mut parent = node.parent();
    while let Some(n) = parent {
        match n.kind() {
            "rule_set" if rule_range.is_none() => rule_range = Some(n.byte_range()),
            "media_statement" | "supports_statement" | "at_rule" => {
                if let Some(p) = prelude(n, src) {
                    at_rules.push(p);
                }
            }
            _ => {}
        }
        parent = n.parent();
    }
    at_rules.reverse();

    let rule_range = rule_range.unwrap_or_else(|| name_range.clone());
    Some(ClassDef {
        name: unescape_ident(raw).into(),
        name_span: span(name_range),
        rule_span: span(rule_range),
        context: at_rules,
    })
}

fn span(r: Range<usize>) -> Span {
    Span {
        start: r.start as u32,
        end: r.end as u32,
    }
}

/// `@media (min-width: 600px)` for a `media_statement`: source text up to the
/// block, whitespace-collapsed.
fn prelude(node: Node, src: &str) -> Option<Box<str>> {
    let mut c = node.walk();
    let block = node.children(&mut c).find(|ch| ch.kind() == "block")?;
    let text = src.get(node.start_byte()..block.start_byte())?;
    let mut out = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    Some(out.into())
}

/// Length in bytes of the longest prefix of `s` made of CSS identifier code
/// points: ASCII alphanumerics, `-`, `_`, anything non-ASCII, and escapes
/// (a backslash plus 1-6 hex digits and one optional whitespace, or a backslash
/// plus any other non-newline character).
pub fn valid_ident_prefix_len(s: &str) -> usize {
    let mut it = s.char_indices().peekable();
    let mut end = 0;
    while let Some((i, c)) = it.next() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || !c.is_ascii() {
            end = i + c.len_utf8();
        } else if c == '\\' {
            let Some(&(_, next)) = it.peek() else { break };
            if next == '\n' || next == '\r' {
                break;
            }
            let mut hex = 0;
            while hex < 6 && it.peek().is_some_and(|&(_, d)| d.is_ascii_hexdigit()) {
                let (j, d) = it.next().expect("peeked");
                end = j + d.len_utf8();
                hex += 1;
            }
            if hex == 0 {
                let (j, d) = it.next().expect("peeked");
                end = j + d.len_utf8();
            } else if let Some(&(j, w)) = it.peek() {
                if matches!(w, ' ' | '\t' | '\x0c') {
                    it.next();
                    end = j + w.len_utf8();
                }
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
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            None => out.push('\u{FFFD}'),
            Some(h) if h.is_ascii_hexdigit() => {
                let mut value = 0u32;
                let mut digits = 0;
                while digits < 6 {
                    match chars.peek().and_then(|d| d.to_digit(16)) {
                        Some(d) => {
                            value = value * 16 + d;
                            chars.next();
                            digits += 1;
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
                out.push(
                    char::from_u32(value)
                        .filter(|&c| c != '\0')
                        .unwrap_or('\u{FFFD}'),
                );
            }
            Some(_) => out.push(chars.next().expect("peeked")),
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescape_cases() {
        let cases = [
            ("plain", "plain"),
            (r"sm\:flex", "sm:flex"),
            (r"w-1\/2", "w-1/2"),
            (r"a\.b", "a.b"),
            (r"\31 0", "10"),         // hex escape eats one trailing space
            (r"\31  0", "1 0"),       // ...but only one
            (r"\32xl\:p-4", "2xl:p-4"), // 'x' is not a hex digit, ends the escape
            (r"\1F600 ", "😀"),
            (r"\e9", "é"),
            (r"\0", "\u{FFFD}"),        // NUL
            (r"\D800", "\u{FFFD}"),     // surrogate
            (r"\110000", "\u{FFFD}"),   // beyond U+10FFFF
            (r"trailing\", "trailing\u{FFFD}"),
            (r"\\", "\\"),
        ];
        for (raw, want) in cases {
            assert_eq!(unescape_ident(raw), want, "raw = {raw:?}");
        }
    }

    #[test]
    fn valid_prefix_stops_at_delimiters_but_keeps_escapes() {
        for (raw, want) in [
            ("abc", "abc"),
            ("w;-1", "w"),
            ("a{b", "a"),
            (r"sm\:flex(x", r"sm\:flex"),
            (r"\31 0 {", r"\31 0 "),
            ("日本)x", "日本"),
            ("a-b_c9.d", "a-b_c9"),
            ("", ""),
            (";a", ""),
            ("a\\", "a"), // lone trailing backslash is not part of the token
        ] {
            assert_eq!(&raw[..valid_ident_prefix_len(raw)], want, "raw = {raw:?}");
        }
    }

    #[test]
    fn mask_preserves_offsets_and_ascii() {
        let src = "a😀é日b";
        let masked = mask_non_ascii(src);
        assert_eq!(masked.len(), src.len());
        assert!(masked.is_ascii());
        assert_eq!(&masked[..1], "a");
        assert_eq!(&masked[masked.len() - 1..], "b");
        assert!(matches!(mask_non_ascii("abc"), Cow::Borrowed(_)));
    }

    #[test]
    fn hex_escape_terminator_space_belongs_to_the_name_span() {
        // CSS: one whitespace after a hex escape is part of the escape, so the
        // identifier token is `x\1F600 ` (with the space). A rename that replaces
        // this span must re-insert a space if the replacement does not need one.
        let src = ".x\\1F600 {}";
        let defs = CssIndexer::new().index(src);
        assert_eq!(defs.len(), 1);
        assert_eq!(&src[defs[0].name_span.range()], "x\\1F600 ");
        assert_eq!(&*defs[0].name, "x😀");
    }

    #[test]
    fn class_at_is_end_inclusive() {
        let mut ix = CssIndexer::new();
        let src = ".ab {} .c {}";
        let defs = ix.index(src);
        assert_eq!(defs.len(), 2);
        assert!(class_at(&defs, 0).is_none()); // on the dot
        assert_eq!(&*class_at(&defs, 1).unwrap().name, "ab");
        assert_eq!(&*class_at(&defs, 3).unwrap().name, "ab"); // just after 'b'
        assert!(class_at(&defs, 4).is_none());
        assert_eq!(&*class_at(&defs, 8).unwrap().name, "c");
        assert!(class_at(&[], 0).is_none());
    }
}
