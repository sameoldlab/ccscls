//! Stage 1 (level 2): the core loop for a plain folder of HTML + CSS, driven
//! through real LSP messages against an in-process server.
//!
//! Expected locations come from the `/*~ */` and `<!--~ -->` annotations in
//! `tests/fixtures/site`, never from the server's own output.

mod common;
use common::*;
use lsp_types::*;

fn site() -> (TempWorkspace, TestClient) {
    let ws = TempWorkspace::copy_fixture("site");
    let c = TestClient::start(ClientOptions {
        workspace: Some(ws.path().to_path_buf()),
        ..Default::default()
    });
    (ws, c)
}

fn labels(r: Option<CompletionResponse>) -> Vec<String> {
    match r.expect("expected completion items") {
        CompletionResponse::Array(items) => items.into_iter().map(|i| i.label).collect(),
        CompletionResponse::List(l) => l.items.into_iter().map(|i| i.label).collect(),
    }
}

fn complete(c: &mut TestClient, u: &Uri, pos: Position) -> Option<CompletionResponse> {
    c.request::<request::Completion>(CompletionParams {
        text_document_position: position_params(u, pos),
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
        context: None,
    })
}

fn hover_text(h: Option<Hover>) -> String {
    match h.expect("expected hover").contents {
        HoverContents::Markup(m) => m.value,
        other => panic!("unexpected hover contents {other:?}"),
    }
}

// ---- handshake ------------------------------------------------------------

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
    assert_eq!(c.init.server_info.as_ref().unwrap().name, "css-class-lsp");
    c.shutdown();
}

// ---- completion -----------------------------------------------------------

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
fn completion_is_null_outside_class_values_and_in_css() {
    let (_ws, mut c) = site();
    let h = uri("file:///virtual/t.html");
    let (text, pos) = parse_cursor(r#"<div id="$0">"#);
    c.open(&h, "html", &text);
    assert!(complete(&mut c, &h, pos).is_none());

    let s = uri("file:///virtual/t.css");
    c.open(&s, "css", ".a { color: $0 }".replace("$0", "").as_str());
    assert!(complete(&mut c, &s, Position::new(0, 5)).is_none());
    c.shutdown();
}

// ---- definition -----------------------------------------------------------

#[test]
fn definition_from_html_usage_lists_every_css_rule() {
    let (ws, mut c) = site();
    let u = ws.uri("index.html");
    c.open(&u, "html", &ws.read("index.html"));
    let pos = ws.cursor_on("index.html", "btn", 0, 1);

    let mut got = c.definition_locations(&u, pos);
    sort_locations(&mut got);
    let want = ws.expected_locations("btn");
    assert_eq!(want.len(), 4, "fixture sanity: .btn is defined 4 times");
    assert_eq!(got, want);
    c.shutdown();
}

#[test]
fn definition_from_css_lists_every_css_rule() {
    let (ws, mut c) = site();
    let u = ws.uri("css/styles.css");
    c.open(&u, "css", &ws.read("css/styles.css"));
    let pos = ws.cursor_on("css/styles.css", "btn", 0, 1);
    let mut got = c.definition_locations(&u, pos);
    sort_locations(&mut got);
    assert_eq!(got, ws.expected_locations("btn"));
    c.shutdown();
}

#[test]
fn definition_works_in_a_file_that_was_never_opened() {
    // Indexed from disk by the workspace scan.
    let (ws, mut c) = site();
    let u = ws.uri("about.html");
    let pos = ws.cursor_on("about.html", "page", 0, 2);
    let mut got = c.definition_locations(&u, pos);
    sort_locations(&mut got);
    assert_eq!(got, ws.expected_locations("page"));
    c.shutdown();
}

#[test]
fn definition_is_null_for_undefined_classes_and_non_classes() {
    let (ws, mut c) = site();
    let u = ws.uri("index.html");
    c.open(&u, "html", &ws.read("index.html"));
    assert!(
        c.definition(&u, ws.cursor_on("index.html", "missing-class", 0, 1))
            .is_none()
    );
    // Cursor on the `<h1>` tag name.
    assert!(c.definition(&u, Position::new(6, 4)).is_none());
    c.shutdown();
}

// ---- references -----------------------------------------------------------

#[test]
fn references_include_css_rules_and_html_usages() {
    let (ws, mut c) = site();
    let u = ws.uri("index.html");
    c.open(&u, "html", &ws.read("index.html"));
    let pos = ws.cursor_on("index.html", "btn", 0, 1);

    let mut with = c.references(&u, pos, true).unwrap();
    sort_locations(&mut with);
    let want = ws.expected_references("btn");
    assert_eq!(want.len(), 4 + 3, "fixture sanity: 4 rules + 3 usages");
    assert_eq!(with, want);

    let mut without = c.references(&u, pos, false).unwrap();
    sort_locations(&mut without);
    assert_eq!(without, ws.expected_usages("btn"));
    c.shutdown();
}

#[test]
fn references_are_the_same_from_the_css_side() {
    let (ws, mut c) = site();
    let u = ws.uri("css/components.css");
    c.open(&u, "css", &ws.read("css/components.css"));
    let pos = ws.cursor_on("css/components.css", "home", 0, 1);
    let mut got = c.references(&u, pos, true).unwrap();
    sort_locations(&mut got);
    assert_eq!(got, ws.expected_references("home"));
    assert_eq!(got.len(), 2, "one rule + one usage");
    c.shutdown();
}

#[test]
fn references_to_an_undefined_class_are_its_usages() {
    let (ws, mut c) = site();
    let pos = ws.cursor_on("index.html", "missing-class", 0, 1);
    let mut got = c.references(&ws.uri("index.html"), pos, true).unwrap();
    sort_locations(&mut got);
    assert_eq!(got, ws.expected_usages("missing-class"));
    assert_eq!(got.len(), 1);
    c.shutdown();
}

// ---- hover ----------------------------------------------------------------

#[test]
fn hover_on_html_usage_shows_the_rule() {
    let (ws, mut c) = site();
    let u = ws.uri("index.html");
    let pos = ws.cursor_on("index.html", "btn-large", 0, 2);
    let h = c.hover(&u, pos).expect("hover");
    let want_range = expected_ranges_in(&ws.read("index.html"), Syntax::Html)
        .into_iter()
        .find(|(e, _)| e.name == "btn-large")
        .unwrap()
        .1;
    assert_eq!(h.range, Some(want_range));
    assert_eq!(
        hover_text(Some(h)),
        "**`.btn-large`** — 1 rule\n\nlinked: `index.html` → `styles.css`\n\n`styles.css:4`\n\n```css\n.btn-large { padding: 8px 16px }\n```"
    );
    c.shutdown();
}

#[test]
fn hover_lists_all_rules_with_their_at_rule_context() {
    let (ws, mut c) = site();
    let u = ws.uri("css/styles.css");
    let text = hover_text(c.hover(&u, ws.cursor_on("css/styles.css", "btn", 0, 1)));
    // Rules are listed by file path, then position.
    assert!(
        text.starts_with("**`.btn`** — 4 rules\n\n`components.css:4` · `@media (min-width: 600px)`\n\n```css\n.btn { padding: 12px }\n```"),
        "{text}"
    );
    for line in ["`styles.css:3`", "`styles.css:5`"] {
        assert!(text.contains(line), "missing {line} in {text}");
    }
    assert_eq!(text.matches("```css").count(), 4);
    c.shutdown();
}

#[test]
fn hover_on_an_unknown_class_says_so() {
    let (ws, mut c) = site();
    let text = hover_text(c.hover(
        &ws.uri("index.html"),
        ws.cursor_on("index.html", "missing-class", 0, 3),
    ));
    assert_eq!(
        text,
        "**`.missing-class`** — no matching rule in the workspace CSS"
    );
    c.shutdown();
}

#[test]
fn hover_is_null_off_classes() {
    let (ws, mut c) = site();
    assert!(
        c.hover(&ws.uri("index.html"), Position::new(0, 3))
            .is_none()
    );
    c.shutdown();
}

// ---- live buffers ---------------------------------------------------------

#[test]
fn unsaved_css_buffer_overrides_disk_until_closed() {
    let (ws, mut c) = site();
    let html = ws.uri("index.html");
    let css = ws.uri("css/styles.css");
    let pos = ws.cursor_on("index.html", "btn-large", 0, 1);

    // The editor has an unsaved edit that renames .btn-large.
    c.open(
        &css,
        "css",
        &ws.read("css/styles.css").replace(".btn-large", ".btn-xl"),
    );
    assert!(
        c.definition(&html, pos).is_none(),
        "disk version must be shadowed by the buffer"
    );
    assert!(hover_text(c.hover(&html, pos)).contains("no matching rule"));

    // Closing without saving reverts to disk.
    c.close(&css);
    let got = c.definition_locations(&html, pos);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].uri, css);
    c.shutdown();
}

#[test]
fn unsaved_html_buffer_adds_usages() {
    let (ws, mut c) = site();
    let about = ws.uri("about.html");
    let pos = ws.cursor_on("index.html", "btn", 0, 1);
    let before = c
        .references(&ws.uri("index.html"), pos, false)
        .unwrap()
        .len();
    assert_eq!(before, 3);

    c.open(
        &about,
        "html",
        &(ws.read("about.html") + "<p class=\"btn\">new</p>\n"),
    );
    assert_eq!(
        c.references(&ws.uri("index.html"), pos, false)
            .unwrap()
            .len(),
        4
    );
    c.close(&about);
    assert_eq!(
        c.references(&ws.uri("index.html"), pos, false)
            .unwrap()
            .len(),
        3
    );
    c.shutdown();
}

#[test]
fn edits_are_applied_in_order_before_later_requests() {
    let mut c = TestClient::start(Default::default());
    let u = uri("file:///virtual/o.css");
    c.open(&u, "css", ".one { }");
    for v in 2..=150 {
        c.change(&u, v, &format!(".n{v} {{ }}"));
    }
    // No waiting between the last edit and this request.
    let id = c.send::<request::HoverRequest>(hover_params(&u, Position::new(0, 2)));
    let text = hover_text(c.wait::<request::HoverRequest>(id));
    assert!(text.starts_with("**`.n150`**"), "{text}");
    c.shutdown();
}

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

// ---- positions ------------------------------------------------------------

#[test]
fn columns_are_utf16_on_both_sides() {
    let mut c = TestClient::start(Default::default());
    let css = uri("file:///virtual/u.css");
    let html = uri("file:///virtual/u.html");
    // 😀 is 2 UTF-16 units (4 bytes), é is 1 (2 bytes): byte columns would differ.
    c.open(&css, "css", "/* 😀é */ .zz {}");
    c.open(&html, "html", r#"<p title="😀é" class="zz">"#);

    let got = c.definition_locations(&html, Position::new(0, 23));
    assert_eq!(
        got,
        vec![Location {
            uri: css.clone(),
            range: Range::new(Position::new(0, 11), Position::new(0, 13))
        }]
    );

    let refs = c.references(&css, Position::new(0, 12), true).unwrap();
    assert_eq!(
        refs,
        vec![
            Location {
                uri: css,
                range: Range::new(Position::new(0, 11), Position::new(0, 13))
            },
            Location {
                uri: html,
                range: Range::new(Position::new(0, 22), Position::new(0, 24))
            },
        ]
    );
    c.shutdown();
}

// ---- workspace scanning and watching -------------------------------------

#[test]
fn node_modules_and_gitignored_directories_are_not_indexed() {
    let (_ws, mut c) = site();
    let css = uri("file:///virtual/q.css");
    c.open(&css, "css", ".vendor { }\n.generated { }");
    for (line, name) in [(0, "vendor"), (1, "generated")] {
        // If the on-disk copies were indexed there would be a second location,
        // and the references would include their html usages as well.
        let refs = c.references(&css, Position::new(line, 3), true).unwrap();
        assert_eq!(
            refs.len(),
            1,
            "`{name}` leaked from an ignored directory: {refs:?}"
        );
    }
    c.shutdown();
}

#[test]
fn watched_file_events_update_the_index() {
    let (ws, mut c) = site();
    let probe = uri("file:///virtual/probe.css");
    c.open(&probe, "css", ".fresh { }");
    let count = |c: &mut TestClient, name: &str| {
        c.change(&probe, 2, &format!(".{name} {{ }}"));
        c.definition_locations(&probe, Position::new(0, 2)).len()
    };
    let watch = |c: &TestClient, rel: &str, typ: FileChangeType| {
        c.notify::<notification::DidChangeWatchedFiles>(DidChangeWatchedFilesParams {
            changes: vec![FileEvent {
                uri: ws.uri(rel),
                typ,
            }],
        });
    };

    assert_eq!(count(&mut c, "fresh"), 1);
    ws.write("css/new.css", ".fresh { color: red }\n");
    watch(&c, "css/new.css", FileChangeType::CREATED);
    assert_eq!(count(&mut c, "fresh"), 2, "created file indexed");

    ws.write("css/new.css", ".fresher { color: red }\n");
    watch(&c, "css/new.css", FileChangeType::CHANGED);
    assert_eq!(count(&mut c, "fresh"), 1, "old name gone after change");
    assert_eq!(count(&mut c, "fresher"), 2, "probe + the changed file");
    std::fs::remove_file(ws.file("css/new.css")).unwrap();
    watch(&c, "css/new.css", FileChangeType::DELETED);
    assert_eq!(count(&mut c, "fresher"), 1, "deleted file removed");
    c.shutdown();
}

#[test]
fn watched_file_events_do_not_clobber_open_buffers() {
    let (ws, mut c) = site();
    let css = ws.uri("css/styles.css");
    c.open(&css, "css", ".only-in-buffer { }");
    ws.write("css/styles.css", ".on-disk { }");
    c.notify::<notification::DidChangeWatchedFiles>(DidChangeWatchedFilesParams {
        changes: vec![FileEvent {
            uri: css.clone(),
            typ: FileChangeType::CHANGED,
        }],
    });
    assert_eq!(c.definition_locations(&css, Position::new(0, 3)).len(), 1);
    // now the disk version wins
    c.close(&css);
    c.open(&uri("file:///virtual/p.css"), "css", ".on-disk { }");
    let refs = c
        .references(&uri("file:///virtual/p.css"), Position::new(0, 3), true)
        .unwrap();
    assert_eq!(refs.len(), 2, "virtual doc + the reloaded styles.css");
    c.shutdown();
}

#[test]
fn watcher_registration_follows_client_capability() {
    let ws = TempWorkspace::copy_fixture("site");
    let mut c = TestClient::start(ClientOptions {
        workspace: Some(ws.path().to_path_buf()),
        dynamic_watcher_registration: true,
        ..Default::default()
    });
    c.barrier();
    assert_eq!(c.server_requests.len(), 1, "{:?}", c.server_requests);
    let req = &c.server_requests[0];
    assert_eq!(req.method, "client/registerCapability");
    let reg = &req.params["registrations"][0];
    assert_eq!(reg["method"], "workspace/didChangeWatchedFiles");
    let globs: Vec<&str> = reg["registerOptions"]["watchers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["globPattern"].as_str().unwrap())
        .collect();
    assert_eq!(globs, ["**/*.css", "**/*.html", "**/*.htm"]);
    c.shutdown();

    // default: no dynamic registration
    let (_ws, mut c) = site();
    c.barrier();
    assert!(c.server_requests.is_empty());
    c.shutdown();
}

// ---- protocol behavior ----------------------------------------------------

#[test]
fn unsupported_languages_are_ignored() {
    let (_ws, mut c) = site();
    let u = uri("file:///virtual/x.js");
    c.open(&u, "javascript", r#"el.className = "btn";"#);
    assert!(c.definition(&u, Position::new(0, 18)).is_none());
    c.shutdown();
}

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
fn shutdown_sequence_exits_cleanly() {
    let (_ws, c) = site();
    // asserts shutdown -> exit -> thread returns Ok
    c.shutdown();
}
