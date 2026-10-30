//! Level 1: the cursor context inside class positions (this drives completion),
//! using `$0` as the cursor marker. Tests that mirror each other across
//! languages are next to each other.

mod common;
use common::{Observer, parse_cursor};
use csscls::languages::Language;
use csscls::line_index::LineIndex;

fn some(prefix: &str, token: &str) -> Option<(String, String)> {
    Some((prefix.to_owned(), token.to_owned()))
}

/// (text before the cursor's token, whole token) at `$0`, or None if the cursor
/// is not in a class value.
fn html_ctx(marked: &str) -> Option<(String, String)> {
    let (text, pos) = parse_cursor(marked);
    let offset = LineIndex::new(&text).offset(&text, pos);
    let c = Language::Html.class_context_at(&text, offset)?;
    Some((
        text[c.prefix.range()].to_owned(),
        text[c.replace.range()].to_owned(),
    ))
}

/// (text before the cursor in its token, whole token) at `$0`, or None if the
/// cursor is not where a class name is written.
fn svelte_ctx(marked: &str) -> Option<(String, String)> {
    let (text, pos) = parse_cursor(marked);
    let offset = LineIndex::new(&text).offset(&text, pos);
    let c = Language::Svelte.class_context_at(&text, offset)?;
    Some((
        text[c.prefix.range()].to_owned(),
        text[c.replace.range()].to_owned(),
    ))
}

// ============================================================================
// inside and between tokens (html, svelte)
// ============================================================================

#[test]
fn html_inside_and_between_tokens() {
    assert_eq!(html_ctx(r#"<div class="$0">"#), some("", ""));
    assert_eq!(html_ctx(r#"<div class="bt$0">"#), some("bt", "bt"));
    // mid-token: replace whole token
    assert_eq!(html_ctx(r#"<div class="b$0tn">"#), some("b", "btn"));
    assert_eq!(html_ctx(r#"<div class="$0btn">"#), some("", "btn"));
    // right before the closing quote
    assert_eq!(html_ctx(r#"<div class="btn$0">"#), some("btn", "btn"));
    assert_eq!(html_ctx(r#"<div class="a $0 b">"#), some("", ""));
    assert_eq!(html_ctx(r#"<div class="a b$0 c">"#), some("b", "b"));
    assert_eq!(html_ctx("<div class='x$0'>"), some("x", "x"));
    assert_eq!(html_ctx("<div class=x$0>"), some("x", "x"));
}

#[test]
fn svelte_static_values() {
    assert_eq!(svelte_ctx(r#"<div class="$0">"#), some("", ""));
    assert_eq!(svelte_ctx(r#"<div class="bt$0">"#), some("bt", "bt"));
    assert_eq!(svelte_ctx(r#"<div class="b$0tn">"#), some("b", "btn"));
    assert_eq!(svelte_ctx(r#"<div class="a b$0 c">"#), some("b", "b"));
    assert_eq!(svelte_ctx("<div class='x$0'>"), some("x", "x"));
    assert_eq!(svelte_ctx("<div class=x$0>"), some("x", "x"));
}

// ============================================================================
// outside class positions (html, svelte)
// ============================================================================

#[test]
fn html_outside_class_values() {
    assert_eq!(html_ctx(r#"<div id="$0">"#), None);
    assert_eq!(html_ctx(r#"<div data-class="a$0">"#), None);
    assert_eq!(html_ctx(r#"<div class="a" $0>"#), None);
    assert_eq!(html_ctx(r#"<div class="a">te$0xt</div>"#), None);
    assert_eq!(html_ctx(r#"<div $0 class="a">"#), None);
    assert_eq!(html_ctx(r#"<!-- <div class="a$0"> -->"#), None);
    assert_eq!(
        html_ctx(r#"<script>x = '<div class="a$0">'</script>"#),
        None
    );
    assert_eq!(html_ctx(r#"<style>.a[class="b$0"] {}</style>"#), None);
    assert_eq!(html_ctx(r#"<p>class="a$0"</p>"#), None);
    // template syntax: opaque
    assert_eq!(html_ctx(r#"<div class="{{ x$0 }}">"#), None);
}

#[test]
fn svelte_everything_else_is_not_a_class_position() {
    assert_eq!(svelte_ctx(r#"<div id="a$0">"#), None);
    assert_eq!(svelte_ctx("<div class:foo={a$0}>"), None);
    assert_eq!(svelte_ctx(r#"<script>let c = "a$0";</script>"#), None);
    assert_eq!(svelte_ctx("<p>{a$0}</p>"), None);
    assert_eq!(svelte_ctx("<div on:click={() => 'x$0'}>"), None);
    assert_eq!(svelte_ctx(r#"<style>.a[class="b$0"] {}</style>"#), None);
    assert_eq!(svelte_ctx(r#"<!-- <div class="a$0"> -->"#), None);
    assert_eq!(svelte_ctx(r#"<p>class="a$0"</p>"#), None);
}

// ============================================================================
// half-typed text (html, svelte)
// ============================================================================

#[test]
fn html_unterminated_value_while_typing() {
    assert_eq!(html_ctx(r#"<p class="typing ba$0"#), some("ba", "ba"));
    assert_eq!(html_ctx(r#"<p class="$0"#), some("", ""));
    assert_eq!(html_ctx(r#"<p class="a b$0"#), some("b", "b"));
    let observed = Observer::new().observe(Language::Html, r#"<p class="typing ba"#);
    let names: Vec<_> = observed.iter().map(|item| item.name.as_str()).collect();
    assert_eq!(names, vec!["typing", "ba"]);
}

#[test]
fn svelte_half_typed_markup() {
    assert_eq!(svelte_ctx(r#"<div class="x $0"#), some("", ""));
    assert_eq!(svelte_ctx(r#"<div class="typing ba$0"#), some("ba", "ba"));
    assert_eq!(
        svelte_ctx("<div\n  class=\"one\n  tw$0o\">"),
        some("tw", "two")
    );
}

// ============================================================================
// html only
// ============================================================================

#[test]
fn html_class_values_after_other_constructs() {
    assert_eq!(html_ctx("<!-- c --><p class=\"a$0\">"), some("a", "a"));
    assert_eq!(
        html_ctx("<script>let a;</script><p class=\"a$0\">"),
        some("a", "a")
    );
    assert_eq!(
        html_ctx("<style>.x{}</style>\n<p class=\"a$0\">"),
        some("a", "a")
    );
    assert_eq!(
        html_ctx("<?xml v?>\n<!doctype html>\n<p class=\"a$0\">"),
        some("a", "a")
    );
    assert_eq!(html_ctx("</div><p class=\"a$0\">"), some("a", "a"));
}

#[test]
fn html_multiline_values_and_attribute_layouts() {
    assert_eq!(
        html_ctx("<div\n  id=\"x\"\n  class=\"one\n         tw$0o\n         three\">"),
        some("tw", "two")
    );
    assert_eq!(
        html_ctx("<input disabled class=\"a$0\" value=\"b\">"),
        some("a", "a")
    );
    assert_eq!(html_ctx("<img src=x.png class=\"a$0\"/>"), some("a", "a"));
    assert_eq!(html_ctx("<P CLASS=\"a$0\">"), some("a", "a"));
    assert_eq!(html_ctx("<div class\n=\n\"a$0\">"), some("a", "a"));
}

// ============================================================================
// svelte only: expressions and directives
// ============================================================================

#[test]
fn svelte_mixed_static_text_and_expressions() {
    assert_eq!(svelte_ctx(r#"<div class="a {b} c$0">"#), some("c", "c"));
    assert_eq!(svelte_ctx(r#"<div class="a$0 {b} c">"#), some("a", "a"));
    assert_eq!(
        svelte_ctx(r#"<div class="a {x ? 'bc' : ''}$0 d">"#),
        some("", "")
    );
    assert_eq!(svelte_ctx(r#"<div class="a {x $0 ? 'b' : ''}">"#), None);
}

#[test]
fn svelte_string_literals_inside_expressions() {
    assert_eq!(
        svelte_ctx(r#"<div class="a {x ? 'b$0' : ''} c">"#),
        some("b", "b")
    );
    assert_eq!(svelte_ctx(r#"<div class={['a$0']}>"#), some("a", "a"));
    assert_eq!(
        svelte_ctx(r#"<div class={cond ? 'a' : 'b$0'}>"#),
        some("b", "b")
    );
    assert_eq!(
        svelte_ctx(r#"<div class={['one two$0']}>"#),
        some("two", "two")
    );
    assert_eq!(
        svelte_ctx(r#"<div class={{ 'is-b$0ig': big }}>"#),
        some("is-b", "is-big")
    );
}

#[test]
fn svelte_directive_names() {
    assert_eq!(svelte_ctx("<div class:ca$0={x}>"), some("ca", "ca"));
    assert_eq!(svelte_ctx("<div class:c$0a={x}>"), some("c", "ca"));
    assert_eq!(svelte_ctx("<div class:$0>"), some("", ""));
    assert_eq!(svelte_ctx("<div class:foo$0>"), some("foo", "foo"));
}
