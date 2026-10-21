//! Stage C, Svelte: class usages in components resolve to the workspace CSS,
//! complete inside every place a class name can be written, and a component's
//! own `<style>` block is private to it.

mod common;
use common::*;
use lsp_types::*;
use serde_json::json;

fn site_with(options: Option<serde_json::Value>) -> (TempWorkspace, TestClient) {
    let workspace = TempWorkspace::copy_fixture("site");
    let client = TestClient::start(ClientOptions {
        workspace: Some(workspace.path().to_path_buf()),
        init_options: options,
        ..Default::default()
    });
    (workspace, client)
}

fn complete_items(client: &mut TestClient, uri: &Uri, marked: &str) -> Vec<CompletionItem> {
    let (text, position) = parse_cursor(marked);
    client.open(uri, "svelte", &text);
    let response = client.request::<request::Completion>(CompletionParams {
        text_document_position: position_params(uri, position),
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
        context: None,
    });
    match response {
        Some(CompletionResponse::Array(items)) => items,
        Some(CompletionResponse::List(list)) => list.items,
        None => Vec::new(),
    }
}

/// Go-to-definition from `chars_in` characters into `needle` must land on every
/// CSS rule of `class`.
fn assert_resolves(
    client: &mut TestClient,
    workspace: &TempWorkspace,
    doc: &Uri,
    text: &str,
    needle: &str,
    chars_in: usize,
    class: &str,
) {
    let mut got = client.definition_locations(doc, position_of(text, needle, chars_in));
    sort_locations(&mut got);
    assert_eq!(
        got,
        workspace.expected_locations(class),
        "`{needle}` should resolve to .{class}"
    );
}

const COMPONENT: &str = r#"<script>
  let active = false;
</script>

<div class="btn {active ? 'card' : ''} page">x</div>
<button class:active>shorthand</button>
<button class:title={active}>explicit</button>
<button class={['home', active && 'btn-large']}>svelte 5 array</button>
"#;

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

#[test]
fn directive_names_complete_and_replace_only_the_name() {
    let (_workspace, mut client) = site_with(None);
    let items = complete_items(
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
    let items = complete_items(&mut client, &uri("file:///virtual/E.svelte"), source);
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

#[test]
fn nothing_is_offered_outside_class_positions() {
    let (_workspace, mut client) = site_with(None);
    for (name, source) in [
        ("a", "<div id=\"ca$0\">"),
        ("b", "<div class:foo={ca$0}>"),
        ("c", "<script>let c = \"ca$0\";</script>"),
        ("d", "<p>{ca$0}</p>"),
    ] {
        let items = complete_items(
            &mut client,
            &uri(&format!("file:///virtual/{name}.svelte")),
            source,
        );
        assert!(items.is_empty(), "{source}: {items:?}");
    }
    client.shutdown();
}

#[test]
fn a_components_style_block_is_private_to_it() {
    let (_workspace, mut client) = site_with(None);
    let own = complete_items(
        &mut client,
        &uri("file:///virtual/A.svelte"),
        "<style>\n.mine { }\n</style>\n<p class=\"$0\">",
    );
    assert!(own.iter().any(|item| item.label == "mine"));
    let other = complete_items(
        &mut client,
        &uri("file:///virtual/B.svelte"),
        "<p class=\"$0\">",
    );
    assert!(
        !other.iter().any(|item| item.label == "mine"),
        "scoped class leaked into another component"
    );
    client.shutdown();
}

#[test]
fn definition_reaches_the_components_own_style_block() {
    let (_workspace, mut client) = site_with(None);
    let doc = uri("file:///virtual/Own.svelte");
    let source = "<p class=\"mine\"></p>\n<style>\n.mine { }\n</style>\n";
    client.open(&doc, "svelte", source);
    let found = client.definition_locations(&doc, position_of(source, "class=\"mine", 8));
    assert_eq!(
        found,
        vec![Location {
            uri: doc,
            range: Range::new(Position::new(2, 1), Position::new(2, 5))
        }]
    );
    client.shutdown();
}

#[test]
fn svelte_files_in_the_workspace_are_indexed_and_found_by_references() {
    let workspace = TempWorkspace::copy_fixture("site");
    workspace.write("src/App.svelte", "<div class=\"btn\">x</div>\n");
    let mut client = TestClient::start(ClientOptions {
        workspace: Some(workspace.path().to_path_buf()),
        ..Default::default()
    });
    let styles = workspace.uri("css/styles.css");
    let position = workspace.cursor_on("css/styles.css", "btn", 0, 1);
    let refs = client
        .references(&styles, position, false)
        .expect("references");

    let svelte = workspace.uri("src/App.svelte");
    let in_component: Vec<&Location> = refs
        .iter()
        .filter(|location| location.uri == svelte)
        .collect();
    assert_eq!(in_component.len(), 1);
    // `<div class="` is 12 characters, so `btn` is columns 12..15.
    assert_eq!(
        in_component[0].range,
        Range::new(Position::new(0, 12), Position::new(0, 15))
    );
    assert_eq!(refs.len(), workspace.expected_usages("btn").len() + 1);
    client.shutdown();
}

#[test]
fn components_stay_quiet_by_default_and_report_when_asked() {
    let source = "<p class=\"nothing-defines-this\"></p>\n";
    let doc = uri("file:///virtual/Quiet.svelte");

    let (_workspace, mut client) = site_with(None);
    client.open(&doc, "svelte", source);
    assert!(latest_diagnostics(&mut client, &doc).is_empty());
    client.shutdown();

    let (_workspace, mut client) = site_with(Some(json!({ "reportInOpenWorld": true })));
    client.open(&doc, "svelte", source);
    assert_eq!(
        diagnostic_summary(&latest_diagnostics(&mut client, &doc)),
        vec![(
            "nothing-defines-this".to_owned(),
            DiagnosticSeverity::WARNING
        )]
    );
    client.shutdown();
}
