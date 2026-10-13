//! Level 1: cursor context inside HTML class attributes (drives completion),
//! using `$0` as the cursor marker.

mod common;
use common::parse_cursor;
use css_class_lsp::line_index::LineIndex;
use css_class_lsp::markup::{self, class_context_at};

/// (text before the cursor's token, whole token) at `$0`, or None if the cursor
/// is not in a class value.
fn ctx(marked: &str) -> Option<(String, String)> {
    let (text, pos) = parse_cursor(marked);
    let offset = LineIndex::new(&text).offset(&text, pos);
    let c = class_context_at(&text, offset)?;
    Some((text[c.prefix.range()].to_owned(), text[c.replace.range()].to_owned()))
}

fn some(prefix: &str, token: &str) -> Option<(String, String)> {
    Some((prefix.to_owned(), token.to_owned()))
}

#[test]
fn inside_and_between_tokens() {
    assert_eq!(ctx(r#"<div class="$0">"#), some("", ""));
    assert_eq!(ctx(r#"<div class="bt$0">"#), some("bt", "bt"));
    // mid-token: replace whole token
    assert_eq!(ctx(r#"<div class="b$0tn">"#), some("b", "btn"));
    assert_eq!(ctx(r#"<div class="$0btn">"#), some("", "btn"));
    // right before the closing quote
    assert_eq!(ctx(r#"<div class="btn$0">"#), some("btn", "btn"));
    assert_eq!(ctx(r#"<div class="a $0 b">"#), some("", ""));
    assert_eq!(ctx(r#"<div class="a b$0 c">"#), some("b", "b"));
    assert_eq!(ctx("<div class='x$0'>"), some("x", "x"));
    assert_eq!(ctx("<div class=x$0>"), some("x", "x"));
}

#[test]
fn outside_class_values() {
    assert_eq!(ctx(r#"<div id="$0">"#), None);
    assert_eq!(ctx(r#"<div data-class="a$0">"#), None);
    assert_eq!(ctx(r#"<div class="a" $0>"#), None);
    assert_eq!(ctx(r#"<div class="a">te$0xt</div>"#), None);
    assert_eq!(ctx(r#"<div $0 class="a">"#), None);
    assert_eq!(ctx(r#"<!-- <div class="a$0"> -->"#), None);
    assert_eq!(ctx(r#"<script>x = '<div class="a$0">'</script>"#), None);
    assert_eq!(ctx(r#"<style>.a[class="b$0"] {}</style>"#), None);
    assert_eq!(ctx(r#"<p>class="a$0"</p>"#), None);
    // template syntax: opaque
    assert_eq!(ctx(r#"<div class="{{ x$0 }}">"#), None);
}

#[test]
fn class_values_after_other_constructs() {
    assert_eq!(ctx("<!-- c --><p class=\"a$0\">"), some("a", "a"));
    assert_eq!(ctx("<script>let a;</script><p class=\"a$0\">"), some("a", "a"));
    assert_eq!(ctx("<style>.x{}</style>\n<p class=\"a$0\">"), some("a", "a"));
    assert_eq!(ctx("<?xml v?>\n<!doctype html>\n<p class=\"a$0\">"), some("a", "a"));
    assert_eq!(ctx("</div><p class=\"a$0\">"), some("a", "a"));
}

#[test]
fn unterminated_value_while_typing() {
    assert_eq!(ctx(r#"<p class="typing ba$0"#), some("ba", "ba"));
    assert_eq!(ctx(r#"<p class="$0"#), some("", ""));
    assert_eq!(ctx(r#"<p class="a b$0"#), some("b", "b"));
    let names: Vec<_> = markup::usages(r#"<p class="typing ba"#).into_iter().map(|u| u.name).collect();
    assert_eq!(names.iter().map(|n| &**n).collect::<Vec<_>>(), vec!["typing", "ba"]);
}

#[test]
fn multiline_values_and_attribute_layouts() {
    assert_eq!(ctx("<div\n  id=\"x\"\n  class=\"one\n         tw$0o\n         three\">"), some("tw", "two"));
    assert_eq!(ctx("<input disabled class=\"a$0\" value=\"b\">"), some("a", "a"));
    assert_eq!(ctx("<img src=x.png class=\"a$0\"/>"), some("a", "a"));
    assert_eq!(ctx("<P CLASS=\"a$0\">"), some("a", "a"));
    assert_eq!(ctx("<div class\n=\n\"a$0\">"), some("a", "a"));
}

#[test]
fn usages_in_multiline_value_have_correct_spans() {
    let src = "<div class=\"one\n   two\">";
    let u = markup::usages(src);
    assert_eq!(u.len(), 2);
    assert_eq!(&src[u[0].span.range()], "one");
    assert_eq!(&src[u[1].span.range()], "two");
}
