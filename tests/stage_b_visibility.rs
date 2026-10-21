//! Stage B (second half): what the editor sees once the import graph is in
//! play. Completion ranks linked classes first and labels the rest, hover says
//! how a class is connected, diagnostics separate undefined from unlinked
//! classes, and a page's own `<style>` block is visible to that page only.

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

fn site() -> (TempWorkspace, TestClient) {
    site_with(None)
}

fn scenario(name: &str) -> (TempWorkspace, TestClient) {
    let workspace = TempWorkspace::copy_fixture(&format!("scenarios/{name}"));
    let client = TestClient::start(ClientOptions {
        workspace: Some(workspace.path().to_path_buf()),
        ..Default::default()
    });
    (workspace, client)
}

fn complete_items(client: &mut TestClient, uri: &Uri, position: Position) -> Vec<CompletionItem> {
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

fn hover_text(hover: Option<Hover>) -> String {
    match hover.expect("expected hover").contents {
        HoverContents::Markup(markup) => markup.value,
        other => panic!("unexpected hover contents {other:?}"),
    }
}

fn expected(entries: &[(&str, DiagnosticSeverity)]) -> Vec<(String, DiagnosticSeverity)> {
    entries
        .iter()
        .map(|(name, severity)| ((*name).to_owned(), *severity))
        .collect()
}

// ---- completion -----------------------------------------------------------

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

// ---- hover ----------------------------------------------------------------

#[test]
fn hover_says_how_a_class_is_linked_or_that_it_is_not() {
    let (workspace, mut client) = site();
    let index = workspace.uri("index.html");

    let linked =
        hover_text(client.hover(&index, workspace.cursor_on("index.html", "btn-large", 0, 2)));
    assert_eq!(
        linked,
        "**`.btn-large`** — 1 rule\n\nlinked: `index.html` → `styles.css`\n\n`styles.css:4`\n\n```css\n.btn-large { padding: 8px 16px }\n```"
    );

    let unlinked =
        hover_text(client.hover(&index, workspace.cursor_on("index.html", "card", 0, 1)));
    assert_eq!(
        unlinked,
        "**`.card`** — 1 rule\n\nnot linked from here: defined in `components.css`\n\n`components.css:1`\n\n```css\n.card { border: 1px solid }\n```"
    );
    client.shutdown();
}

#[test]
fn hover_in_a_page_that_may_load_unknown_css_adds_the_caveat() {
    let (_workspace, mut client) = site();
    let fragment = uri("file:///virtual/fragment.html");
    client.open(&fragment, "html", r#"<p class="card">"#);
    let text = hover_text(client.hover(&fragment, Position::new(0, 11)));
    assert!(
        text.contains("not linked from here: defined in `components.css` (this file may load CSS we cannot see)"),
        "{text}"
    );
    client.shutdown();
}

#[test]
fn hover_follows_import_chains() {
    let (workspace, mut client) = scenario("chain");
    let index = workspace.uri("index.html");
    let text = hover_text(client.hover(&index, workspace.cursor_on("index.html", "btn", 0, 1)));
    assert!(
        text.contains("linked: `index.html` → `main.css` → `base.css`"),
        "{text}"
    );
    client.shutdown();
}

#[test]
fn a_link_to_a_stylesheet_created_later_starts_working_without_a_restart() {
    let (workspace, mut client) = scenario("broken_link");
    let index = workspace.uri("index.html");
    let position = workspace.cursor_on("index.html", "late", 0, 1);
    assert_eq!(
        hover_text(client.hover(&index, position)),
        "**`.late`** — no matching rule in the workspace CSS"
    );

    workspace.write("css/late.css", ".late { color: red }\n");
    client.notify::<notification::DidChangeWatchedFiles>(DidChangeWatchedFilesParams {
        changes: vec![FileEvent {
            uri: workspace.uri("css/late.css"),
            typ: FileChangeType::CREATED,
        }],
    });
    let text = hover_text(client.hover(&index, position));
    assert!(text.contains("linked: `index.html` → `late.css`"), "{text}");
    client.shutdown();
}

// ---- inline style blocks --------------------------------------------------

const INLINE_PAGE: &str = "<!doctype html><html><head><style>.only { }</style></head><body><p class=\"only\"></p></body></html>";

#[test]
fn definition_and_hover_reach_the_pages_own_style_block() {
    let (_workspace, mut client) = site();
    let page = uri("file:///virtual/inline.html");
    client.open(&page, "html", INLINE_PAGE);

    let usage = position_of(INLINE_PAGE, "class=\"only", 7);
    let definition_start = position_of(INLINE_PAGE, ".only", 1);
    let definition_end = Position::new(definition_start.line, definition_start.character + 4);
    let locations = client.definition_locations(&page, usage);
    assert_eq!(
        locations,
        vec![Location {
            uri: page.clone(),
            range: Range::new(definition_start, definition_end)
        }]
    );

    let from_usage = hover_text(client.hover(&page, usage));
    assert!(from_usage.contains("defined in this file"), "{from_usage}");
    assert!(from_usage.contains("`inline.html:1`"), "{from_usage}");
    assert!(
        from_usage.contains("```css\n.only { }\n```"),
        "{from_usage}"
    );

    let from_definition = hover_text(client.hover(&page, definition_start));
    assert!(
        from_definition.contains("```css\n.only { }\n```"),
        "{from_definition}"
    );
    client.shutdown();
}

// ---- diagnostics ----------------------------------------------------------

#[test]
fn undefined_classes_warn_and_unlinked_ones_hint_by_default() {
    let (workspace, mut client) = site();
    let index = workspace.uri("index.html");
    client.open(&index, "html", &workspace.read("index.html"));
    let diagnostics = latest_diagnostics(&mut client, &index);

    assert_eq!(
        diagnostic_summary(&diagnostics),
        expected(&[
            ("card", DiagnosticSeverity::HINT),
            ("card-title", DiagnosticSeverity::HINT),
            ("home", DiagnosticSeverity::HINT),
            ("missing-class", DiagnosticSeverity::WARNING),
        ])
    );
    let missing = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("`missing-class`"))
        .expect("missing-class is reported");
    let missing_range = expected_ranges_in(&workspace.read("index.html"), Syntax::Html)
        .into_iter()
        .find(|(entry, _)| entry.name == "missing-class")
        .map(|(_, range)| range)
        .expect("annotated in the fixture");
    assert_eq!(missing.range, missing_range);
    assert_eq!(missing.source.as_deref(), Some("css-class-lsp"));
    assert_eq!(
        missing.code,
        Some(NumberOrString::String("undefined-class".to_owned()))
    );

    let home = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("`home`"))
        .expect("home is reported");
    assert!(home.message.contains("components.css"), "{}", home.message);
    assert_eq!(
        home.code,
        Some(NumberOrString::String("unlinked-class".to_owned()))
    );
    client.shutdown();
}

#[test]
fn severities_come_from_initialization_options() {
    let cases: Vec<(serde_json::Value, Vec<(&str, DiagnosticSeverity)>)> = vec![
        (
            json!({ "ambientSeverity": "off" }),
            vec![("missing-class", DiagnosticSeverity::WARNING)],
        ),
        (
            json!({ "ambientSeverity": "error", "undefinedSeverity": "information" }),
            vec![
                ("card", DiagnosticSeverity::ERROR),
                ("card-title", DiagnosticSeverity::ERROR),
                ("home", DiagnosticSeverity::ERROR),
                ("missing-class", DiagnosticSeverity::INFORMATION),
            ],
        ),
        (
            json!({ "ambientSeverity": "off", "undefinedSeverity": "off" }),
            vec![],
        ),
        // Unreadable options fall back to the defaults instead of failing.
        (
            json!({ "ambientSeverity": "loud" }),
            vec![
                ("card", DiagnosticSeverity::HINT),
                ("card-title", DiagnosticSeverity::HINT),
                ("home", DiagnosticSeverity::HINT),
                ("missing-class", DiagnosticSeverity::WARNING),
            ],
        ),
    ];
    for (options, wanted) in cases {
        let (workspace, mut client) = site_with(Some(options.clone()));
        let index = workspace.uri("index.html");
        client.open(&index, "html", &workspace.read("index.html"));
        let got = diagnostic_summary(&latest_diagnostics(&mut client, &index));
        assert_eq!(got, expected(&wanted), "options = {options}");
        client.shutdown();
    }
}

#[test]
fn files_with_unknown_css_sources_stay_quiet_unless_asked() {
    let fragment = r#"<p class="nothing-defines-this card">"#;
    let scripted = "<!doctype html><html><head><script src=\"x.js\"></script></head><body><p class=\"nothing-defines-this\"></body></html>";
    for (text, address) in [
        (fragment, "file:///virtual/fragment.html"),
        (scripted, "file:///virtual/scripted.html"),
    ] {
        let (_workspace, mut client) = site();
        let page = uri(address);
        client.open(&page, "html", text);
        assert!(
            latest_diagnostics(&mut client, &page).is_empty(),
            "{address} should be quiet"
        );
        client.shutdown();
    }

    let (_workspace, mut client) = site_with(Some(json!({ "reportInOpenWorld": true })));
    let page = uri("file:///virtual/fragment.html");
    client.open(&page, "html", fragment);
    assert_eq!(
        diagnostic_summary(&latest_diagnostics(&mut client, &page)),
        expected(&[
            ("card", DiagnosticSeverity::HINT),
            ("nothing-defines-this", DiagnosticSeverity::WARNING),
        ])
    );
    client.shutdown();
}

#[test]
fn editing_a_stylesheet_refreshes_the_diagnostics_of_pages_that_use_it() {
    let (workspace, mut client) = site();
    let index = workspace.uri("index.html");
    let styles = workspace.uri("css/styles.css");
    client.open(&index, "html", &workspace.read("index.html"));
    let before = diagnostic_summary(&latest_diagnostics(&mut client, &index));
    assert!(
        before.iter().any(|(name, _)| name == "missing-class"),
        "{before:?}"
    );

    // Define the missing class in a stylesheet the page links.
    let original = workspace.read("css/styles.css");
    client.open(&styles, "css", &format!("{original}.missing-class {{ }}\n"));
    let after = diagnostic_summary(&latest_diagnostics(&mut client, &index));
    assert!(
        after.iter().all(|(name, _)| name != "missing-class"),
        "{after:?}"
    );
    assert_eq!(
        after.len(),
        3,
        "only the unlinked-class hints remain: {after:?}"
    );

    // Take it away again.
    client.change(&styles, 2, &original);
    let restored = diagnostic_summary(&latest_diagnostics(&mut client, &index));
    assert!(
        restored.iter().any(|(name, _)| name == "missing-class"),
        "{restored:?}"
    );
    client.shutdown();
}

#[test]
fn closing_a_page_clears_its_diagnostics() {
    let (workspace, mut client) = site();
    let index = workspace.uri("index.html");
    client.open(&index, "html", &workspace.read("index.html"));
    assert!(!latest_diagnostics(&mut client, &index).is_empty());
    client.close(&index);
    assert!(latest_diagnostics(&mut client, &index).is_empty());
    client.shutdown();
}

#[test]
fn import_chains_make_classes_verified_for_diagnostics() {
    let (workspace, mut client) = scenario("chain");
    let index = workspace.uri("index.html");
    client.open(&index, "html", &workspace.read("index.html"));
    assert_eq!(
        diagnostic_summary(&latest_diagnostics(&mut client, &index)),
        expected(&[("ghost", DiagnosticSeverity::WARNING)])
    );
    client.shutdown();
}

#[test]
fn a_style_block_class_is_undefined_on_every_other_page() {
    let (_workspace, mut client) = site();
    let owner = uri("file:///virtual/owner.html");
    let other = uri("file:///virtual/other.html");
    client.open(&owner, "html", INLINE_PAGE);
    client.open(
        &other,
        "html",
        "<!doctype html><html><head></head><body><p class=\"only\"></p></body></html>",
    );
    assert!(latest_diagnostics(&mut client, &owner).is_empty());
    assert_eq!(
        diagnostic_summary(&latest_diagnostics(&mut client, &other)),
        expected(&[("only", DiagnosticSeverity::WARNING)])
    );
    client.shutdown();
}
