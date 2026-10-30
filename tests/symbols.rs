//! workspace/symbol over class names. Not built yet.

mod common;
use common::*;
use lsp_types::*;

// ============================================================================
// all languages
// ============================================================================

#[test]
#[ignore = "not built: workspace symbols"]
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
