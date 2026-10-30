//! Hover on class names, by language.

mod common;
use common::*;
use lsp_types::*;


// ============================================================================
// css
// ============================================================================

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

// ============================================================================
// html
// ============================================================================

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

#[test]
fn hover_says_how_a_class_is_linked_or_that_it_is_not() {
    let (workspace, mut client) = site();
    let index = workspace.uri("index.html");

    let linked =
        hover_text(client.hover(&index, workspace.cursor_on("index.html", "btn-large", 0, 2)));
    assert_eq!(
        linked,
        "**`.btn-large`** — 1 rule\n\nlinked: `index.html` → `styles.css`\n\n`styles.css:4`\n\n```css\n.btn-large { padding: 8px 16px }\n```"
    );

    let unlinked =
        hover_text(client.hover(&index, workspace.cursor_on("index.html", "card", 0, 1)));
    assert_eq!(
        unlinked,
        "**`.card`** — 1 rule\n\nnot linked from here: defined in `components.css`\n\n`components.css:1`\n\n```css\n.card { border: 1px solid }\n```"
    );
    client.shutdown();
}

#[test]
fn hover_in_a_page_that_may_load_unknown_css_adds_the_caveat() {
    let (_workspace, mut client) = site();
    let fragment = uri("file:///virtual/fragment.html");
    client.open(&fragment, "html", r#"<p class="card">"#);
    let text = hover_text(client.hover(&fragment, Position::new(0, 11)));
    assert!(
        text.contains("not linked from here: defined in `components.css` (this file may load CSS we cannot see)"),
        "{text}"
    );
    client.shutdown();
}

#[test]
fn hover_follows_import_chains() {
    let (workspace, mut client) = scenario("chain");
    let index = workspace.uri("index.html");
    let text = hover_text(client.hover(&index, workspace.cursor_on("index.html", "btn", 0, 1)));
    assert!(
        text.contains("linked: `index.html` → `main.css` → `base.css`"),
        "{text}"
    );
    client.shutdown();
}

// ============================================================================
// svelte
// ============================================================================

#[test]
fn svelte_hover_on_a_usage_shows_the_rule() {
    let mut session =
        Session::start("//- /a.css\n.btn { color: red }\n//- /A.svelte\n<p class=\"b$0tn\">\n");
    let text = hover_text(session.hover());
    assert!(text.starts_with("**`.btn`** — 1 rule"), "{text}");
    assert!(text.contains("color: red"), "{text}");
    session.finish();
}

#[test]
fn svelte_hover_on_an_unknown_class_says_so() {
    let mut session = Session::start("//- /A.svelte\n<p class=\"mis$0sing\">\n");
    assert_eq!(
        hover_text(session.hover()),
        "**`.missing`** — no matching rule in the workspace CSS"
    );
    session.finish();
}

#[test]
fn svelte_hover_is_null_off_classes() {
    let mut session = Session::start("//- /a.css\n.btn { }\n//- /A.svelte\n<$0p class=\"btn\">\n");
    assert!(session.hover().is_none());
    session.finish();
}
