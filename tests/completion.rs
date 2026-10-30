//! Completion inside class positions, by language.
//!
//! Tests that mirror each other across languages sit next to each other; what
//! is specific to one language is in `svelte.rs`.

mod common;
use common::*;
use lsp_types::*;

fn complete_svelte(client: &mut TestClient, uri: &Uri, marked: &str) -> Vec<CompletionItem> {
    complete_marked(client, uri, "svelte", marked)
}

// ============================================================================
// every workspace class is offered
// ============================================================================

#[test]
fn completion_offers_every_workspace_class_inside_class_values() {
    let (_ws, mut c) = site();
    let u = uri("file:///virtual/t.html");
    let (text, pos) = parse_cursor(r#"<div class="btn $0">"#);
    c.open(&u, "html", &text);
    let r = complete(&mut c, &u, pos);
    // Sorted; nothing from node_modules/ or the gitignored generated/.
    assert_eq!(
        labels(r),
        [
            "active",
            "btn",
            "btn-large",
            "card",
            "card-title",
            "home",
            "page",
            "title"
        ]
    );
    c.shutdown();
}

// ============================================================================
// items replace the whole token and show rule counts (html, svelte)
// ============================================================================

#[test]
fn completion_items_replace_the_whole_token_and_show_rule_counts() {
    let (_ws, mut c) = site();
    let u = uri("file:///virtual/t.html");
    let (text, pos) = parse_cursor(r#"<div class="b$0tn">"#);
    c.open(&u, "html", &text);
    let Some(CompletionResponse::Array(items)) = complete(&mut c, &u, pos) else {
        panic!("no items")
    };
    let btn = items.iter().find(|i| i.label == "btn").unwrap();
    assert_eq!(btn.kind, Some(CompletionItemKind::CLASS));
    assert_eq!(btn.detail.as_deref(), Some("4 rules"));
    // `btn` spans columns 12..15; the cursor is inside it.
    let Some(CompletionTextEdit::Edit(edit)) = &btn.text_edit else {
        panic!("no text edit")
    };
    assert_eq!(
        edit.range,
        Range::new(Position::new(0, 12), Position::new(0, 15))
    );
    assert_eq!(edit.new_text, "btn");
    let page = items.iter().find(|i| i.label == "page").unwrap();
    assert_eq!(page.detail.as_deref(), Some("1 rule"));
    c.shutdown();
}

#[test]
fn svelte_completion_items_replace_the_whole_token_and_show_rule_counts() {
    let mut session = Session::start(
        "//- /a.css\n.btn { }\n.btn { }\n.card { }\n//- /A.svelte\n<div class=\"b$0tn\">\n",
    );
    let items = complete_items(&mut session.client, &session.uri, session.position);
    let btn = items.iter().find(|item| item.label == "btn").unwrap();
    assert_eq!(btn.kind, Some(CompletionItemKind::CLASS));
    assert_eq!(btn.detail.as_deref(), Some("2 rules"));
    let Some(CompletionTextEdit::Edit(edit)) = &btn.text_edit else {
        panic!("no text edit")
    };
    assert_eq!(
        edit.range,
        Range::new(Position::new(0, 12), Position::new(0, 15))
    );
    let card = items.iter().find(|item| item.label == "card").unwrap();
    assert_eq!(card.detail.as_deref(), Some("1 rule"));
    session.finish();
}

// ============================================================================
// nothing is offered outside class positions (html, css, svelte)
// ============================================================================

#[test]
fn html_completion_is_null_outside_class_values() {
    let (_ws, mut c) = site();
    let h = uri("file:///virtual/t.html");
    let (text, pos) = parse_cursor(r#"<div id="$0">"#);
    c.open(&h, "html", &text);
    assert!(complete(&mut c, &h, pos).is_none());
    c.shutdown();
}

#[test]
fn css_completion_is_null() {
    let (_ws, mut c) = site();
    let s = uri("file:///virtual/t.css");
    c.open(&s, "css", ".a { color:  }");
    assert!(complete(&mut c, &s, Position::new(0, 5)).is_none());
    c.shutdown();
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
        let items = complete_svelte(
            &mut client,
            &uri(&format!("file:///virtual/{name}.svelte")),
            source,
        );
        assert!(items.is_empty(), "{source}: {items:?}");
    }
    client.shutdown();
}

// ============================================================================
// ranking by how a class is linked (html)
// ============================================================================

#[test]
fn completion_ranks_linked_classes_before_unlinked_ones() {
    let (workspace, mut client) = site();
    let index = workspace.uri("index.html");
    // index.html links styles.css but not components.css.
    let items = complete_items(
        &mut client,
        &index,
        workspace.cursor_on("index.html", "page", 0, 1),
    );

    let rows: Vec<(String, String, String)> = items
        .iter()
        .map(|item| {
            (
                item.label.clone(),
                item.detail.clone().unwrap_or_default(),
                item.label_details
                    .as_ref()
                    .and_then(|details| details.description.clone())
                    .unwrap_or_default(),
            )
        })
        .collect();
    let row = |label: &str, detail: &str, description: &str| {
        (label.to_owned(), detail.to_owned(), description.to_owned())
    };
    assert_eq!(
        rows,
        vec![
            row("active", "1 rule", "styles.css"),
            row("btn", "3 rules", "styles.css"),
            row("btn-large", "1 rule", "styles.css"),
            row("page", "1 rule", "styles.css"),
            row("title", "1 rule", "styles.css"),
            row("card", "1 rule", "not linked"),
            row("card-title", "1 rule", "not linked"),
            row("home", "1 rule", "not linked"),
        ]
    );
    assert_eq!(
        items.first().and_then(|item| item.sort_text.clone()),
        Some("0-active".to_owned())
    );
    assert_eq!(
        items.last().and_then(|item| item.sort_text.clone()),
        Some("1-home".to_owned())
    );
    client.shutdown();
}

#[test]
fn a_page_that_links_nothing_sees_everything_as_not_linked_but_still_gets_it() {
    let (_workspace, mut client) = site();
    let page = uri("file:///virtual/unlinked.html");
    let (text, position) = parse_cursor(r#"<div class="$0">"#);
    client.open(&page, "html", &text);
    let items = complete_items(&mut client, &page, position);
    assert_eq!(items.len(), 8, "nothing is hidden");
    let all_not_linked = items.iter().all(|item| {
        item.label_details
            .as_ref()
            .and_then(|details| details.description.as_deref())
            == Some("not linked")
    });
    assert!(all_not_linked);
    client.shutdown();
}

// ============================================================================
// a page's or component's own style block is private to it (html, svelte)
// ============================================================================

#[test]
fn a_pages_own_style_block_classes_are_offered_to_that_page_only() {
    let (_workspace, mut client) = site();
    let own = uri("file:///virtual/own.html");
    let (text, position) = parse_cursor(
        "<!doctype html><html><head><style>.mine { }</style></head><body><p class=\"$0\">",
    );
    client.open(&own, "html", &text);
    let mine = complete_items(&mut client, &own, position)
        .into_iter()
        .find(|item| item.label == "mine")
        .expect("the page's own class is offered");
    assert_eq!(mine.sort_text.as_deref(), Some("0-mine"));

    let other = uri("file:///virtual/other.html");
    let (text, position) = parse_cursor("<p class=\"$0\">");
    client.open(&other, "html", &text);
    let labels: Vec<String> = complete_items(&mut client, &other, position)
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(
        !labels.contains(&"mine".to_owned()),
        "private CSS leaked: {labels:?}"
    );
    client.shutdown();
}

#[test]
fn a_component_is_offered_the_classes_of_its_own_style_block() {
    let labels =
        completion_labels("//- /A.svelte\n<style>\n.mine { }\n</style>\n<p class=\"$0\">\n");
    assert!(labels.contains(&"mine".to_owned()), "{labels:?}");
}

#[test]
fn another_component_is_not_offered_them() {
    let labels = completion_labels(
        "//- /a.css\n.btn { }\n//- /A.svelte\n<style>\n.mine { }\n</style>\n//- /B.svelte\n<p class=\"$0\">\n",
    );
    // The control: completion works here, it just does not include `.mine`.
    assert!(labels.contains(&"btn".to_owned()), "{labels:?}");
    assert!(!labels.contains(&"mine".to_owned()), "{labels:?}");
}

// ============================================================================
// typing (html)
// ============================================================================

#[test]
fn typing_a_class_into_html_is_reflected_immediately() {
    let (_ws, mut c) = site();
    let u = uri("file:///virtual/t.html");
    c.open(&u, "html", r#"<p class="">"#);
    c.change(&u, 2, r#"<p class="ca">"#);
    let (_, pos) = parse_cursor(r#"<p class="ca$0">"#);
    let l = labels(complete(&mut c, &u, pos));
    assert!(l.contains(&"card".to_owned()));
    c.shutdown();
}

// ============================================================================
// jsx and vue (not built)
// ============================================================================

#[test]
#[ignore = "not built: jsx"]
fn jsx_completion_inside_class_name_strings_only() {
    let (_ws, mut c) = site();
    let l = complete_labels(
        &mut c,
        &uri("file:///virtual/A.tsx"),
        r#"const a = <p className="ca$0" />;"#,
        "typescriptreact",
    );
    assert!(l.contains(&"card".to_owned()), "{l:?}");
    let l = complete_labels(
        &mut c,
        &uri("file:///virtual/B.tsx"),
        r#"const a = <p title="ca$0" />;"#,
        "typescriptreact",
    );
    assert!(l.is_empty(), "title is not a class attribute: {l:?}");
    let l = complete_labels(
        &mut c,
        &uri("file:///virtual/C.tsx"),
        r#"const a = clsx("a", "ca$0");"#,
        "typescriptreact",
    );
    assert!(
        l.contains(&"card".to_owned()),
        "clsx args are class positions: {l:?}"
    );
    let l = complete_labels(
        &mut c,
        &uri("file:///virtual/D.tsx"),
        r#"const a = other("ca$0");"#,
        "typescriptreact",
    );
    assert!(
        l.is_empty(),
        "arbitrary calls are not class positions: {l:?}"
    );
    c.shutdown();
}
