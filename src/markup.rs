//! Class usages in HTML-like markup.
//!
//! A deliberately lexical, forgiving scanner: it only needs to answer "which
//! `class="..."` values are in this file" and it must keep working while the
//! user is mid-edit (unterminated quotes, half-written tags).
//!
//! Template syntax (`{{ }}`, `{% %}`, `<% %>`, `<? ?>`) inside a value makes
//! the whole value opaque: no usages, no completion.

use crate::css::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage {
    pub name: Box<str>,
    pub span: Span,
}

/// The class-name token under the cursor, for completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassContext {
    /// The whole token the cursor is in (empty when between tokens).
    pub replace: Span,
    /// Token text before the cursor.
    pub prefix: Span,
}

/// Every class usage in the document, in source order.
pub fn usages(src: &str) -> Vec<Usage> {
    let mut found = Vec::new();
    for value_span in class_values(src) {
        let Some(value_text) = src.get(value_span.range()) else {
            continue;
        };
        if is_opaque(value_text) {
            continue;
        }
        for (token_start, token) in tokens(value_text) {
            let start = value_span.start as usize + token_start;
            found.push(Usage {
                name: token.into(),
                span: Span {
                    start: start as u32,
                    end: (start + token.len()) as u32,
                },
            });
        }
    }
    found
}

/// If `offset` is inside a class attribute value, the token being edited.
pub fn class_context_at(src: &str, offset: usize) -> Option<ClassContext> {
    for value_span in class_values(src) {
        let (value_start, value_end) = (value_span.start as usize, value_span.end as usize);
        if offset < value_start || offset > value_end {
            continue;
        }
        let value_text = src.get(value_start..value_end)?;
        if is_opaque(value_text) {
            return None;
        }
        let cursor = offset - value_start;
        let before_cursor = value_text.get(..cursor)?;
        let after_cursor = value_text.get(cursor..)?;
        let is_space = |ch: char| ch.is_ascii_whitespace();
        let token_start = before_cursor.rfind(is_space).map_or(0, |index| index + 1);
        let token_end = after_cursor.find(is_space).map_or(value_text.len(), |index| cursor + index);
        return Some(ClassContext {
            replace: Span {
                start: (value_start + token_start) as u32,
                end: (value_start + token_end) as u32,
            },
            prefix: Span {
                start: (value_start + token_start) as u32,
                end: offset as u32,
            },
        });
    }
    None
}

/// The usage under `offset` (end-inclusive). `usages` must be in source order.
pub fn usage_at(usages: &[Usage], offset: usize) -> Option<&Usage> {
    let starting_at_or_before = usages.partition_point(|usage| usage.span.start as usize <= offset);
    let candidate = usages.get(starting_at_or_before.checked_sub(1)?)?;
    (offset <= candidate.span.end as usize).then_some(candidate)
}

fn is_opaque(value: &str) -> bool {
    ["{{", "{%", "<%", "<?"].iter().any(|marker| value.contains(marker))
}

/// Whitespace-separated tokens with their byte offsets within `text`.
fn tokens(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut position = 0;
    std::iter::from_fn(move || {
        let rest = text.get(position..)?;
        let skipped = rest.len() - rest.trim_start_matches(|ch: char| ch.is_ascii_whitespace()).len();
        let start = position + skipped;
        let token_text = text.get(start..)?;
        if token_text.is_empty() {
            return None;
        }
        let length = token_text
            .find(|ch: char| ch.is_ascii_whitespace())
            .unwrap_or(token_text.len());
        position = start + length;
        Some((start, token_text.get(..length)?))
    })
}

/// Byte spans of the inside of every `class` attribute value (excluding quotes).
fn class_values(src: &str) -> Vec<Span> {
    let bytes = src.as_bytes();
    let length = bytes.len();
    let mut values = Vec::new();
    let mut cursor = 0;
    while cursor < length {
        if bytes[cursor] != b'<' {
            cursor += 1;
            continue;
        }
        if bytes[cursor..].starts_with(b"<!--") {
            cursor = find_bytes(bytes, cursor + 4, b"-->").map_or(length, |found| found + 3);
            continue;
        }
        match bytes.get(cursor + 1) {
            Some(next) if next.is_ascii_alphabetic() => {}
            Some(b'/' | b'!' | b'?') => {
                // Closing tag, doctype or processing instruction: skip it whole.
                let search_from = cursor + 1;
                cursor = bytes[search_from..]
                    .iter()
                    .position(|&byte| byte == b'>')
                    .map_or(length, |found| search_from + found + 1);
                continue;
            }
            _ => {
                cursor += 1;
                continue;
            }
        }

        // Tag name.
        let mut scan = cursor + 1;
        while scan < length
            && !bytes[scan].is_ascii_whitespace()
            && bytes[scan] != b'/'
            && bytes[scan] != b'>'
        {
            scan += 1;
        }
        let tag_name = src.get(cursor + 1..scan).unwrap_or("").to_ascii_lowercase();

        // Attributes.
        loop {
            while scan < length && (bytes[scan].is_ascii_whitespace() || bytes[scan] == b'/') {
                scan += 1;
            }
            if scan >= length {
                break;
            }
            if bytes[scan] == b'>' {
                scan += 1;
                break;
            }
            let name_start = scan;
            while scan < length
                && !bytes[scan].is_ascii_whitespace()
                && !matches!(bytes[scan], b'=' | b'/' | b'>')
            {
                scan += 1;
            }
            if scan == name_start {
                // Stray `=` or quote: step over it so the scan always advances.
                scan += 1;
                continue;
            }
            let is_class = src
                .get(name_start..scan)
                .is_some_and(|name| name.eq_ignore_ascii_case("class"));
            while scan < length && bytes[scan].is_ascii_whitespace() {
                scan += 1;
            }
            if bytes.get(scan) != Some(&b'=') {
                continue;
            }
            scan += 1;
            while scan < length && bytes[scan].is_ascii_whitespace() {
                scan += 1;
            }
            match bytes.get(scan) {
                Some(&quote @ (b'"' | b'\'')) => {
                    let value_start = scan + 1;
                    let value_end = bytes[value_start..]
                        .iter()
                        .position(|&byte| byte == quote)
                        .map_or(length, |found| value_start + found);
                    if is_class {
                        values.push(Span {
                            start: value_start as u32,
                            end: value_end as u32,
                        });
                    }
                    scan = (value_end + 1).min(length);
                }
                Some(_) => {
                    let value_start = scan;
                    while scan < length && !bytes[scan].is_ascii_whitespace() && bytes[scan] != b'>' {
                        scan += 1;
                    }
                    if is_class {
                        values.push(Span {
                            start: value_start as u32,
                            end: scan as u32,
                        });
                    }
                }
                None => {}
            }
        }

        // Raw-text elements: their content is not markup.
        if tag_name == "script" || tag_name == "style" {
            let closing_tag = format!("</{tag_name}");
            cursor = find_bytes_ignore_case(bytes, scan, closing_tag.as_bytes()).unwrap_or(length);
        } else {
            cursor = scan;
        }
    }
    values
}

fn find_bytes(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|found| from + found)
}

fn find_bytes_ignore_case(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
        .map(|found| from + found)
}
