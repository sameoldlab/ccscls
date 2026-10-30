//! Settings from `initializationOptions` that are not built yet. (Diagnostic
//! severities are tested in `diagnostics.rs`.)

mod common;
use common::*;
use lsp_types::*;


// ============================================================================
// ignore globs and extra class positions (not built)
// ============================================================================

#[test]
#[ignore = "not built: ignore globs"]
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
#[ignore = "not built: class attributes"]
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
