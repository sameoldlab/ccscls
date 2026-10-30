//! Diagnostics for classes that are undefined or not linked, and their
//! configuration.

mod common;
use common::*;
use lsp_types::*;
use serde_json::json;

fn expected(entries: &[(&str, DiagnosticSeverity)]) -> Vec<(String, DiagnosticSeverity)> {
    entries
        .iter()
        .map(|(name, severity)| ((*name).to_owned(), *severity))
        .collect()
}

// ============================================================================
// html pages
// ============================================================================

#[test]
fn undefined_classes_warn_and_unlinked_ones_hint_by_default() {
    let (workspace, mut client) = site();
    let index = workspace.uri("index.html");
    client.open(&index, "html", &workspace.read("index.html"));
    let diagnostics = latest_diagnostics(&mut client, &index);

    assert_eq!(
        diagnostic_summary(&diagnostics),
        expected(&[
            ("card", DiagnosticSeverity::HINT),
            ("card-title", DiagnosticSeverity::HINT),
            ("home", DiagnosticSeverity::HINT),
            ("missing-class", DiagnosticSeverity::WARNING),
        ])
    );
    let missing = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("`missing-class`"))
        .expect("missing-class is reported");
    let missing_range = expected_ranges_in(&workspace.read("index.html"), Syntax::Html)
        .into_iter()
        .find(|(entry, _)| entry.name == "missing-class")
        .map(|(_, range)| range)
        .expect("annotated in the fixture");
    assert_eq!(missing.range, missing_range);
    assert_eq!(missing.source.as_deref(), Some("csscls"));
    assert_eq!(
        missing.code,
        Some(NumberOrString::String("undefined-class".to_owned()))
    );

    let home = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message.contains("`home`"))
        .expect("home is reported");
    assert!(home.message.contains("components.css"), "{}", home.message);
    assert_eq!(
        home.code,
        Some(NumberOrString::String("unlinked-class".to_owned()))
    );
    client.shutdown();
}

#[test]
fn severities_come_from_initialization_options() {
    let cases: Vec<(serde_json::Value, Vec<(&str, DiagnosticSeverity)>)> = vec![
        (
            json!({ "ambientSeverity": "off" }),
            vec![("missing-class", DiagnosticSeverity::WARNING)],
        ),
        (
            json!({ "ambientSeverity": "error", "undefinedSeverity": "information" }),
            vec![
                ("card", DiagnosticSeverity::ERROR),
                ("card-title", DiagnosticSeverity::ERROR),
                ("home", DiagnosticSeverity::ERROR),
                ("missing-class", DiagnosticSeverity::INFORMATION),
            ],
        ),
        (
            json!({ "ambientSeverity": "off", "undefinedSeverity": "off" }),
            vec![],
        ),
        // Unreadable options fall back to the defaults instead of failing.
        (
            json!({ "ambientSeverity": "loud" }),
            vec![
                ("card", DiagnosticSeverity::HINT),
                ("card-title", DiagnosticSeverity::HINT),
                ("home", DiagnosticSeverity::HINT),
                ("missing-class", DiagnosticSeverity::WARNING),
            ],
        ),
    ];
    for (options, wanted) in cases {
        let (workspace, mut client) = site_with(Some(options.clone()));
        let index = workspace.uri("index.html");
        client.open(&index, "html", &workspace.read("index.html"));
        let got = diagnostic_summary(&latest_diagnostics(&mut client, &index));
        assert_eq!(got, expected(&wanted), "options = {options}");
        client.shutdown();
    }
}

#[test]
fn files_with_unknown_css_sources_stay_quiet_unless_asked() {
    let fragment = r#"<p class="nothing-defines-this card">"#;
    let scripted = "<!doctype html><html><head><script src=\"x.js\"></script></head><body><p class=\"nothing-defines-this\"></body></html>";
    for (text, address) in [
        (fragment, "file:///virtual/fragment.html"),
        (scripted, "file:///virtual/scripted.html"),
    ] {
        let (_workspace, mut client) = site();
        let page = uri(address);
        client.open(&page, "html", text);
        assert!(
            latest_diagnostics(&mut client, &page).is_empty(),
            "{address} should be quiet"
        );
        client.shutdown();
    }

    let (_workspace, mut client) = site_with(Some(json!({ "reportInOpenWorld": true })));
    let page = uri("file:///virtual/fragment.html");
    client.open(&page, "html", fragment);
    assert_eq!(
        diagnostic_summary(&latest_diagnostics(&mut client, &page)),
        expected(&[
            ("card", DiagnosticSeverity::HINT),
            ("nothing-defines-this", DiagnosticSeverity::WARNING),
        ])
    );
    client.shutdown();
}

#[test]
fn editing_a_stylesheet_refreshes_the_diagnostics_of_pages_that_use_it() {
    let (workspace, mut client) = site();
    let index = workspace.uri("index.html");
    let styles = workspace.uri("css/styles.css");
    client.open(&index, "html", &workspace.read("index.html"));
    let before = diagnostic_summary(&latest_diagnostics(&mut client, &index));
    assert!(
        before.iter().any(|(name, _)| name == "missing-class"),
        "{before:?}"
    );

    // Define the missing class in a stylesheet the page links.
    let original = workspace.read("css/styles.css");
    client.open(&styles, "css", &format!("{original}.missing-class {{ }}\n"));
    let after = diagnostic_summary(&latest_diagnostics(&mut client, &index));
    assert!(
        after.iter().all(|(name, _)| name != "missing-class"),
        "{after:?}"
    );
    assert_eq!(
        after.len(),
        3,
        "only the unlinked-class hints remain: {after:?}"
    );

    // Take it away again.
    client.change(&styles, 2, &original);
    let restored = diagnostic_summary(&latest_diagnostics(&mut client, &index));
    assert!(
        restored.iter().any(|(name, _)| name == "missing-class"),
        "{restored:?}"
    );
    client.shutdown();
}

#[test]
fn closing_a_page_clears_its_diagnostics() {
    let (workspace, mut client) = site();
    let index = workspace.uri("index.html");
    client.open(&index, "html", &workspace.read("index.html"));
    assert!(!latest_diagnostics(&mut client, &index).is_empty());
    client.close(&index);
    assert!(latest_diagnostics(&mut client, &index).is_empty());
    client.shutdown();
}

#[test]
fn import_chains_make_classes_verified_for_diagnostics() {
    let (workspace, mut client) = scenario("chain");
    let index = workspace.uri("index.html");
    client.open(&index, "html", &workspace.read("index.html"));
    assert_eq!(
        diagnostic_summary(&latest_diagnostics(&mut client, &index)),
        expected(&[("ghost", DiagnosticSeverity::WARNING)])
    );
    client.shutdown();
}

#[test]
fn a_style_block_class_is_undefined_on_every_other_page() {
    let (_workspace, mut client) = site();
    let owner = uri("file:///virtual/owner.html");
    let other = uri("file:///virtual/other.html");
    client.open(&owner, "html", INLINE_PAGE);
    client.open(
        &other,
        "html",
        "<!doctype html><html><head></head><body><p class=\"only\"></p></body></html>",
    );
    assert!(latest_diagnostics(&mut client, &owner).is_empty());
    assert_eq!(
        diagnostic_summary(&latest_diagnostics(&mut client, &other)),
        expected(&[("only", DiagnosticSeverity::WARNING)])
    );
    client.shutdown();
}

// ============================================================================
// svelte components
// ============================================================================

#[test]
fn components_stay_quiet_by_default_and_report_when_asked() {
    let source = "<p class=\"nothing-defines-this\"></p>\n";
    let doc = uri("file:///virtual/Quiet.svelte");

    let (_workspace, mut client) = site_with(None);
    client.open(&doc, "svelte", source);
    assert!(latest_diagnostics(&mut client, &doc).is_empty());
    client.shutdown();

    let (_workspace, mut client) = site_with(Some(json!({ "reportInOpenWorld": true })));
    client.open(&doc, "svelte", source);
    assert_eq!(
        diagnostic_summary(&latest_diagnostics(&mut client, &doc)),
        vec![(
            "nothing-defines-this".to_owned(),
            DiagnosticSeverity::WARNING
        )]
    );
    client.shutdown();
}
