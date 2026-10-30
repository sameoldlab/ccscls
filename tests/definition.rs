//! Go-to-definition from class usages (and from CSS itself), by language.

mod common;
use common::*;
use lsp_types::*;

// ============================================================================
// css
// ============================================================================

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

// ============================================================================
// html
// ============================================================================

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

#[test]
fn definition_and_hover_reach_the_pages_own_style_block() {
    let (_workspace, mut client) = site();
    let page = uri("file:///virtual/inline.html");
    client.open(&page, "html", INLINE_PAGE);

    let usage = position_of(INLINE_PAGE, "class=\"only", 7);
    let definition_start = position_of(INLINE_PAGE, ".only", 1);
    let definition_end = Position::new(definition_start.line, definition_start.character + 4);
    let locations = client.definition_locations(&page, usage);
    assert_eq!(
        locations,
        vec![Location {
            uri: page.clone(),
            range: Range::new(definition_start, definition_end)
        }]
    );

    let from_usage = hover_text(client.hover(&page, usage));
    assert!(from_usage.contains("defined in this file"), "{from_usage}");
    assert!(from_usage.contains("`inline.html:1`"), "{from_usage}");
    assert!(
        from_usage.contains("```css\n.only { }\n```"),
        "{from_usage}"
    );

    let from_definition = hover_text(client.hover(&page, definition_start));
    assert!(
        from_definition.contains("```css\n.only { }\n```"),
        "{from_definition}"
    );
    client.shutdown();
}

// ============================================================================
// svelte (syntax-specific cases are in svelte.rs)
// ============================================================================

#[test]
fn svelte_definition_is_null_for_undefined_classes_and_non_classes() {
    let mut undefined =
        Session::start("//- /a.css\n.btn { }\n//- /A.svelte\n<p class=\"mis$0sing\">\n");
    assert!(
        undefined
            .client
            .definition(&undefined.uri, undefined.position)
            .is_none()
    );
    undefined.finish();

    let mut tag = Session::start("//- /a.css\n.btn { }\n//- /A.svelte\n<$0p class=\"btn\">\n");
    assert!(tag.client.definition(&tag.uri, tag.position).is_none());
    tag.finish();
}

#[test]
fn definition_reaches_the_components_own_style_block() {
    let (_workspace, mut client) = site_with(None);
    let doc = uri("file:///virtual/Own.svelte");
    let source = "<p class=\"mine\"></p>\n<style>\n.mine { }\n</style>\n";
    client.open(&doc, "svelte", source);
    let found = client.definition_locations(&doc, position_of(source, "class=\"mine", 8));
    assert_eq!(
        found,
        vec![Location {
            uri: doc,
            range: Range::new(Position::new(2, 1), Position::new(2, 5))
        }]
    );
    client.shutdown();
}

// ============================================================================
// jsx and vue (not built)
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
#[ignore = "not built: jsx"]
fn jsx_class_name_attribute_and_helper_calls_resolve() {
    let (ws, mut c) = site();
    let u = uri("file:///virtual/App.jsx");
    c.open(&u, "javascriptreact", JSX);
    assert_resolves(&mut c, &ws, &u, JSX, "\"page\"", 1, "page");
    // className={'...'}
    assert_resolves(&mut c, &ws, &u, JSX, "'btn-large'", 1, "btn-large");
    // clsx("card", ...)
    assert_resolves(&mut c, &ws, &u, JSX, "\"card\"", 1, "card");
    assert_resolves(&mut c, &ws, &u, JSX, "\"title\"", 1, "title");
    // cn([...])
    assert_resolves(&mut c, &ws, &u, JSX, "\"home\"", 1, "home");
    // object key
    assert_resolves(&mut c, &ws, &u, JSX, "active: on", 1, "active");
    // static part of a template literal
    assert_resolves(&mut c, &ws, &u, JSX, "`btn ", 1, "btn");
    // `class` also works in JSX
    assert_resolves(&mut c, &ws, &u, JSX, "\"card-title\"", 1, "card-title");
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
#[ignore = "not built: vue"]
fn vue_static_and_bound_class_attributes_resolve() {
    let (ws, mut c) = site();
    let u = uri("file:///virtual/Comp.vue");
    c.open(&u, "vue", VUE);
    assert_resolves(&mut c, &ws, &u, VUE, "btn\" :class", 1, "btn");
    // unquoted object key
    assert_resolves(&mut c, &ws, &u, VUE, "active: isOn", 1, "active");
    // quoted object key
    assert_resolves(&mut c, &ws, &u, VUE, "'btn-large'", 1, "btn-large");
    assert_resolves(&mut c, &ws, &u, VUE, "'card'", 1, "card");
    assert_resolves(&mut c, &ws, &u, VUE, "'title'", 1, "title");
    // `isOn` / `big` / `page` (an identifier here) are JS expressions, not class names.
    assert!(c.definition(&u, position_of(VUE, "isOn,", 1)).is_none());
    assert!(c.definition(&u, position_of(VUE, "=\"page\"", 3)).is_none());
    c.shutdown();
}
