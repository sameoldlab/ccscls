//! Tests for the test tooling itself, so a typo in the annotation syntax can
//! never silently weaken the fixture tests.

mod common;
use common::*;
use lsp_types::{Position, Range};

#[test]
fn parses_names_raw_and_context() {
    let src = "x\n.a, .b { }  /*~ a, b */\n.sm\\:flex { } /*~ sm\\:flex=>sm:flex */ /*ctx: @layer x | @media (a > b) */\n";
    let got = parse_expectations(src);
    assert_eq!(got.len(), 3);
    assert_eq!((got[0].line, got[0].raw.as_str(), got[0].name.as_str()), (1, "a", "a"));
    assert!(got[0].ctx.is_empty());
    assert_eq!((got[2].line, got[2].raw.as_str(), got[2].name.as_str()), (2, "sm\\:flex", "sm:flex"));
    assert_eq!(got[2].ctx, vec!["@layer x", "@media (a > b)"]);
}

#[test]
fn crlf_lines_are_handled() {
    let got = parse_expectations(".a { } /*~ a */\r\n.b { } /*~ b */\r\n");
    assert_eq!(got.iter().map(|e| e.line).collect::<Vec<_>>(), vec![0, 1]);
}

#[test]
fn lines_without_marker_have_no_expectations() {
    assert!(parse_expectations(".a { }\n/* plain comment */\n").is_empty());
}

#[test]
fn expected_ranges_skip_longer_identifiers_and_count_utf16() {
    // `.btn-large` must not be mistaken for `.btn`; the emoji is 2 UTF-16 units.
    let src = "/* 😀 */ .btn-large .btn { } /*~ btn-large, btn */\n";
    let r: Vec<Range> = expected_ranges(src).into_iter().map(|(_, r)| r).collect();
    // "/* 😀 */ " = 3 + 2 + 4 = 9 units; ".btn-large" name starts at 10, 9 long
    assert_eq!(r[0], Range::new(Position::new(0, 10), Position::new(0, 19)));
    assert_eq!(r[1], Range::new(Position::new(0, 21), Position::new(0, 24)));
}

#[test]
fn cursor_marker_gives_utf16_position() {
    let (text, pos) = parse_cursor("a\n😀é.b$0tn");
    assert_eq!(text, "a\n😀é.btn");
    // 😀=2, é=1, '.'=1, 'b'=1
    assert_eq!(pos, Position::new(1, 5));
}

#[test]
#[should_panic(expected = "no $0")]
fn cursor_marker_is_required() {
    parse_cursor("nothing");
}
