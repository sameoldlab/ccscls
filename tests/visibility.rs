//! Which CSS a file can see: links, imports, own style blocks and how the
//! import graph reacts to change. (The graph itself is tested on annotated
//! workspaces in `scenarios.rs`.)

mod common;
use common::*;
use lsp_types::*;

// ============================================================================
// html
// ============================================================================

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

// ============================================================================
// declared edges (not built)
// ============================================================================

#[test]
#[ignore = "not built: declared edges"]
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

// ============================================================================
// vue (not built)
// ============================================================================

#[test]
#[ignore = "not built: vue"]
fn vue_scoped_and_module_styles_are_component_local() {
    let (_ws, mut c) = site();
    let own = complete_labels(
        &mut c,
        &uri("file:///virtual/A.vue"),
        "<template><p class=\"$0\"/></template>\n<style scoped>\n.mine { }\n</style>",
        "vue",
    );
    assert!(own.contains(&"mine".to_owned()), "{own:?}");
    let other = complete_labels(
        &mut c,
        &uri("file:///virtual/B.vue"),
        "<template><p class=\"$0\"/></template>",
        "vue",
    );
    assert!(!other.contains(&"mine".to_owned()), "{other:?}");

    // CSS modules: `$style.name` resolves into the same file's <style module>.
    let module = uri("file:///virtual/M.vue");
    let source =
        "<template><p :class=\"$style.boxed\"/></template>\n<style module>\n.boxed { }\n</style>";
    c.open(&module, "vue", source);
    let found = c.definition_locations(&module, position_of(source, "boxed\"", 2));
    assert_eq!((found.len(), found[0].uri.clone()), (1, module));
    c.shutdown();
}
