//! Live buffers: unsaved text overrides disk, edits apply in order, positions
//! are UTF-16. By language.

mod common;
use common::*;
use lsp_types::*;

// ============================================================================
// css
// ============================================================================

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

// ============================================================================
// html
// ============================================================================

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

// ============================================================================
// svelte
// ============================================================================

#[test]
fn svelte_unsaved_buffer_adds_usages() {
    let mut session =
        Session::start("//- /a.css\n.b$0tn { }\n//- /A.svelte\n<p>no classes yet</p>\n");
    assert!(session.references(false).is_empty());

    let component = session.workspace.uri("A.svelte");
    session
        .client
        .open(&component, "svelte", "<p class=\"btn\">new</p>\n");
    assert_eq!(file_names(&session.references(false)), ["A.svelte"]);

    session.client.close(&component);
    assert!(session.references(false).is_empty());
    session.finish();
}

#[test]
fn svelte_typing_a_class_is_reflected_immediately() {
    let mut session = Session::start("//- /a.css\n.card { }\n//- /A.svelte\n<p class=\"$0\">\n");
    session.client.change(&session.uri, 2, "<p class=\"ca\">");
    let items = complete_items(&mut session.client, &session.uri, Position::new(0, 12));
    assert!(items.iter().any(|item| item.label == "card"));
    session.finish();
}

// ============================================================================
// positions are UTF-16 (css and html, then svelte)
// ============================================================================

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

#[test]
fn svelte_columns_are_utf16() {
    // 😀 is 2 UTF-16 units and é is 1, so byte columns would differ.
    let mut session =
        Session::start("//- /a.css\n.z$0z { }\n//- /A.svelte\n<p title=\"😀é\" class=\"zz\">\n");
    let found = session.references(true);
    let range_in = |name: &str| {
        found
            .iter()
            .find(|location| location.uri.as_str().ends_with(name))
            .unwrap_or_else(|| panic!("no location in {name}: {found:?}"))
            .range
    };
    assert_eq!(
        range_in("/a.css"),
        Range::new(Position::new(0, 1), Position::new(0, 3))
    );
    assert_eq!(
        range_in("/A.svelte"),
        Range::new(Position::new(0, 22), Position::new(0, 24))
    );
    session.finish();
}
