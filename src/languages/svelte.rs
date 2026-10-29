//! Svelte components: class names in `class="..."` attributes, `class:name`
//! directives, and string literals and object keys inside `class={...}`
//! expressions become usages. `<style>` blocks become private CSS.
//!
//! Svelte is HTML plus `{...}` expressions, and an expression may contain `>`,
//! spaces and quotes, so the scanner skips balanced braces everywhere (in text,
//! between attributes, and inside attribute values) instead of treating those
//! characters as markup. Inside a `class` expression it looks for the three
//! places a class name can be written:
//!
//! * a string literal (`'card'`, `"card"`, template text) holding class names,
//! * a bare object key (`{ active, 'is-big': big, spaced: x }`),
//! * the name of a `class:name` directive.
//!
//! This is lexical, not a JavaScript parser: a string literal inside a class
//! expression is assumed to be a class name.

use super::html::{find_bytes, find_bytes_ignore_case, tokens};
use super::{ClassContext, Language, Region};
use crate::analysis::{FileAnalysis, Role, Span};
use crate::strings::Strings;

/// Fill `out` with usages and return the `<style>` blocks for the core to
/// analyze as CSS. A component's CSS also comes from outside it (global
/// stylesheets imported elsewhere), so it is always open-world.
pub(super) fn analyze(src: &str, strings: &mut Strings, out: &mut FileAnalysis) -> Vec<Region> {
    let scanned = scan_component(src);
    for part in &scanned.parts {
        let Some(text) = src.get(part.span.range()) else {
            continue;
        };
        match part.kind {
            PartKind::Words => {
                for (token_start, token) in tokens(text) {
                    let start = part.span.start as usize + token_start;
                    let span = Span::of(start..start + token.len());
                    out.add(Role::Usage, strings.intern(token), span, span, &[]);
                }
            }
            PartKind::Directive => {
                if !text.is_empty() {
                    out.add(Role::Usage, strings.intern(text), part.span, part.span, &[]);
                }
            }
        }
    }
    out.open_world = true;
    scanned
        .style_regions
        .iter()
        .map(|span| Region {
            language: Language::Css,
            span: *span,
        })
        .collect()
}

/// If `offset` is where a class name is being written, the token being edited.
pub(super) fn class_context_at(src: &str, offset: usize) -> Option<ClassContext> {
    for part in scan_component(src).parts {
        let (start, end) = (part.span.start as usize, part.span.end as usize);
        if offset < start || offset > end {
            continue;
        }
        let text = src.get(start..end)?;
        match part.kind {
            PartKind::Directive => {
                return Some(ClassContext {
                    replace: part.span,
                    prefix: Span::of(start..offset),
                });
            }
            PartKind::Words => {
                let cursor = offset - start;
                let before_cursor = text.get(..cursor)?;
                let after_cursor = text.get(cursor..)?;
                let is_space = |ch: char| ch.is_ascii_whitespace();
                let token_start = before_cursor.rfind(is_space).map_or(0, |index| index + 1);
                let token_end = after_cursor
                    .find(is_space)
                    .map_or(text.len(), |index| cursor + index);
                return Some(ClassContext {
                    replace: Span::of(start + token_start..start + token_end),
                    prefix: Span::of(start + token_start..offset),
                });
            }
        }
    }
    None
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PartKind {
    /// Whitespace-separated class names.
    Words,
    /// The name in `class:name`: exactly one class name, possibly still empty.
    Directive,
}

#[derive(Clone, Copy)]
struct Part {
    kind: PartKind,
    span: Span,
}

#[derive(Default)]
struct Scan {
    parts: Vec<Part>,
    /// The `<style>` blocks that are plain CSS.
    style_regions: Vec<Span>,
}

/// One piece of an attribute value.
enum Chunk {
    /// Literal text.
    Static(Span),
    /// The inside of a `{...}` expression, without the braces.
    Expression(Span),
}

fn scan_component(src: &str) -> Scan {
    let bytes = src.as_bytes();
    let length = bytes.len();
    let mut found = Scan::default();
    let mut cursor = 0;
    while cursor < length {
        match bytes[cursor] {
            // A block or expression in the text: `{#if a < b}`, `{value}`, `{@html x}`.
            b'{' => {
                cursor = skip_braced(bytes, cursor);
                continue;
            }
            b'<' => {}
            _ => {
                cursor += 1;
                continue;
            }
        }
        if bytes[cursor..].starts_with(b"<!--") {
            cursor = find_bytes(bytes, cursor + 4, b"-->").map_or(length, |end| end + 3);
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
                    .map_or(length, |end| search_from + end + 1);
                continue;
            }
            _ => {
                cursor += 1;
                continue;
            }
        }
        cursor = scan_tag(src, cursor, &mut found);
    }
    found
}

/// Scan the tag that starts at `tag_start` (a `<` followed by a letter).
/// Returns where to continue scanning.
fn scan_tag(src: &str, tag_start: usize, found: &mut Scan) -> usize {
    let bytes = src.as_bytes();
    let length = bytes.len();
    let mut scan = tag_start + 1;
    while scan < length
        && !bytes[scan].is_ascii_whitespace()
        && bytes[scan] != b'/'
        && bytes[scan] != b'>'
    {
        scan += 1;
    }
    let tag_name = src
        .get(tag_start + 1..scan)
        .unwrap_or("")
        .to_ascii_lowercase();

    let mut self_closing = false;
    let mut style_is_css = true;
    loop {
        while scan < length && (bytes[scan].is_ascii_whitespace() || bytes[scan] == b'/') {
            scan += 1;
        }
        if scan >= length {
            break;
        }
        match bytes[scan] {
            b'>' => {
                self_closing = scan >= 2 && bytes[scan - 1] == b'/';
                scan += 1;
                break;
            }
            // Spread or shorthand attribute: `{...props}`, `{value}`.
            b'{' => {
                scan = skip_braced(bytes, scan);
                continue;
            }
            _ => {}
        }
        let name_start = scan;
        while scan < length
            && !bytes[scan].is_ascii_whitespace()
            && !matches!(bytes[scan], b'=' | b'/' | b'>' | b'{')
        {
            scan += 1;
        }
        if scan == name_start {
            // Stray quote or `=`: step over it so the scan always advances.
            scan += 1;
            continue;
        }
        let attribute_name = src.get(name_start..scan).unwrap_or("");
        if attribute_name.starts_with("class:") {
            found.parts.push(Part {
                kind: PartKind::Directive,
                span: Span::of(name_start + "class:".len()..scan),
            });
        }

        let mut after_name = scan;
        while after_name < length && bytes[after_name].is_ascii_whitespace() {
            after_name += 1;
        }
        if bytes.get(after_name) != Some(&b'=') {
            continue;
        }
        scan = after_name + 1;
        while scan < length && bytes[scan].is_ascii_whitespace() {
            scan += 1;
        }
        let (value_end, chunks) = match bytes.get(scan) {
            Some(&quote @ (b'"' | b'\'')) => read_value(bytes, scan + 1, Some(quote)),
            Some(_) => read_value(bytes, scan, None),
            None => break,
        };
        scan = value_end;

        if attribute_name.eq_ignore_ascii_case("class") {
            for chunk in &chunks {
                match chunk {
                    Chunk::Static(span) => found.parts.push(Part {
                        kind: PartKind::Words,
                        span: *span,
                    }),
                    Chunk::Expression(span) => {
                        collect_expression_words(
                            bytes,
                            span.start as usize,
                            span.end as usize,
                            &mut found.parts,
                        );
                    }
                }
            }
        } else if tag_name == "style" && (attribute_name == "lang" || attribute_name == "type") {
            if let [Chunk::Static(span)] = chunks.as_slice() {
                let value = src.get(span.range()).unwrap_or("");
                style_is_css &=
                    value.eq_ignore_ascii_case("css") || value.eq_ignore_ascii_case("text/css");
            }
        }
    }

    // Raw-text elements: their content is not markup.
    if (tag_name == "script" || tag_name == "style") && !self_closing {
        let closing_tag = format!("</{tag_name}");
        let content_end =
            find_bytes_ignore_case(bytes, scan, closing_tag.as_bytes()).unwrap_or(length);
        if tag_name == "style" && style_is_css && content_end > scan {
            found.style_regions.push(Span::of(scan..content_end));
        }
        return content_end;
    }
    scan
}

/// Read an attribute value that starts at `start`, splitting it into literal
/// text and `{...}` expressions. `quote` is the closing quote of a quoted
/// value; an unquoted value ends at whitespace or `>`. Returns the position
/// after the value (after the closing quote, if any).
fn read_value(bytes: &[u8], start: usize, quote: Option<u8>) -> (usize, Vec<Chunk>) {
    let length = bytes.len();
    let mut chunks = Vec::new();
    let mut cursor = start;
    let mut static_start = start;
    while cursor < length {
        let byte = bytes[cursor];
        if byte == b'{' {
            if cursor > static_start {
                chunks.push(Chunk::Static(Span::of(static_start..cursor)));
            }
            let end = skip_braced(bytes, cursor);
            let is_closed = end > cursor + 1 && bytes.get(end - 1) == Some(&b'}');
            let inner_end = if is_closed { end - 1 } else { end };
            chunks.push(Chunk::Expression(Span::of(
                cursor + 1..inner_end.max(cursor + 1),
            )));
            cursor = end;
            static_start = cursor;
            continue;
        }
        let ends_value = match quote {
            Some(closing) => byte == closing,
            None => byte.is_ascii_whitespace() || byte == b'>',
        };
        if ends_value {
            break;
        }
        cursor += 1;
    }
    // An empty quoted value (`class=""`) still gets an empty chunk, so the cursor
    // inside it is a place where a class name can be written.
    if cursor > static_start || (quote.is_some() && chunks.is_empty()) {
        chunks.push(Chunk::Static(Span::of(static_start..cursor.min(length))));
    }
    let after = if quote.is_some() && cursor < length {
        cursor + 1
    } else {
        cursor
    };
    (after, chunks)
}

/// The position just after the `}` that closes the `{` at `open`, or the end
/// of the input if it is never closed. Strings, template literals and
/// comments inside the expression are skipped, so a brace in one does not count.
fn skip_braced(bytes: &[u8], open: usize) -> usize {
    let length = bytes.len();
    let mut depth = 0usize;
    let mut cursor = open;
    while cursor < length {
        match bytes[cursor] {
            b'{' => {
                depth += 1;
                cursor += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                cursor += 1;
                if depth == 0 {
                    return cursor;
                }
            }
            b'\'' | b'"' | b'`' => cursor = skip_string(bytes, cursor),
            b'/' if bytes.get(cursor + 1) == Some(&b'/') => {
                cursor = bytes[cursor..]
                    .iter()
                    .position(|&byte| byte == b'\n')
                    .map_or(length, |end| cursor + end);
            }
            b'/' if bytes.get(cursor + 1) == Some(&b'*') => {
                cursor = find_bytes(bytes, cursor + 2, b"*/").map_or(length, |end| end + 2);
            }
            _ => cursor += 1,
        }
    }
    length
}

/// The position just after the string literal that starts at `start` (which
/// holds the opening quote), or the end of the input if it is not closed.
fn skip_string(bytes: &[u8], start: usize) -> usize {
    let length = bytes.len();
    let quote = bytes[start];
    let mut cursor = start + 1;
    while cursor < length {
        match bytes[cursor] {
            b'\\' => cursor += 2,
            byte if byte == quote => return cursor + 1,
            b'$' if quote == b'`' && bytes.get(cursor + 1) == Some(&b'{') => {
                cursor = skip_braced(bytes, cursor + 1);
            }
            _ => cursor += 1,
        }
    }
    length
}

fn is_identifier_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$'
}

fn is_identifier_part(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

fn next_significant(bytes: &[u8], from: usize, to: usize) -> Option<u8> {
    bytes
        .get(from..to.min(bytes.len()))?
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
}

/// Class-name candidates in the JavaScript-like text `bytes[from..to]`:
/// string literals, template literal text, and bare object keys.
fn collect_expression_words(bytes: &[u8], from: usize, to: usize, parts: &mut Vec<Part>) {
    let to = to.min(bytes.len());
    let mut brackets: Vec<u8> = Vec::new();
    // The last non-space byte before the cursor, outside literals. An
    // identifier counts as the letter `a`.
    let mut previous = 0u8;
    let mut cursor = from;
    while cursor < to {
        let byte = bytes[cursor];
        match byte {
            b'\'' | b'"' => {
                let end = skip_string(bytes, cursor).min(to);
                let is_closed = end > cursor + 1 && bytes.get(end - 1) == Some(&byte);
                let content_end = if is_closed { end - 1 } else { end };
                parts.push(Part {
                    kind: PartKind::Words,
                    span: Span::of(cursor + 1..content_end.max(cursor + 1)),
                });
                previous = byte;
                cursor = end;
            }
            b'`' => {
                cursor = collect_template_words(bytes, cursor, to, parts);
                previous = b'`';
            }
            b'/' if bytes.get(cursor + 1) == Some(&b'/') => {
                cursor = bytes[cursor..to]
                    .iter()
                    .position(|&other| other == b'\n')
                    .map_or(to, |end| cursor + end);
            }
            b'/' if bytes.get(cursor + 1) == Some(&b'*') => {
                cursor = find_bytes(bytes, cursor + 2, b"*/").map_or(to, |end| (end + 2).min(to));
            }
            b'{' | b'[' | b'(' => {
                brackets.push(byte);
                previous = byte;
                cursor += 1;
            }
            b'}' | b']' | b')' => {
                brackets.pop();
                previous = byte;
                cursor += 1;
            }
            _ if byte.is_ascii_whitespace() => cursor += 1,
            _ if is_identifier_start(byte) => {
                let start = cursor;
                while cursor < to && is_identifier_part(bytes[cursor]) {
                    cursor += 1;
                }
                let in_object = brackets.last() == Some(&b'{');
                let after_separator = previous == b'{' || previous == b',';
                let next = next_significant(bytes, cursor, to);
                if in_object && after_separator && matches!(next, Some(b':' | b',' | b'}')) {
                    parts.push(Part {
                        kind: PartKind::Words,
                        span: Span::of(start..cursor),
                    });
                }
                previous = b'a';
            }
            _ => {
                previous = byte;
                cursor += 1;
            }
        }
    }
}

/// Class-name text in the template literal that starts at `start` (the
/// backtick), including the literals inside its `${...}` parts. Returns the
/// position after the closing backtick, or `to` if it is not closed.
fn collect_template_words(bytes: &[u8], start: usize, to: usize, parts: &mut Vec<Part>) -> usize {
    let mut cursor = start + 1;
    let mut text_start = cursor;
    while cursor < to {
        match bytes[cursor] {
            b'\\' => cursor += 2,
            b'`' => {
                parts.push(Part {
                    kind: PartKind::Words,
                    span: Span::of(text_start..cursor),
                });
                return cursor + 1;
            }
            b'$' if bytes.get(cursor + 1) == Some(&b'{') => {
                if cursor > text_start {
                    parts.push(Part {
                        kind: PartKind::Words,
                        span: Span::of(text_start..cursor),
                    });
                }
                let end = skip_braced(bytes, cursor + 1).min(to);
                let is_closed = end > cursor + 2 && bytes.get(end - 1) == Some(&b'}');
                let inner_end = if is_closed { end - 1 } else { end };
                collect_expression_words(bytes, cursor + 2, inner_end, parts);
                cursor = end;
                text_start = cursor;
            }
            _ => cursor += 1,
        }
    }
    let end = cursor.min(to);
    if end > text_start {
        parts.push(Part {
            kind: PartKind::Words,
            span: Span::of(text_start..end),
        });
    }
    to
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(source: &str) -> (FileAnalysis, Strings, Vec<Region>) {
        let mut strings = Strings::new();
        let mut out = FileAnalysis::default();
        let regions = analyze(source, &mut strings, &mut out);
        (out, strings, regions)
    }

    fn used_names(source: &str) -> Vec<String> {
        let (out, strings, _) = run(source);
        out.occurrences
            .iter()
            .map(|occurrence| strings.get(occurrence.name).to_owned())
            .collect()
    }

    /// The class names found in a bare expression, as `class={...}` would hold.
    fn expression_names(expression: &str) -> Vec<String> {
        let mut parts = Vec::new();
        collect_expression_words(expression.as_bytes(), 0, expression.len(), &mut parts);
        let mut names = Vec::new();
        for part in parts {
            let text = expression.get(part.span.range()).unwrap_or("");
            names.extend(tokens(text).map(|(_, token)| token.to_owned()));
        }
        names
    }

    #[test]
    fn braces_are_skipped_as_a_unit() {
        let cases = [
            ("{a}x", 3),
            ("{a{b}c}x", 7),
            ("{'}'}x", 5),
            ("{\"}\"}x", 5),
            ("{`${a}}`}x", 9),
            ("{// }\n}x", 7),
            ("{/* } */}x", 9),
            ("{}x", 2),
            ("{unclosed", 9),
            ("{'unclosed", 10),
        ];
        for (source, expected) in cases {
            assert_eq!(
                skip_braced(source.as_bytes(), 0),
                expected,
                "source = {source:?}"
            );
        }
    }

    #[test]
    fn expressions_yield_string_literals_template_text_and_object_keys() {
        let cases: [(&str, &[&str]); 12] = [
            ("active ? 'on' : 'off'", &["on", "off"]),
            ("['home', active && 'btn-large']", &["home", "btn-large"]),
            (
                "{ active, 'is-big': big, spaced: x }",
                &["active", "is-big", "spaced"],
            ),
            ("`btn ${size} ${on ? 'x' : 'y'}`", &["btn", "x", "y"]),
            ("'a b  c'", &["a", "b", "c"]),
            // Bare identifiers are variables, not class names.
            ("[a, b, c]", &[]),
            ("fn(a, b)", &[]),
            ("cond ? a : b", &[]),
            ("{ a: x ? b : c }", &["a"]),
            ("'' || ''", &[]),
            ("a /* 'no' */ + 'yes' // 'nope'", &["yes"]),
            (
                "{ 'quoted': 1, plain: 2, third }",
                &["quoted", "plain", "third"],
            ),
        ];
        for (expression, expected) in cases {
            assert_eq!(
                expression_names(expression),
                expected,
                "expression = {expression:?}"
            );
        }
    }

    #[test]
    fn static_values_expressions_and_directives_all_produce_usages() {
        let source = concat!(
            "<div class=\"btn {active ? 'card' : ''} page\"></div>",
            "<p class={['home', on && 'x']}></p>",
            "<button class:active class:btn-large={big}></button>",
        );
        assert_eq!(
            used_names(source),
            ["btn", "card", "page", "home", "x", "active", "btn-large"]
        );
    }

    #[test]
    fn other_attributes_text_and_script_content_are_not_classes() {
        let source = concat!(
            "<script>let c = '<p class=\"in-script\">';</script>",
            "{#if a < b}<p id=\"btn\" data-class=\"nope\" classes=\"nope2\">{c} class=\"text\"</p>{/if}",
            "{@html '<div class=\"in-html\">'}",
            "<!-- <p class=\"commented\"> -->",
            "<input disabled {value} {...rest} on:click={() => n++} bind:value />",
            "<p class:ok class=\"yes\"></p>",
        );
        assert_eq!(used_names(source), ["ok", "yes"]);
    }

    #[test]
    fn a_greater_than_inside_an_expression_does_not_end_the_tag() {
        let source = "<p on:click={() => go(a > b)} class=\"after\"></p>";
        assert_eq!(used_names(source), ["after"]);
    }

    #[test]
    fn spans_point_at_the_exact_text() {
        let source = "<p class=\"a {x ? 'bb' : ''} ccc\" class:dd></p>";
        let (out, _, _) = run(source);
        let texts: Vec<&str> = out
            .occurrences
            .iter()
            .filter_map(|occurrence| source.get(occurrence.name_span.range()))
            .collect();
        assert_eq!(texts, ["a", "bb", "ccc", "dd"]);
    }

    #[test]
    fn style_blocks_are_regions_unless_they_are_another_language() {
        let (_, _, regions) = run(
            "<style>.a { }</style><style lang=\"scss\">.b { }</style><style lang=\"css\">.c { }</style>",
        );
        assert_eq!(regions.len(), 2);
        let (_, _, none) = run(
            "<style lang=\"postcss\">.a { }</style><script>let a = '<style>.x</style>';</script>",
        );
        assert!(none.is_empty());
    }

    #[test]
    fn components_are_open_world() {
        let (out, _, _) = run("<p class=\"a\"></p>");
        assert!(out.open_world);
    }

    #[test]
    fn half_typed_markup_never_panics_and_keeps_what_was_typed() {
        assert_eq!(used_names("<div class=\"typing ba"), ["typing", "ba"]);
        assert_eq!(used_names("<div class={['a', 'b"), ["a", "b"]);
        for source in [
            "<div class:",
            "<div class=",
            "<div class={",
            "{#if",
            "<",
            "{",
            "<div {",
            "<p class=\"{x",
        ] {
            let _ = used_names(source);
        }
    }
}
