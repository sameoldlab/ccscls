//! Find-references, by language. A class is the same class in CSS, HTML and
//! components, so every language sees the others.

mod common;
use common::*;
use lsp_types::*;

// ============================================================================
// css
// ============================================================================

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

// ============================================================================
// html
// ============================================================================

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
fn references_to_an_undefined_class_are_its_usages() {
    let (ws, mut c) = site();
    let pos = ws.cursor_on("index.html", "missing-class", 0, 1);
    let mut got = c.references(&ws.uri("index.html"), pos, true).unwrap();
    sort_locations(&mut got);
    assert_eq!(got, ws.expected_usages("missing-class"));
    assert_eq!(got.len(), 1);
    c.shutdown();
}

// ============================================================================
// svelte
// ============================================================================

#[test]
fn svelte_files_in_the_workspace_are_indexed_and_found_by_references() {
    let workspace = TempWorkspace::copy_fixture("site");
    workspace.write("src/App.svelte", "<div class=\"btn\">x</div>\n");
    let mut client = TestClient::start(ClientOptions {
        workspace: Some(workspace.path().to_path_buf()),
        ..Default::default()
    });
    let styles = workspace.uri("css/styles.css");
    let position = workspace.cursor_on("css/styles.css", "btn", 0, 1);
    let refs = client
        .references(&styles, position, false)
        .expect("references");

    let svelte = workspace.uri("src/App.svelte");
    let in_component: Vec<&Location> = refs
        .iter()
        .filter(|location| location.uri == svelte)
        .collect();
    assert_eq!(in_component.len(), 1);
    // `<div class="` is 12 characters, so `btn` is columns 12..15.
    assert_eq!(
        in_component[0].range,
        Range::new(Position::new(0, 12), Position::new(0, 15))
    );
    assert_eq!(refs.len(), workspace.expected_usages("btn").len() + 1);
    client.shutdown();
}

#[test]
fn svelte_references_include_rules_and_usages_from_every_language() {
    let mut session = Session::start(
        "//- /a.css\n.btn { }\n//- /index.html\n<p class=\"btn\">\n//- /A.svelte\n<p class=\"b$0tn\">\n",
    );
    assert_eq!(
        file_names(&session.references(true)),
        ["A.svelte", "a.css", "index.html"]
    );
    assert_eq!(
        file_names(&session.references(false)),
        ["A.svelte", "index.html"]
    );
    session.finish();
}

#[test]
fn svelte_references_to_an_undefined_class_are_its_usages() {
    let mut session = Session::start(
        "//- /A.svelte\n<p class=\"mis$0sing\">\n//- /B.svelte\n<p class=\"missing\">\n",
    );
    assert_eq!(
        file_names(&session.references(true)),
        ["A.svelte", "B.svelte"]
    );
    session.finish();
}

// ============================================================================
// jsx (not built)
// ============================================================================

const JSX: &str = r#"
export const App = ({ big, on }) => (
  <div className="page">
    <button className={'btn-large'}>a</button>
    <button className={clsx("card", on && "title")}>b</button>
    <button className={cn(["home", { active: on }])}>c</button>
    <button className={`btn ${big ? "x" : ""}`}>d</button>
    <button class="card-title">e</button>
    <p id="btn-large" data-x="page">not classes</p>
  </div>
);
"#;

#[test]
#[ignore = "not built: jsx"]
fn jsx_usages_show_up_in_references_from_css() {
    let (ws, mut c) = site();
    let u = uri("file:///virtual/App.jsx");
    c.open(&u, "javascriptreact", JSX);
    let refs = c
        .references(
            &ws.uri("css/styles.css"),
            ws.cursor_on("css/styles.css", "page", 0, 1),
            true,
        )
        .unwrap();
    assert!(
        refs.iter().any(|l| l.uri == u),
        "jsx usage missing from {refs:?}"
    );
    c.shutdown();
}
