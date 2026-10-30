//! Workspace scanning and file watching: what is indexed, and how disk changes
//! reach the index.

mod common;
use common::*;
use lsp_types::*;

// ============================================================================
// scanning
// ============================================================================

#[test]
fn node_modules_and_gitignored_directories_are_not_indexed() {
    let (_ws, mut c) = site();
    let css = uri("file:///virtual/q.css");
    c.open(&css, "css", ".vendor { }\n.generated { }");
    for (line, name) in [(0, "vendor"), (1, "generated")] {
        // If the on-disk copies were indexed there would be a second location,
        // and the references would include their html usages as well.
        let refs = c.references(&css, Position::new(line, 3), true).unwrap();
        assert_eq!(
            refs.len(),
            1,
            "`{name}` leaked from an ignored directory: {refs:?}"
        );
    }
    c.shutdown();
}

// ============================================================================
// watching (css, then svelte)
// ============================================================================

#[test]
fn watched_file_events_update_the_index() {
    let (ws, mut c) = site();
    let probe = uri("file:///virtual/probe.css");
    c.open(&probe, "css", ".fresh { }");
    let count = |c: &mut TestClient, name: &str| {
        c.change(&probe, 2, &format!(".{name} {{ }}"));
        c.definition_locations(&probe, Position::new(0, 2)).len()
    };
    let watch = |c: &TestClient, rel: &str, typ: FileChangeType| {
        c.notify::<notification::DidChangeWatchedFiles>(DidChangeWatchedFilesParams {
            changes: vec![FileEvent {
                uri: ws.uri(rel),
                typ,
            }],
        });
    };

    assert_eq!(count(&mut c, "fresh"), 1);
    ws.write("css/new.css", ".fresh { color: red }\n");
    watch(&c, "css/new.css", FileChangeType::CREATED);
    assert_eq!(count(&mut c, "fresh"), 2, "created file indexed");

    ws.write("css/new.css", ".fresher { color: red }\n");
    watch(&c, "css/new.css", FileChangeType::CHANGED);
    assert_eq!(count(&mut c, "fresh"), 1, "old name gone after change");
    assert_eq!(count(&mut c, "fresher"), 2, "probe + the changed file");
    std::fs::remove_file(ws.file("css/new.css")).unwrap();
    watch(&c, "css/new.css", FileChangeType::DELETED);
    assert_eq!(count(&mut c, "fresher"), 1, "deleted file removed");
    c.shutdown();
}

#[test]
fn watched_file_events_do_not_clobber_open_buffers() {
    let (ws, mut c) = site();
    let css = ws.uri("css/styles.css");
    c.open(&css, "css", ".only-in-buffer { }");
    ws.write("css/styles.css", ".on-disk { }");
    c.notify::<notification::DidChangeWatchedFiles>(DidChangeWatchedFilesParams {
        changes: vec![FileEvent {
            uri: css.clone(),
            typ: FileChangeType::CHANGED,
        }],
    });
    assert_eq!(c.definition_locations(&css, Position::new(0, 3)).len(), 1);
    // now the disk version wins
    c.close(&css);
    c.open(&uri("file:///virtual/p.css"), "css", ".on-disk { }");
    let refs = c
        .references(&uri("file:///virtual/p.css"), Position::new(0, 3), true)
        .unwrap();
    assert_eq!(refs.len(), 2, "virtual doc + the reloaded styles.css");
    c.shutdown();
}

#[test]
fn watcher_registration_follows_client_capability() {
    let ws = TempWorkspace::copy_fixture("site");
    let mut c = TestClient::start(ClientOptions {
        workspace: Some(ws.path().to_path_buf()),
        dynamic_watcher_registration: true,
        ..Default::default()
    });
    c.barrier();
    assert_eq!(c.server_requests.len(), 1, "{:?}", c.server_requests);
    let req = &c.server_requests[0];
    assert_eq!(req.method, "client/registerCapability");
    let reg = &req.params["registrations"][0];
    assert_eq!(reg["method"], "workspace/didChangeWatchedFiles");
    let globs: Vec<&str> = reg["registerOptions"]["watchers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["globPattern"].as_str().unwrap())
        .collect();
    assert_eq!(globs, ["**/*.css", "**/*.html", "**/*.htm", "**/*.svelte"]);
    c.shutdown();

    // default: no dynamic registration
    let (_ws, mut c) = site();
    c.barrier();
    assert!(c.server_requests.is_empty());
    c.shutdown();
}

#[test]
fn svelte_files_created_and_deleted_on_disk_are_followed() {
    let mut session = Session::start("//- /a.css\n.b$0tn { }\n");
    assert!(session.references(false).is_empty());

    session.workspace.write("B.svelte", "<p class=\"btn\">\n");
    let component = session.workspace.uri("B.svelte");
    session
        .client
        .notify::<notification::DidChangeWatchedFiles>(DidChangeWatchedFilesParams {
            changes: vec![FileEvent {
                uri: component.clone(),
                typ: FileChangeType::CREATED,
            }],
        });
    assert_eq!(file_names(&session.references(false)), ["B.svelte"]);

    std::fs::remove_file(session.workspace.file("B.svelte")).unwrap();
    session
        .client
        .notify::<notification::DidChangeWatchedFiles>(DidChangeWatchedFilesParams {
            changes: vec![FileEvent {
                uri: component,
                typ: FileChangeType::DELETED,
            }],
        });
    assert!(session.references(false).is_empty());
    session.finish();
}
