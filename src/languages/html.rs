//! HTML-like markup: class names in `class="..."` attributes become usages.
//! Stylesheet links and `<style>` blocks feed the import graph.
//!
//! A deliberately lexical, forgiving scanner: it only needs to answer "which
//! `class="..."` values are in this file" and it must keep working while the
//! user is mid-edit (unterminated quotes, half-written tags).
//!
//! Template syntax (`{{ }}`, `{% %}`, `<% %>`, `<? ?>`) inside a value makes
//! the whole value opaque: no usages, no completion.

use super::{ClassContext, Language, Region};
use crate::analysis::{FileAnalysis, Import, Role, Span};
use crate::strings::Strings;

/// Fill `out` with usages and stylesheet links. Returns the `<style>` blocks
/// for the core to analyze as CSS.
pub(super) fn analyze(src: &str, strings: &mut Strings, out: &mut FileAnalysis) -> Vec<Region> {
    let scanned = scan_document(src);
    for value_span in &scanned.class_values {
        let Some(value_text) = src.get(value_span.range()) else {
            continue;
        };
        if is_opaque(value_text) {
            continue;
        }
        for (token_start, token) in tokens(value_text) {
            let start = value_span.start as usize + token_start;
            let span = Span::of(start..start + token.len());
            out.add(Role::Usage, strings.intern(token), span, span, &[]);
        }
    }
    for href_span in &scanned.stylesheet_links {
        let Some(href) = src
            .get(href_span.range())
            .map(str::trim)
            .filter(|text| !text.is_empty())
        else {
            continue;
        };
        out.imports.push(Import {
            specifier: strings.intern(href),
            span: *href_span,
            condition: None,
        });
    }
    out.open_world = !scanned.has_document_marker || scanned.has_script || scanned.has_base;
    scanned
        .style_regions
        .iter()
        .map(|span| Region {
            language: Language::Css,
            span: *span,
        })
        .collect()
}

/// If `offset` is inside a class attribute value, the token being edited.
pub(super) fn class_context_at(src: &str, offset: usize) -> Option<ClassContext> {
    for value_span in scan_document(src).class_values {
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
        let token_end = after_cursor
            .find(is_space)
            .map_or(value_text.len(), |index| cursor + index);
        return Some(ClassContext {
            replace: Span::of(value_start + token_start..value_start + token_end),
            prefix: Span::of(value_start + token_start..offset),
        });
    }
    None
}

fn is_opaque(value: &str) -> bool {
    ["{{", "{%", "<%", "<?"]
        .iter()
        .any(|marker| value.contains(marker))
}

/// Whitespace-separated tokens with their byte offsets within `text`.
fn tokens(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut position = 0;
    std::iter::from_fn(move || {
        let rest = text.get(position..)?;
        let skipped = rest.len()
            - rest
                .trim_start_matches(|ch: char| ch.is_ascii_whitespace())
                .len();
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

/// Everything one pass over a document finds.
#[derive(Default)]
struct Scan {
    /// The inside of every `class` attribute value (excluding quotes).
    class_values: Vec<Span>,
    /// The `href` value of every `<link rel="stylesheet">`.
    stylesheet_links: Vec<Span>,
    /// The content of every `<style>` element.
    style_regions: Vec<Span>,
    /// `<!doctype>`, `<html>` or `<head>`: this is a whole page, not a fragment.
    has_document_marker: bool,
    has_script: bool,
    has_base: bool,
}

fn scan_document(src: &str) -> Scan {
    let bytes = src.as_bytes();
    let length = bytes.len();
    let mut found = Scan::default();
    let mut cursor = 0;
    while cursor < length {
        if bytes[cursor] != b'<' {
            cursor += 1;
            continue;
        }
        if bytes[cursor..].starts_with(b"<!--") {
            cursor = find_bytes(bytes, cursor + 4, b"-->").map_or(length, |end| end + 3);
            continue;
        }
        match bytes.get(cursor + 1) {
            Some(next) if next.is_ascii_alphabetic() => {}
            Some(b'/' | b'!' | b'?') => {
                let is_doctype = bytes.get(cursor + 1) == Some(&b'!')
                    && bytes
                        .get(cursor + 2..cursor + 9)
                        .is_some_and(|word| word.eq_ignore_ascii_case(b"doctype"));
                found.has_document_marker |= is_doctype;
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
        match tag_name.as_str() {
            "html" | "head" => found.has_document_marker = true,
            "script" => found.has_script = true,
            "base" => found.has_base = true,
            _ => {}
        }

        // Attributes.
        let mut rel_value: Option<Span> = None;
        let mut href_value: Option<Span> = None;
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
            let attribute_name = src.get(name_start..scan).unwrap_or("");
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
            let value_span = match bytes.get(scan) {
                Some(&quote @ (b'"' | b'\'')) => {
                    let value_start = scan + 1;
                    let value_end = bytes[value_start..]
                        .iter()
                        .position(|&byte| byte == quote)
                        .map_or(length, |end| value_start + end);
                    scan = (value_end + 1).min(length);
                    Some(Span::of(value_start..value_end))
                }
                Some(_) => {
                    let value_start = scan;
                    while scan < length && !bytes[scan].is_ascii_whitespace() && bytes[scan] != b'>'
                    {
                        scan += 1;
                    }
                    Some(Span::of(value_start..scan))
                }
                None => None,
            };
            let Some(value_span) = value_span else {
                continue;
            };
            if attribute_name.eq_ignore_ascii_case("class") {
                found.class_values.push(value_span);
            } else if attribute_name.eq_ignore_ascii_case("rel") {
                rel_value = Some(value_span);
            } else if attribute_name.eq_ignore_ascii_case("href") {
                href_value = Some(value_span);
            }
        }

        if tag_name == "link" {
            let is_stylesheet = rel_value
                .and_then(|span| src.get(span.range()))
                .is_some_and(|rel| {
                    rel.split_ascii_whitespace()
                        .any(|token| token.eq_ignore_ascii_case("stylesheet"))
                });
            if is_stylesheet {
                if let Some(href) = href_value {
                    found.stylesheet_links.push(href);
                }
            }
        }

        // Raw-text elements: their content is not markup.
        if tag_name == "script" || tag_name == "style" {
            let closing_tag = format!("</{tag_name}");
            let content_end =
                find_bytes_ignore_case(bytes, scan, closing_tag.as_bytes()).unwrap_or(length);
            if tag_name == "style" && content_end > scan {
                found.style_regions.push(Span::of(scan..content_end));
            }
            cursor = content_end;
        } else {
            cursor = scan;
        }
    }
    found
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

#[cfg(test)]
mod tests {
    use super::*;

    fn run(source: &str) -> (FileAnalysis, Strings, Vec<Region>) {
        let mut strings = Strings::new();
        let mut out = FileAnalysis::default();
        let regions = analyze(source, &mut strings, &mut out);
        (out, strings, regions)
    }

    fn hrefs(source: &str) -> Vec<String> {
        let (out, strings, _) = run(source);
        out.imports
            .iter()
            .map(|import| strings.get(import.specifier).to_owned())
            .collect()
    }

    fn style_texts(source: &str) -> Vec<String> {
        let (_, _, regions) = run(source);
        regions
            .iter()
            .filter_map(|region| source.get(region.span.range()))
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn only_stylesheet_links_are_imports() {
        let cases: [(&str, &[&str]); 9] = [
            (r#"<link rel="stylesheet" href="a.css">"#, &["a.css"]),
            (r#"<link href="a.css" rel="stylesheet">"#, &["a.css"]),
            (r#"<LINK REL="Stylesheet" HREF="a.css"/>"#, &["a.css"]),
            ("<link rel=stylesheet href=a.css>", &["a.css"]),
            (
                r#"<link rel="alternate stylesheet" href="a.css">"#,
                &["a.css"],
            ),
            (r#"<link rel="icon" href="favicon.ico">"#, &[]),
            (r#"<link rel="stylesheet">"#, &[]),
            (r#"<link rel="stylesheet" href="">"#, &[]),
            (
                r#"<link rel="stylesheet" href="a.css"><link rel="stylesheet" href="b.css">"#,
                &["a.css", "b.css"],
            ),
        ];
        for (source, expected) in cases {
            assert_eq!(hrefs(source), expected, "source = {source:?}");
        }
    }

    #[test]
    fn stylesheet_link_spans_point_at_the_href_text() {
        let source = r#"<link rel="stylesheet" href="css/site.css">"#;
        let (out, _, _) = run(source);
        assert_eq!(out.imports.len(), 1);
        assert_eq!(
            source.get(out.imports[0].span.range()),
            Some("css/site.css")
        );
    }

    #[test]
    fn links_inside_comments_scripts_and_styles_are_ignored() {
        let source = concat!(
            "<!-- <link rel=\"stylesheet\" href=\"commented.css\"> -->",
            "<script>document.write('<link rel=\"stylesheet\" href=\"script.css\">')</script>",
            "<style>/* <link rel=\"stylesheet\" href=\"style.css\"> */</style>",
        );
        assert!(hrefs(source).is_empty());
    }

    #[test]
    fn style_elements_become_regions_with_exact_content() {
        assert_eq!(style_texts("<style>.a { }</style>"), [".a { }"]);
        assert_eq!(
            style_texts("<style media=\"print\">.a { }</style><p><STYLE>.b { }</STYLE>"),
            [".a { }", ".b { }"]
        );
        assert_eq!(style_texts("<style></style>"), Vec::<String>::new());
        assert_eq!(
            style_texts("<!-- <style>.a { }</style> -->"),
            Vec::<String>::new()
        );
        assert_eq!(style_texts("<style>.typing { "), [".typing { "]);
    }

    #[test]
    fn whole_pages_without_scripts_are_closed_world() {
        let closed = [
            "<!doctype html><p>",
            "<!DOCTYPE html><div>",
            "<html><body>",
            "<head></head><body>",
        ];
        for source in closed {
            let (out, _, _) = run(source);
            assert!(!out.open_world, "source = {source:?}");
        }
    }

    #[test]
    fn fragments_scripts_and_base_make_a_file_open_world() {
        let open = [
            "<div class=\"a\">",
            "<!doctype html><html><head><script src=\"x.js\"></script></head>",
            "<!doctype html><script>run()</script>",
            "<!doctype html><base href=\"/app/\">",
        ];
        for source in open {
            let (out, _, _) = run(source);
            assert!(out.open_world, "source = {source:?}");
        }
    }
}
