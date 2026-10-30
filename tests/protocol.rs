//! Protocol behavior that does not depend on a language: handshake, unknown
//! requests, unknown documents, shutdown.

mod common;
use common::*;
use lsp_types::*;


// ============================================================================
// handshake and shutdown
// ============================================================================

#[test]
fn initialize_advertises_capabilities() {
    let (_ws, c) = site();
    let caps = &c.init.capabilities;
    assert_eq!(
        caps.hover_provider,
        Some(HoverProviderCapability::Simple(true))
    );
    assert_eq!(caps.definition_provider, Some(OneOf::Left(true)));
    assert_eq!(caps.references_provider, Some(OneOf::Left(true)));
    assert!(caps.completion_provider.is_some());
    match &caps.text_document_sync {
        Some(TextDocumentSyncCapability::Options(o)) => {
            assert_eq!(o.open_close, Some(true));
            assert_eq!(o.change, Some(TextDocumentSyncKind::FULL));
        }
        other => panic!("unexpected sync capability {other:?}"),
    }
    assert_eq!(c.init.server_info.as_ref().unwrap().name, "csscls");
    c.shutdown();
}

#[test]
fn shutdown_sequence_exits_cleanly() {
    let (_ws, c) = site();
    // asserts shutdown -> exit -> thread returns Ok
    c.shutdown();
}

// ============================================================================
// bad input
// ============================================================================

#[test]
fn requests_for_unknown_documents_are_null_not_errors() {
    let (_ws, mut c) = site();
    let u = uri("file:///nowhere/x.html");
    assert!(c.hover(&u, Position::new(0, 0)).is_none());
    assert!(c.definition(&u, Position::new(0, 0)).is_none());
    assert!(c.references(&u, Position::new(0, 0), true).is_none());
    // Edits and closes for documents that were never opened are ignored.
    c.change(&u, 2, ".x {}");
    c.close(&u);
    assert!(c.hover(&u, Position::new(0, 0)).is_none());
    c.shutdown();
}

#[test]
fn unknown_methods_and_bad_params_get_error_responses() {
    let (_ws, mut c) = site();
    let err = c
        .raw_request("textDocument/somethingElse", serde_json::json!({}))
        .unwrap_err();
    assert_eq!(err.code, -32601);
    let err = c
        .raw_request("textDocument/hover", serde_json::json!({ "bogus": 1 }))
        .unwrap_err();
    assert_eq!(err.code, -32602);
    // ...and the server is still alive afterwards.
    c.barrier();
    c.shutdown();
}

#[test]
fn closing_an_unsaved_document_forgets_it() {
    let (_ws, mut c) = site();
    let u = uri("file:///virtual/gone.css");
    c.open(&u, "css", ".gone { }");
    assert!(c.hover(&u, Position::new(0, 2)).is_some());
    c.close(&u);
    assert!(c.hover(&u, Position::new(0, 2)).is_none());
    c.shutdown();
}

#[test]
fn unsupported_languages_are_ignored() {
    let (_ws, mut c) = site();
    let u = uri("file:///virtual/x.js");
    c.open(&u, "javascript", r#"el.className = "btn";"#);
    assert!(c.definition(&u, Position::new(0, 18)).is_none());
    c.shutdown();
}
