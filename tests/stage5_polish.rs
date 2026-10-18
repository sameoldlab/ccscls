//! Stage 5: workspace symbols and configuration that are not built yet.
//! Run with `cargo test --test stage5_polish -- --ignored`.

mod common;
use common::*;
use lsp_types::*;

fn site_with(init: Option<serde_json::Value>) -> (TempWorkspace, TestClient) {
    let ws = TempWorkspace::copy_fixture("site");
    let c = TestClient::start(ClientOptions {
        workspace: Some(ws.path().to_path_buf()),
        init_options: init,
        ..Default::default()
    });
    (ws, c)
}

#[test]
#[ignore = "stage 5: workspace symbols"]
fn workspace_symbol_searches_class_names() {
    let (ws, mut c) = site_with(None);
    let r = c.request::<request::WorkspaceSymbolRequest>(WorkspaceSymbolParams {
        query: "card".into(),
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    });
    let Some(WorkspaceSymbolResponse::Flat(mut syms)) = r else {
        panic!("expected flat symbols")
    };
    syms.sort_by(|a, b| a.name.cmp(&b.name));
    let names: Vec<_> = syms.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, [".card", ".card-title"]);
    assert!(syms.iter().all(|s| s.kind == SymbolKind::CLASS));
    assert_eq!(syms[0].location.uri, ws.uri("css/components.css"));
    c.shutdown();
}

#[test]
#[ignore = "stage 5: ignore globs"]
fn ignore_globs_from_initialization_options_are_honored() {
    let (_ws, mut c) = site_with(Some(
        serde_json::json!({ "ignore": ["css/components.css"] }),
    ));
    let u = uri("file:///virtual/t.html");
    let (text, pos) = parse_cursor("<p class=\"$0\">");
    c.open(&u, "html", &text);
    let Some(CompletionResponse::Array(items)) =
        c.request::<request::Completion>(CompletionParams {
            text_document_position: position_params(&u, pos),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: None,
        })
    else {
        panic!("no completion")
    };
    let l: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(!l.contains(&"card") && l.contains(&"btn"), "{l:?}");
    c.shutdown();
}

#[test]
#[ignore = "stage 5: declared edges"]
fn declared_edges_make_classes_verified_without_a_link() {
    // "css/components.css applies to every page under the workspace root."
    let (ws, mut c) = site_with(Some(serde_json::json!({
        "declaredEdges": [{ "from": "**/*.html", "to": "css/components.css" }]
    })));
    let index = ws.uri("index.html");
    let text = match c
        .hover(&index, ws.cursor_on("index.html", "card", 0, 1))
        .expect("hover")
        .contents
    {
        HoverContents::Markup(m) => m.value,
        other => panic!("unexpected {other:?}"),
    };
    assert!(
        text.contains("declared: `index.html` → `components.css`"),
        "{text}"
    );
    c.shutdown();
}

#[test]
#[ignore = "stage 5: config"]
fn extra_class_attributes_and_helper_functions_are_configurable() {
    let (ws, mut c) = site_with(Some(serde_json::json!({
        "classAttributes": ["data-cls"],
        "classFunctions": ["tw"],
    })));
    let h = uri("file:///virtual/t.html");
    let html = r#"<p data-cls="btn">"#;
    c.open(&h, "html", html);
    assert_eq!(
        c.definition_locations(&h, position_of(html, "btn", 1))
            .len(),
        ws.expected_locations("btn").len()
    );

    let j = uri("file:///virtual/t.jsx");
    let jsx = r#"const x = tw("card");"#;
    c.open(&j, "javascriptreact", jsx);
    assert_eq!(
        c.definition_locations(&j, position_of(jsx, "card", 1))
            .len(),
        1
    );
    c.shutdown();
}
