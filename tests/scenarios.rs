//! Level 1: scenario workspaces with known verdicts.
//!
//! Each directory under `tests/fixtures/scenarios` is a small project. Every
//! class usage in its HTML files carries an annotation on the same line, and
//! every file says whether its CSS sources are fully known:
//!
//!   <p class="btn card ghost">  <!--v: btn=verified card=ambient ghost=undefined -->
//!   <!--world: closed -->       (or `open`)
//!
//! The expectations are written from what a browser would do, not copied from
//! the output. A usage with no annotation, or an annotation with no matching
//! usage, is a failure.

mod common;
use common::fixtures_dir;
use css_class_lsp::analysis::Role;
use css_class_lsp::languages::Language;
use css_class_lsp::uri::{file_key, path_to_uri};
use css_class_lsp::workspace::Workspace;
use std::path::{Path, PathBuf};

fn files_under(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            files_under(&path, found);
        } else {
            found.push(path);
        }
    }
}

#[derive(Debug, PartialEq)]
struct Marker {
    line: usize,
    name: String,
    verdict: String,
}

fn verdict_markers(source: &str) -> Vec<Marker> {
    let mut markers = Vec::new();
    for (line, text) in source.split('\n').enumerate() {
        let Some(start) = text.find("<!--v:") else {
            continue;
        };
        let rest = &text[start + "<!--v:".len()..];
        let Some(end) = rest.find("-->") else {
            continue;
        };
        for entry in rest[..end].split_whitespace() {
            if let Some((name, verdict)) = entry.rsplit_once('=') {
                markers.push(Marker {
                    line,
                    name: name.to_owned(),
                    verdict: verdict.to_owned(),
                });
            }
        }
    }
    markers
}

/// `Some(true)` for `<!--world: open -->`, `Some(false)` for `closed`.
fn world_marker(source: &str) -> Option<bool> {
    let start = source.find("<!--world:")? + "<!--world:".len();
    let end = source[start..].find("-->")? + start;
    match source[start..end].trim() {
        "open" => Some(true),
        "closed" => Some(false),
        _ => None,
    }
}

fn load(dir: &Path) -> (Workspace, Vec<PathBuf>) {
    let mut workspace = Workspace::new();
    workspace.set_roots(vec![dir.to_path_buf()]);
    let mut paths = Vec::new();
    files_under(dir, &mut paths);
    let mut html_files = Vec::new();
    for path in paths {
        let Some(language) = Language::detect(None, Some(&path)) else {
            continue;
        };
        let text = std::fs::read_to_string(&path).expect("readable fixture");
        let uri = path_to_uri(&path).expect("fixture path has a uri");
        workspace.upsert(uri, text, language, None);
        if language == Language::Html {
            html_files.push(path);
        }
    }
    (workspace, html_files)
}

fn check_scenario(dir: &Path) -> Vec<String> {
    let (workspace, html_files) = load(dir);
    let mut failures = Vec::new();
    for path in html_files {
        let label = path
            .strip_prefix(dir)
            .unwrap_or(&path)
            .display()
            .to_string();
        let source = std::fs::read_to_string(&path).expect("readable fixture");
        let markers = verdict_markers(&source);
        let uri = path_to_uri(&path).expect("fixture path has a uri");
        let Some(origin) = workspace.file_id(&file_key(&uri)) else {
            failures.push(format!("{label}: not indexed"));
            continue;
        };
        let Some(entry) = workspace.file(origin) else {
            continue;
        };

        match (world_marker(&source), workspace.reach(origin).open_world) {
            (Some(expected), actual) if expected != actual => failures.push(format!(
                "{label}: expected {} world but found {}",
                if expected { "open" } else { "closed" },
                if actual { "open" } else { "closed" }
            )),
            (None, _) => failures.push(format!("{label}: missing <!--world: ... --> marker")),
            _ => {}
        }

        let mut matched = vec![false; markers.len()];
        for occurrence in entry
            .analysis
            .occurrences
            .iter()
            .filter(|occurrence| occurrence.role == Role::Usage)
        {
            let name = workspace.name(occurrence.name);
            let line = source[..occurrence.name_span.start as usize]
                .matches('\n')
                .count();
            let got =
                format!("{:?}", workspace.classify(origin, occurrence.name).verdict).to_lowercase();
            match markers
                .iter()
                .position(|marker| marker.line == line && marker.name == name)
            {
                Some(index) => {
                    matched[index] = true;
                    if markers[index].verdict != got {
                        failures.push(format!(
                            "{label}:{}: `{name}` expected {} but was {got}",
                            line + 1,
                            markers[index].verdict
                        ));
                    }
                }
                None => failures.push(format!(
                    "{label}:{}: usage `{name}` has no <!--v: --> annotation",
                    line + 1
                )),
            }
        }
        for (marker, was_matched) in markers.iter().zip(matched) {
            if !was_matched {
                failures.push(format!(
                    "{label}:{}: annotated `{}` is not a usage in the file",
                    marker.line + 1,
                    marker.name
                ));
            }
        }
    }
    failures
}

#[test]
fn every_scenario_has_the_annotated_verdicts() {
    let root = fixtures_dir().join("scenarios");
    let mut scenarios: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("scenarios directory")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    scenarios.sort();
    assert!(
        scenarios.len() >= 6,
        "scenarios missing? found {}",
        scenarios.len()
    );

    let mut failures = Vec::new();
    for scenario in &scenarios {
        let name = scenario
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        failures.extend(
            check_scenario(scenario)
                .into_iter()
                .map(|failure| format!("[{name}] {failure}")),
        );
    }
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}

#[test]
fn verdict_markers_parse_names_with_colons_and_slashes() {
    let source = "<p class=\"sm:flex w-1/2\">  <!--v: sm:flex=verified w-1/2=ambient -->\n";
    assert_eq!(
        verdict_markers(source),
        vec![
            Marker {
                line: 0,
                name: "sm:flex".to_owned(),
                verdict: "verified".to_owned()
            },
            Marker {
                line: 0,
                name: "w-1/2".to_owned(),
                verdict: "ambient".to_owned()
            },
        ]
    );
    assert!(verdict_markers("<p>\n").is_empty());
}

#[test]
fn world_markers_parse() {
    assert_eq!(world_marker("<!--world: open -->"), Some(true));
    assert_eq!(world_marker("x\n<!--world:closed-->"), Some(false));
    assert_eq!(world_marker("<!--world: maybe -->"), None);
    assert_eq!(world_marker("nothing"), None);
}
