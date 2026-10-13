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
    /// `None` when the grammar could not be loaded; `index` then finds nothing.
    parser: Option<Parser>,
    load_error: Option<String>,
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
            Ok(()) => Self { parser: Some(parser), load_error: None },
            Err(error) => Self { parser: None, load_error: Some(error.to_string()) },
        }
    }

    /// Why the grammar failed to load (a tree-sitter ABI mismatch), if it did.
    /// The server reports this at startup instead of crashing.
    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    /// Never fails: broken CSS yields whatever classes could be recovered.
    /// The result is sorted by position.
    pub fn index(&mut self, src: &str) -> Vec<ClassDef> {
        let Some(parser) = self.parser.as_mut() else {
            return Vec::new();
        };
        let masked = mask_non_ascii(src);
        let Some(tree) = parser.parse(masked.as_bytes(), None) else {
            return Vec::new();
        };
        let mut definitions = Vec::new();
        collect(tree.root_node(), src, &mut definitions);
        definitions.sort_by_key(|definition| definition.name_span.start);
        definitions
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
    let mut masked = String::with_capacity(src.len());
    for byte in src.bytes() {
        masked.push(if byte >= 0x80 { '_' } else { byte as char });
    }
    Cow::Owned(masked)
}

/// Find the class under `offset` (end-inclusive, so a cursor right after the
/// last character still counts). `definitions` must be sorted by position.
pub fn class_at(definitions: &[ClassDef], offset: usize) -> Option<&ClassDef> {
    let starting_at_or_before = definitions.partition_point(|definition| definition.name_span.start as usize <= offset);
    let candidate = definitions.get(starting_at_or_before.checked_sub(1)?)?;
    (offset <= candidate.name_span.end as usize).then_some(candidate)
}

fn collect(root: Node, src: &str, definitions: &mut Vec<ClassDef>) {
    let mut cursor = root.walk();
    'walk: loop {
        let node = cursor.node();
        let is_class_token = node.kind() == "class_name"
            && node.parent().is_some_and(|parent| parent.kind() == "class_selector");
        if is_class_token {
            if let Some(definition) = make_definition(node, src) {
                definitions.push(definition);
            }
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

fn make_definition(node: Node, src: &str) -> Option<ClassDef> {
    if node.is_missing() || node.byte_range().is_empty() {
        return None;
    }
    let mut name_range = node.byte_range();
    let raw = src.get(name_range.clone())?;
    // Do not trust the grammar for token boundaries: on garbage like `.w;-1`
    // it can swallow delimiters. Keep only the spec-valid identifier prefix.
    let valid_len = valid_ident_prefix_len(raw);
    if valid_len == 0 {
        return None;
    }
    name_range.end = name_range.start + valid_len;
    let raw = raw.get(..valid_len)?;

    let mut rule_range = None;
    let mut at_rules: Vec<Box<str>> = Vec::new();
    let mut ancestor = node.parent();
    while let Some(current) = ancestor {
        match current.kind() {
            "rule_set" if rule_range.is_none() => rule_range = Some(current.byte_range()),
            "media_statement" | "supports_statement" | "at_rule" => {
                if let Some(prelude_text) = prelude(current, src) {
                    at_rules.push(prelude_text);
                }
            }
            _ => {}
        }
        ancestor = current.parent();
    }
    at_rules.reverse();

    let rule_range = rule_range.unwrap_or_else(|| name_range.clone());
    Some(ClassDef {
        name: unescape_ident(raw).into(),
        name_span: span_of(name_range),
        rule_span: span_of(rule_range),
        context: at_rules,
    })
}

fn span_of(range: Range<usize>) -> Span {
    Span {
        start: range.start as u32,
        end: range.end as u32,
    }
}

/// `@media (min-width: 600px)` for a `media_statement`: source text up to the
/// block, whitespace-collapsed.
fn prelude(node: Node, src: &str) -> Option<Box<str>> {
    let mut cursor = node.walk();
    let block = node.children(&mut cursor).find(|child| child.kind() == "block")?;
    let text = src.get(node.start_byte()..block.start_byte())?;
    let mut collapsed = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        if !collapsed.is_empty() {
            collapsed.push(' ');
        }
        collapsed.push_str(word);
    }
    Some(collapsed.into())
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
            let Some(&(_, escaped)) = chars.peek() else { break };
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

    #[test]
    fn unescape_cases() {
        let cases = [
            ("plain", "plain"),
            (r"sm\:flex", "sm:flex"),
            (r"w-1\/2", "w-1/2"),
            (r"a\.b", "a.b"),
            // hex escape eats one trailing space
            (r"\31 0", "10"),
            // ...but only one
            (r"\31  0", "1 0"),
            // 'x' is not a hex digit, ends the escape
            (r"\32xl\:p-4", "2xl:p-4"),
            (r"\1F600 ", "😀"),
            (r"\e9", "é"),
            // NUL
            (r"\0", "\u{FFFD}"),
            // surrogate
            (r"\D800", "\u{FFFD}"),
            // beyond U+10FFFF
            (r"\110000", "\u{FFFD}"),
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
            // 2nd space follows a plain char: not part of the token
            (r"\31 0 {", r"\31 0"),
            // the space right after a hex escape is
            (r"x\1F600 {", r"x\1F600 "),
            ("日本)x", "日本"),
            ("a-b_c9.d", "a-b_c9"),
            ("", ""),
            (";a", ""),
            // lone trailing backslash is not part of the token
            ("a\\", "a"),
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
        // on the dot
        assert!(class_at(&defs, 0).is_none());
        assert_eq!(&*class_at(&defs, 1).unwrap().name, "ab");
        // just after 'b'
        assert_eq!(&*class_at(&defs, 3).unwrap().name, "ab");
        assert!(class_at(&defs, 4).is_none());
        assert_eq!(&*class_at(&defs, 8).unwrap().name, "c");
        assert!(class_at(&[], 0).is_none());
    }
}
