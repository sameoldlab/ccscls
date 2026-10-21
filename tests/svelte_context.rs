//! Level 1: cursor context in Svelte class positions (drives completion),
//! using `$0` as the cursor marker.

mod common;
use common::parse_cursor;
use css_class_lsp::languages::Language;
use css_class_lsp::line_index::LineIndex;

/// (text before the cursor in its token, whole token) at `$0`, or None if the
/// cursor is not where a class name is written.
fn ctx(marked: &str) -> Option<(String, String)> {
    let (text, pos) = parse_cursor(marked);
    let offset = LineIndex::new(&text).offset(&text, pos);
    let c = Language::Svelte.class_context_at(&text, offset)?;
    Some((
        text[c.prefix.range()].to_owned(),
        text[c.replace.range()].to_owned(),
    ))
}

fn some(prefix: &str, token: &str) -> Option<(String, String)> {
    Some((prefix.to_owned(), token.to_owned()))
}

#[test]
fn static_values() {
    assert_eq!(ctx(r#"<div class="$0">"#), some("", ""));
    assert_eq!(ctx(r#"<div class="bt$0">"#), some("bt", "bt"));
    assert_eq!(ctx(r#"<div class="b$0tn">"#), some("b", "btn"));
    assert_eq!(ctx(r#"<div class="a b$0 c">"#), some("b", "b"));
    assert_eq!(ctx("<div class='x$0'>"), some("x", "x"));
    assert_eq!(ctx("<div class=x$0>"), some("x", "x"));
}

#[test]
fn mixed_static_text_and_expressions() {
    assert_eq!(ctx(r#"<div class="a {b} c$0">"#), some("c", "c"));
    assert_eq!(ctx(r#"<div class="a$0 {b} c">"#), some("a", "a"));
    assert_eq!(ctx(r#"<div class="a {x ? 'bc' : ''}$0 d">"#), some("", ""));
    assert_eq!(ctx(r#"<div class="a {x $0 ? 'b' : ''}">"#), None);
}

#[test]
fn string_literals_inside_expressions() {
    assert_eq!(ctx(r#"<div class="a {x ? 'b$0' : ''} c">"#), some("b", "b"));
    assert_eq!(ctx(r#"<div class={['a$0']}>"#), some("a", "a"));
    assert_eq!(ctx(r#"<div class={cond ? 'a' : 'b$0'}>"#), some("b", "b"));
    assert_eq!(ctx(r#"<div class={['one two$0']}>"#), some("two", "two"));
    assert_eq!(
        ctx(r#"<div class={{ 'is-b$0ig': big }}>"#),
        some("is-b", "is-big")
    );
}

#[test]
fn directive_names() {
    assert_eq!(ctx("<div class:ca$0={x}>"), some("ca", "ca"));
    assert_eq!(ctx("<div class:c$0a={x}>"), some("c", "ca"));
    assert_eq!(ctx("<div class:$0>"), some("", ""));
    assert_eq!(ctx("<div class:foo$0>"), some("foo", "foo"));
}

#[test]
fn everything_else_is_not_a_class_position() {
    assert_eq!(ctx(r#"<div id="a$0">"#), None);
    assert_eq!(ctx("<div class:foo={a$0}>"), None);
    assert_eq!(ctx(r#"<script>let c = "a$0";</script>"#), None);
    assert_eq!(ctx("<p>{a$0}</p>"), None);
    assert_eq!(ctx("<div on:click={() => 'x$0'}>"), None);
    assert_eq!(ctx(r#"<style>.a[class="b$0"] {}</style>"#), None);
    assert_eq!(ctx(r#"<!-- <div class="a$0"> -->"#), None);
    assert_eq!(ctx(r#"<p>class="a$0"</p>"#), None);
}

#[test]
fn half_typed_markup() {
    assert_eq!(ctx(r#"<div class="x $0"#), some("", ""));
    assert_eq!(ctx(r#"<div class="typing ba$0"#), some("ba", "ba"));
    assert_eq!(ctx("<div\n  class=\"one\n  tw$0o\">"), some("tw", "two"));
}
