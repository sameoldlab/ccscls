//! Rename across CSS and HTML. NOT IMPLEMENTED YET: these are the spec.
//! Run with `cargo test --test rename -- --ignored`.

mod common;
use common::*;
use csscls::languages::Language;
use lsp_types::*;

const TODO: &str = "not built: rename";

fn rename(
    c: &mut TestClient,
    u: &Uri,
    pos: Position,
    new: &str,
) -> Result<Option<WorkspaceEdit>, lsp_server::ResponseError> {
    c.try_request::<request::Rename>(rename_params(u, pos, new))
}

fn capabilities_say_rename(c: &TestClient) -> bool {
    matches!(
        c.init.capabilities.rename_provider,
        Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            ..
        }))
    )
}

#[test]
#[ignore = "stage 2: rename"]
fn rename_is_advertised_with_prepare_support() {
    let (_ws, c) = site();
    assert!(
        capabilities_say_rename(&c),
        "{TODO}: {:?}",
        c.init.capabilities.rename_provider
    );
    c.shutdown();
}

#[test]
#[ignore = "stage 2: rename"]
fn rename_from_css_edits_the_css_rule_and_the_html_usage() {
    let (ws, mut c) = site();
    let edit = rename(
        &mut c,
        &ws.uri("css/styles.css"),
        ws.cursor_on("css/styles.css", "btn-large", 0, 1),
        "btn-xl",
    )
    .unwrap()
    .expect("workspace edit");
    let by_uri = edits_by_uri(&edit);
    assert_eq!(
        by_uri.len(),
        2,
        "only styles.css and index.html mention btn-large: {:?}",
        by_uri.keys()
    );

    let css = apply_edits(
        &ws.read("css/styles.css"),
        &by_uri[ws.uri("css/styles.css").as_str()],
    );
    assert_eq!(
        css,
        ws.read("css/styles.css")
            .replacen(".btn-large {", ".btn-xl {", 1)
    );
    let html = apply_edits(
        &ws.read("index.html"),
        &by_uri[ws.uri("index.html").as_str()],
    );
    assert_eq!(
        html,
        ws.read("index.html")
            .replacen("btn btn-large", "btn btn-xl", 1)
    );
    c.shutdown();
}

#[test]
#[ignore = "stage 2: rename"]
fn rename_from_an_html_usage_gives_the_same_edit() {
    let (ws, mut c) = site();
    let from_css = rename(
        &mut c,
        &ws.uri("css/styles.css"),
        ws.cursor_on("css/styles.css", "btn-large", 0, 1),
        "btn-xl",
    )
    .unwrap();
    let from_html = rename(
        &mut c,
        &ws.uri("index.html"),
        ws.cursor_on("index.html", "btn-large", 0, 2),
        "btn-xl",
    )
    .unwrap();
    assert_eq!(
        edits_by_uri(&from_css.unwrap()),
        edits_by_uri(&from_html.unwrap())
    );
    c.shutdown();
}

#[test]
#[ignore = "stage 2: rename"]
fn renaming_a_repeated_class_touches_every_occurrence_and_nothing_else() {
    let (ws, mut c) = site();
    let edit = rename(
        &mut c,
        &ws.uri("index.html"),
        ws.cursor_on("index.html", "btn", 0, 1),
        "button",
    )
    .unwrap()
    .unwrap();
    let mut total = 0;
    for (uri, edits) in edits_by_uri(&edit) {
        total += edits.len();
        let path = csscls::uri::uri_to_path(&uri.parse::<Uri>().unwrap()).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let after = apply_edits(&before, &edits);
        if uri.ends_with(".css") {
            let defs = Observer::new().observe(Language::Css, &after);
            assert!(
                defs.iter().all(|d| d.name != "btn"),
                "{uri} still defines btn"
            );
            assert!(defs.iter().any(|d| d.name == "button"));
            assert!(
                defs.iter().any(|d| d.name == "btn-large") || !before.contains("btn-large"),
                "btn-large must be untouched"
            );
        } else {
            let usages = Observer::new().observe(Language::Html, &after);
            assert!(
                usages.iter().all(|u| u.name != "btn"),
                "{uri} still uses btn"
            );
        }
    }
    assert_eq!(total, 4 + 3, "4 rules + 3 usages");
    c.shutdown();
}

#[test]
#[ignore = "stage 2: rename"]
fn prepare_rename_returns_the_class_range_and_rejects_everything_else() {
    let (ws, mut c) = site();
    let u = ws.uri("index.html");
    let pos = ws.cursor_on("index.html", "btn", 0, 1);
    let r = c.request::<request::PrepareRenameRequest>(position_params(&u, pos));
    match r {
        Some(PrepareRenameResponse::RangeWithPlaceholder { placeholder, range }) => {
            assert_eq!(placeholder, "btn");
            assert_eq!(range.end.character - range.start.character, 3);
        }
        other => panic!("{TODO}: unexpected {other:?}"),
    }
    let r = c.request::<request::PrepareRenameRequest>(position_params(&u, Position::new(6, 4)));
    assert!(r.is_none(), "cursor on a tag name is not renameable");
    c.shutdown();
}

#[test]
#[ignore = "stage 2: rename"]
fn new_names_are_css_escaped_in_css_and_plain_in_html() {
    for (new_name, css_expected) in [
        ("md:flex", r".md\:flex { }"),
        ("10", r".\31 0 { }"),
        ("w-1/2", r".w-1\/2 { }"),
    ] {
        let mut c = TestClient::start(Default::default());
        let (css_u, html_u) = (uri("file:///virtual/e.css"), uri("file:///virtual/e.html"));
        c.open(&css_u, "css", r".sm\:flex { }");
        c.open(&html_u, "html", r#"<p class="a sm:flex b">"#);
        let edit = rename(&mut c, &css_u, Position::new(0, 3), new_name)
            .unwrap()
            .unwrap();
        let by = edits_by_uri(&edit);
        assert_eq!(
            apply_edits(r".sm\:flex { }", &by[css_u.as_str()]),
            css_expected,
            "css for {new_name}"
        );
        assert_eq!(
            apply_edits(r#"<p class="a sm:flex b">"#, &by[html_u.as_str()]),
            format!(r#"<p class="a {new_name} b">"#)
        );
        c.shutdown();
    }
}

#[test]
#[ignore = "stage 2: rename"]
fn hex_escape_terminator_space_is_preserved() {
    // `.\31 0{}`: the token is `\31 `+`0`; replacing it with a plain name must not glue to `{`.
    let mut c = TestClient::start(Default::default());
    let u = uri("file:///virtual/h.css");
    c.open(&u, "css", r".x\1F600 {}");
    let edit = rename(&mut c, &u, Position::new(0, 2), "y")
        .unwrap()
        .unwrap();
    assert_eq!(
        apply_edits(r".x\1F600 {}", &edits_by_uri(&edit)[u.as_str()]),
        ".y {}"
    );
    c.shutdown();
}

#[test]
#[ignore = "stage 2: rename"]
fn invalid_or_conflicting_new_names_are_rejected() {
    let (ws, mut c) = site();
    let u = ws.uri("css/styles.css");
    let pos = ws.cursor_on("css/styles.css", "btn", 0, 1);
    // A valid rename must work first, otherwise "error" below proves nothing.
    assert!(
        rename(&mut c, &u, pos, "button")
            .expect("valid rename")
            .is_some()
    );
    for bad in ["", "two words", "card", "has\nnewline"] {
        let r = rename(&mut c, &u, pos, bad);
        assert!(
            r.is_err(),
            "{TODO}: `{bad:?}` should be rejected, got {r:?}"
        );
    }
    c.shutdown();
}

#[test]
#[ignore = "stage 2: rename"]
fn rename_uses_unsaved_buffer_text() {
    let (ws, mut c) = site();
    let about = ws.uri("about.html");
    let buffer = ws.read("about.html") + "<p class=\"btn\">new</p>\n";
    c.open(&about, "html", &buffer);
    let edit = rename(
        &mut c,
        &about,
        ws.cursor_on("about.html", "btn", 0, 1),
        "button",
    )
    .unwrap()
    .unwrap();
    let after = apply_edits(&buffer, &edits_by_uri(&edit)[about.as_str()]);
    assert_eq!(after.matches("class=\"button\"").count(), 2, "{after}");
    c.shutdown();
}
