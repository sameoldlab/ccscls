//! Stage 3: class usages in JSX/TSX and Vue. NOT IMPLEMENTED YET.
//! (Svelte is implemented and tested in `stage_c_svelte.rs`.)
//! Run with `cargo test --test stage3_frameworks -- --ignored`.
//!
//! The CSS side is the `site` fixture; each test opens one virtual component
//! and checks that classes used in it resolve to the right CSS rules.

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

/// Go-to-definition from `needle` (cursor 1 char in) must land on every CSS rule of `class`.
fn assert_resolves(
    c: &mut TestClient,
    ws: &TempWorkspace,
    doc: &Uri,
    text: &str,
    needle: &str,
    class: &str,
) {
    let mut got = c.definition_locations(doc, position_of(text, needle, 1));
    sort_locations(&mut got);
    assert_eq!(
        got,
        ws.expected_locations(class),
        "`{needle}` should resolve to .{class}"
    );
}

fn complete_labels(c: &mut TestClient, u: &Uri, marked: &str, lang: &str) -> Vec<String> {
    let (text, pos) = parse_cursor(marked);
    c.open(u, lang, &text);
    let r = c.request::<request::Completion>(CompletionParams {
        text_document_position: position_params(u, pos),
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
        context: None,
    });
    match r {
        Some(CompletionResponse::Array(items)) => items.into_iter().map(|i| i.label).collect(),
        Some(CompletionResponse::List(l)) => l.items.into_iter().map(|i| i.label).collect(),
        None => Vec::new(),
    }
}

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
#[ignore = "stage 3: jsx"]
fn jsx_class_name_attribute_and_helper_calls_resolve() {
    let (ws, mut c) = site();
    let u = uri("file:///virtual/App.jsx");
    c.open(&u, "javascriptreact", JSX);
    assert_resolves(&mut c, &ws, &u, JSX, "\"page\"", "page");
    // className={'...'}
    assert_resolves(&mut c, &ws, &u, JSX, "'btn-large'", "btn-large");
    // clsx("card", ...)
    assert_resolves(&mut c, &ws, &u, JSX, "\"card\"", "card");
    assert_resolves(&mut c, &ws, &u, JSX, "\"title\"", "title");
    // cn([...])
    assert_resolves(&mut c, &ws, &u, JSX, "\"home\"", "home");
    // object key
    assert_resolves(&mut c, &ws, &u, JSX, "active: on", "active");
    // static part of a template literal
    assert_resolves(&mut c, &ws, &u, JSX, "`btn ", "btn");
    // `class` also works in JSX
    assert_resolves(&mut c, &ws, &u, JSX, "\"card-title\"", "card-title");
    // Not class positions:
    assert!(
        c.definition(&u, position_of(JSX, r#"id="btn-large"#, 6))
            .is_none()
    );
    assert!(
        c.definition(&u, position_of(JSX, r#"data-x="page"#, 9))
            .is_none()
    );
    c.shutdown();
}

#[test]
#[ignore = "stage 3: jsx"]
fn jsx_completion_inside_class_name_strings_only() {
    let (_ws, mut c) = site();
    let l = complete_labels(
        &mut c,
        &uri("file:///virtual/A.tsx"),
        r#"const a = <p className="ca$0" />;"#,
        "typescriptreact",
    );
    assert!(l.contains(&"card".to_owned()), "{l:?}");
    let l = complete_labels(
        &mut c,
        &uri("file:///virtual/B.tsx"),
        r#"const a = <p title="ca$0" />;"#,
        "typescriptreact",
    );
    assert!(l.is_empty(), "title is not a class attribute: {l:?}");
    let l = complete_labels(
        &mut c,
        &uri("file:///virtual/C.tsx"),
        r#"const a = clsx("a", "ca$0");"#,
        "typescriptreact",
    );
    assert!(
        l.contains(&"card".to_owned()),
        "clsx args are class positions: {l:?}"
    );
    let l = complete_labels(
        &mut c,
        &uri("file:///virtual/D.tsx"),
        r#"const a = other("ca$0");"#,
        "typescriptreact",
    );
    assert!(
        l.is_empty(),
        "arbitrary calls are not class positions: {l:?}"
    );
    c.shutdown();
}

#[test]
#[ignore = "stage 3: jsx"]
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

const VUE: &str = r#"<template>
  <div class="btn" :class="{ active: isOn, 'btn-large': big }">
    <p :class="['card', isOn && 'title']">x</p>
    <p v-bind:class="page">dynamic expression, no literal</p>
  </div>
</template>
<script setup>
const isOn = true;
</script>
"#;

#[test]
#[ignore = "stage 3: vue"]
fn vue_static_and_bound_class_attributes_resolve() {
    let (ws, mut c) = site();
    let u = uri("file:///virtual/Comp.vue");
    c.open(&u, "vue", VUE);
    assert_resolves(&mut c, &ws, &u, VUE, "btn\" :class", "btn");
    // unquoted object key
    assert_resolves(&mut c, &ws, &u, VUE, "active: isOn", "active");
    // quoted object key
    assert_resolves(&mut c, &ws, &u, VUE, "'btn-large'", "btn-large");
    assert_resolves(&mut c, &ws, &u, VUE, "'card'", "card");
    assert_resolves(&mut c, &ws, &u, VUE, "'title'", "title");
    // `isOn` / `big` / `page` (an identifier here) are JS expressions, not class names.
    assert!(c.definition(&u, position_of(VUE, "isOn,", 1)).is_none());
    assert!(c.definition(&u, position_of(VUE, "=\"page\"", 3)).is_none());
    c.shutdown();
}

#[test]
#[ignore = "stage 3: vue scoped/module style"]
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
