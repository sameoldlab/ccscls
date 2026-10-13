//! Stage 4: which CSS applies to which file. NOT IMPLEMENTED YET.
//! Run with `cargo test --test stage4_scoping -- --ignored`.
//!
//! Until now every CSS file in the workspace is visible everywhere. Real
//! projects scope CSS: `<link>`/`@import` graphs, inline `<style>`, and
//! component-scoped styles.

mod common;
use common::*;
use lsp_types::*;

fn site() -> (TempWorkspace, TestClient) {
    let ws = TempWorkspace::copy_fixture("site");
    let c = TestClient::start(ClientOptions { workspace: Some(ws.path().to_path_buf()), ..Default::default() });
    (ws, c)
}

fn labels(c: &mut TestClient, u: &Uri, marked: &str, lang: &str) -> Vec<String> {
    let (text, pos) = parse_cursor(marked);
    c.open(u, lang, &text);
    match c.request::<request::Completion>(CompletionParams {
        text_document_position: position_params(u, pos),
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
        context: None,
    }) {
        Some(CompletionResponse::Array(items)) => items.into_iter().map(|i| i.label).collect(),
        Some(CompletionResponse::List(l)) => l.items.into_iter().map(|i| i.label).collect(),
        None => Vec::new(),
    }
}

#[test]
#[ignore = "stage 4: link tags"]
fn linked_stylesheets_limit_what_html_can_see() {
    let (ws, mut c) = site();
    // styles.css defines page/title/btn/btn-large/active; components.css defines card/card-title/home.
    let only_styles = labels(
        &mut c,
        &ws.uri("linked.html"),
        "<link rel=\"stylesheet\" href=\"css/styles.css\">\n<p class=\"$0\">",
        "html",
    );
    assert_eq!(only_styles, ["active", "btn", "btn-large", "page", "title"]);

    // No <link> at all: fall back to the whole workspace (stage 1 behavior).
    let all = labels(&mut c, &ws.uri("bare.html"), "<p class=\"$0\">", "html");
    assert!(all.contains(&"card".to_owned()) && all.contains(&"page".to_owned()), "{all:?}");
    c.shutdown();
}

#[test]
#[ignore = "stage 4: @import"]
fn import_chains_are_followed() {
    let (ws, mut c) = site();
    ws.write("css/main.css", "@import \"styles.css\";\n@import url(\"./components.css\");\n.main-only { }\n");
    // Exists in the workspace but is NOT reachable from main.css: must stay invisible.
    ws.write("css/unrelated.css", ".unrelated { }\n");
    c.notify::<notification::DidChangeWatchedFiles>(DidChangeWatchedFilesParams {
        changes: ["css/main.css", "css/unrelated.css"]
            .into_iter()
            .map(|rel| FileEvent { uri: ws.uri(rel), typ: FileChangeType::CREATED })
            .collect(),
    });
    let l = labels(
        &mut c,
        &ws.uri("imp.html"),
        "<link rel=\"stylesheet\" href=\"css/main.css\">\n<p class=\"$0\">",
        "html",
    );
    assert_eq!(
        l,
        ["active", "btn", "btn-large", "card", "card-title", "home", "main-only", "page", "title"]
    );
    c.shutdown();
}

#[test]
#[ignore = "stage 4: inline style"]
fn inline_style_blocks_define_classes_for_their_own_document() {
    let (_ws, mut c) = site();
    let u = uri("file:///virtual/inline.html");
    let src = "<style>\n.only-here { color: red }\n</style>\n<p class=\"only-here\">x</p>\n";
    c.open(&u, "html", src);
    let defs = c.definition_locations(&u, position_of(src, "only-here\">", 2));
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].uri, u);
    // inside the <style> block
    assert_eq!(defs[0].range.start, Position::new(1, 1));

    let other = labels(&mut c, &uri("file:///virtual/other.html"), "<p class=\"$0\">", "html");
    assert!(!other.contains(&"only-here".to_owned()), "inline style leaked to other documents: {other:?}");
    c.shutdown();
}

#[test]
#[ignore = "stage 4: svelte scoped style"]
fn svelte_style_blocks_are_component_scoped() {
    let (_ws, mut c) = site();
    let a = labels(
        &mut c,
        &uri("file:///virtual/A.svelte"),
        "<style>\n.mine { }\n</style>\n<p class=\"$0\">",
        "svelte",
    );
    assert!(a.contains(&"mine".to_owned()), "{a:?}");
    let b = labels(&mut c, &uri("file:///virtual/B.svelte"), "<p class=\"$0\">", "svelte");
    assert!(!b.contains(&"mine".to_owned()), "scoped class leaked into B: {b:?}");
    c.shutdown();
}

#[test]
#[ignore = "stage 4: vue scoped/module style"]
fn vue_scoped_and_module_styles_are_component_local() {
    let (_ws, mut c) = site();
    let a = labels(
        &mut c,
        &uri("file:///virtual/A.vue"),
        "<template><p class=\"$0\"/></template>\n<style scoped>\n.mine { }\n</style>",
        "vue",
    );
    assert!(a.contains(&"mine".to_owned()), "{a:?}");
    let b = labels(&mut c, &uri("file:///virtual/B.vue"), "<template><p class=\"$0\"/></template>", "vue");
    assert!(!b.contains(&"mine".to_owned()), "{b:?}");

    // CSS modules: `$style.name` resolves into the same file's <style module>.
    let u = uri("file:///virtual/M.vue");
    let src = "<template><p :class=\"$style.boxed\"/></template>\n<style module>\n.boxed { }\n</style>";
    c.open(&u, "vue", src);
    let defs = c.definition_locations(&u, position_of(src, "boxed\"", 2));
    assert_eq!((defs.len(), defs[0].uri.clone()), (1, u));
    c.shutdown();
}
