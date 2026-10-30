//! Svelte syntax that has no HTML or CSS counterpart: directives, expressions,
//! object and array class values.

mod common;
use common::*;
use lsp_types::*;

fn complete_svelte(client: &mut TestClient, uri: &Uri, marked: &str) -> Vec<CompletionItem> {
    complete_marked(client, uri, "svelte", marked)
}

const COMPONENT: &str = r#"<script>
  let active = false;
</script>

<div class="btn {active ? 'card' : ''} page">x</div>
<button class:active>shorthand</button>
<button class:title={active}>explicit</button>
<button class={['home', active && 'btn-large']}>svelte 5 array</button>
"#;

// ============================================================================
// definition from directives and expressions
// ============================================================================

#[test]
fn class_attributes_directives_and_expressions_resolve() {
    let (workspace, mut client) = site_with(None);
    let doc = uri("file:///virtual/Widget.svelte");
    client.open(&doc, "svelte", COMPONENT);
    assert_resolves(
        &mut client,
        &workspace,
        &doc,
        COMPONENT,
        "btn {active",
        1,
        "btn",
    );
    assert_resolves(
        &mut client,
        &workspace,
        &doc,
        COMPONENT,
        "'card'",
        1,
        "card",
    );
    assert_resolves(
        &mut client,
        &workspace,
        &doc,
        COMPONENT,
        "page\">",
        1,
        "page",
    );
    assert_resolves(
        &mut client,
        &workspace,
        &doc,
        COMPONENT,
        ":active>",
        1,
        "active",
    );
    assert_resolves(
        &mut client,
        &workspace,
        &doc,
        COMPONENT,
        "title={active}",
        1,
        "title",
    );
    assert_resolves(
        &mut client,
        &workspace,
        &doc,
        COMPONENT,
        "'home'",
        1,
        "home",
    );
    assert_resolves(
        &mut client,
        &workspace,
        &doc,
        COMPONENT,
        "'btn-large'",
        1,
        "btn-large",
    );

    // Not class positions: a script variable, the `class` keyword, a variable in an expression.
    assert!(
        client
            .definition(&doc, position_of(COMPONENT, "let active", 6))
            .is_none()
    );
    assert!(
        client
            .definition(&doc, position_of(COMPONENT, "class:active", 2))
            .is_none()
    );
    assert!(
        client
            .definition(&doc, position_of(COMPONENT, "{active ?", 2))
            .is_none()
    );
    client.shutdown();
}

// ============================================================================
// completion in directives and expressions
// ============================================================================

#[test]
fn directive_names_complete_and_replace_only_the_name() {
    let (_workspace, mut client) = site_with(None);
    let items = complete_svelte(
        &mut client,
        &uri("file:///virtual/D.svelte"),
        "<div class:ca$0={x}>",
    );
    let card = items
        .iter()
        .find(|item| item.label == "card")
        .expect("card is offered");
    let Some(CompletionTextEdit::Edit(edit)) = &card.text_edit else {
        panic!("no text edit")
    };
    // `<div class:` is 11 characters; the name `ca` is columns 11..13.
    assert_eq!(
        edit.range,
        Range::new(Position::new(0, 11), Position::new(0, 13))
    );
    assert_eq!(edit.new_text, "card");
    client.shutdown();
}

#[test]
fn completion_works_inside_string_literals_in_expressions() {
    let (_workspace, mut client) = site_with(None);
    let source = "<div class=\"btn {on ? 'ca$0' : ''}\">";
    let (_, position) = parse_cursor(source);
    let items = complete_svelte(&mut client, &uri("file:///virtual/E.svelte"), source);
    let card = items
        .iter()
        .find(|item| item.label == "card")
        .expect("card is offered");
    let Some(CompletionTextEdit::Edit(edit)) = &card.text_edit else {
        panic!("no text edit")
    };
    // Only the two characters of the literal are replaced, not the whole attribute.
    assert_eq!(
        edit.range,
        Range::new(Position::new(0, position.character - 2), position)
    );
    client.shutdown();
}
