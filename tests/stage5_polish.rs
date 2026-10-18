//! Stage 5: diagnostics, workspace symbols, configuration. NOT IMPLEMENTED YET.
//! Run with `cargo test --test stage5_polish -- --ignored`.

mod common;
use common::*;
use lsp_types::*;
use std::time::Duration;

fn site_with(init: Option<serde_json::Value>) -> (TempWorkspace, TestClient) {
    let ws = TempWorkspace::copy_fixture("site");
    let c = TestClient::start(ClientOptions {
        workspace: Some(ws.path().to_path_buf()),
        init_options: init,
        ..Default::default()
    });
    (ws, c)
}

fn diagnostics_for(c: &mut TestClient, u: &Uri) -> Vec<Diagnostic> {
    let n = c
        .wait_notification("textDocument/publishDiagnostics", Duration::from_secs(2))
        .expect("no publishDiagnostics notification");
    let p: PublishDiagnosticsParams = serde_json::from_value(n.params).unwrap();
    assert_eq!(&p.uri, u);
    p.diagnostics
}

#[test]
#[ignore = "stage 5: diagnostics"]
fn unknown_classes_in_html_are_warned_about() {
    let (ws, mut c) = site_with(None);
    let u = ws.uri("index.html");
    c.open(&u, "html", &ws.read("index.html"));
    let d = diagnostics_for(&mut c, &u);
    // only `missing-class`
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].severity, Some(DiagnosticSeverity::WARNING));
    assert_eq!(d[0].source.as_deref(), Some("css-class-lsp"));
    assert!(d[0].message.contains("missing-class"));
    let (_, want) = expected_ranges_in(&ws.read("index.html"), Syntax::Html)
        .into_iter()
        .find(|(e, _)| e.name == "missing-class")
        .unwrap();
    assert_eq!(d[0].range, want);

    // Fixing the typo clears it.
    c.change(
        &u,
        2,
        &ws.read("index.html").replace("missing-class", "card"),
    );
    assert!(diagnostics_for(&mut c, &u).is_empty());
    c.shutdown();
}

#[test]
#[ignore = "stage 5: diagnostics"]
fn adding_the_missing_rule_clears_diagnostics_in_open_html() {
    let (ws, mut c) = site_with(None);
    let u = ws.uri("index.html");
    c.open(&u, "html", &ws.read("index.html"));
    assert_eq!(diagnostics_for(&mut c, &u).len(), 1);
    let css = ws.uri("css/styles.css");
    c.open(
        &css,
        "css",
        &(ws.read("css/styles.css") + ".missing-class { }\n"),
    );
    assert!(
        diagnostics_for(&mut c, &u).is_empty(),
        "html diagnostics must refresh when CSS changes"
    );
    c.shutdown();
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
#[ignore = "stage 5: config"]
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
