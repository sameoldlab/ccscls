//! Shared test tooling: fixture annotations, cursor markers, and an in-process
//! LSP client. Included by each integration test via `mod common;`.
#![allow(dead_code)]

use csscls::analysis::{FileAnalysis, Role, Span};
use csscls::languages::{Engines, Language};
use csscls::strings::Strings;
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response, ResponseError};
use lsp_types::{
    ClientCapabilities, DidChangeWatchedFilesClientCapabilities, InitializeParams,
    InitializeResult, Location, Position, Range, Uri, WorkspaceClientCapabilities, WorkspaceFolder,
};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::thread::JoinHandle;
use std::time::Duration;

pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

// ---------------------------------------------------------------------------
// Fixture annotations
//
//   .btn, .card { }   /*~ btn, card */
//   .sm\:flex { }     /*~ sm\:flex=>sm:flex */
//   .x { }            /*~ x */ /*ctx: @layer base | @media (min-width: 1px) */
//
// `/*~ ... */` lists, in source order, the class tokens that START on that
// line: `raw` is the identifier exactly as written, optionally followed by
// `=>name` when the unescaped name differs. `/*ctx: ... */` lists the
// enclosing at-rule preludes (outermost first, separated by `|`) that apply to
// every class on the line. A line with no `/*~` marker must contain no classes.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expected {
    /// 0-based line.
    pub line: u32,
    pub raw: String,
    pub name: String,
    pub ctx: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    /// `.btn { }  /*~ btn */`: class definitions in selectors.
    Css,
    /// `<p class="btn">  <!--~ btn -->`: class usages in attributes.
    Html,
}

impl Syntax {
    fn markers(self) -> (&'static str, &'static str) {
        match self {
            Syntax::Css => ("/*~", "*/"),
            Syntax::Html => ("<!--~", "-->"),
        }
    }
}

pub fn parse_expectations(src: &str) -> Vec<Expected> {
    parse_expectations_in(src, Syntax::Css)
}

pub fn parse_expectations_in(src: &str, syntax: Syntax) -> Vec<Expected> {
    let (open, close) = syntax.markers();
    let mut out = Vec::new();
    for (i, line) in src.split('\n').enumerate() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let Some(classes) = marker(line, open, close) else {
            continue;
        };
        let ctx: Vec<String> = marker(line, "/*ctx:", "*/")
            .map(|c| c.split('|').map(|s| s.trim().to_owned()).collect())
            .unwrap_or_default();
        for entry in classes.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            let (raw, name) = entry.split_once("=>").unwrap_or((entry, entry));
            out.push(Expected {
                line: i as u32,
                raw: raw.to_owned(),
                name: name.to_owned(),
                ctx: ctx.clone(),
            });
        }
    }
    out
}

fn marker<'a>(line: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = line.find(open)? + open.len();
    let end = line[start..].find(close)? + start;
    Some(line[start..end].trim())
}

/// One occurrence as a language handler reported it, with names resolved to text.
#[derive(Debug, Clone, PartialEq)]
pub struct Observed {
    pub role: Role,
    pub name: String,
    pub name_span: Span,
    pub rule_span: Span,
    pub context: Vec<String>,
}

/// Runs language handlers on text without a workspace, for level-1 tests.
pub struct Observer {
    engines: Engines,
    strings: Strings,
}

impl Observer {
    pub fn new() -> Self {
        Self {
            engines: Engines::new(),
            strings: Strings::new(),
        }
    }

    pub fn observe(&mut self, language: Language, source: &str) -> Vec<Observed> {
        let mut analysis = FileAnalysis::default();
        language.analyze(&mut self.engines, source, &mut self.strings, &mut analysis);
        analysis
            .occurrences
            .iter()
            .map(|occurrence| Observed {
                role: occurrence.role,
                name: self.strings.get(occurrence.name).to_owned(),
                name_span: occurrence.name_span,
                rule_span: occurrence.rule_span,
                context: analysis
                    .context_of(occurrence)
                    .iter()
                    .map(|id| self.strings.get(*id).to_owned())
                    .collect(),
            })
            .collect()
    }
}

/// What a handler actually found, in the same shape as the annotations.
pub fn describe(src: &str, observed: &[Observed]) -> Vec<Expected> {
    observed
        .iter()
        .map(|item| Expected {
            line: src[..item.name_span.start as usize].matches('\n').count() as u32,
            raw: src[item.name_span.range()].to_owned(),
            name: item.name.clone(),
            ctx: item.context.clone(),
        })
        .collect()
}

pub fn expected_ranges(src: &str) -> Vec<(Expected, Range)> {
    expected_ranges_in(src, Syntax::Css)
}

/// LSP ranges for each annotated class, computed independently of the server
/// by scanning the annotated line for the token (not adjacent to more
/// identifier characters) and counting UTF-16 units.
pub fn expected_ranges_in(src: &str, syntax: Syntax) -> Vec<(Expected, Range)> {
    let lines: Vec<&str> = src
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    let mut out = Vec::new();
    // (line, byte offset consumed so far)
    let mut cursor = (u32::MAX, 0usize);
    for e in parse_expectations_in(src, syntax) {
        let line = lines[e.line as usize];
        if cursor.0 != e.line {
            cursor = (e.line, 0);
        }
        let (lead, needle) = match syntax {
            Syntax::Css => (1, format!(".{}", e.raw)),
            Syntax::Html => (0, e.raw.clone()),
        };
        let mut from = cursor.1;
        let idx = loop {
            let found = line[from..]
                .find(&needle)
                .unwrap_or_else(|| panic!("`{needle}` not found on line {}: {line:?}", e.line))
                + from;
            let after = found + needle.len();
            let before_ok = syntax == Syntax::Css
                || !line[..found].chars().next_back().is_some_and(is_ident_char);
            if before_ok && !line[after..].chars().next().is_some_and(is_ident_char) {
                break found;
            }
            from = found + 1;
        };
        let name_start = idx + lead;
        let name_end = name_start + e.raw.len();
        cursor.1 = name_end;
        let col = |byte: usize| line[..byte].encode_utf16().count() as u32;
        let range = Range::new(
            Position::new(e.line, col(name_start)),
            Position::new(e.line, col(name_end)),
        );
        out.push((e, range));
    }
    out
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | '\\') || !c.is_ascii()
}

/// `$0` marks the cursor. Returns the text without the marker and the
/// cursor's LSP position (UTF-16 column).
pub fn parse_cursor(text: &str) -> (String, Position) {
    let idx = text.find("$0").expect("no $0 cursor marker");
    assert_eq!(text.matches("$0").count(), 1, "multiple $0 markers");
    let clean = format!("{}{}", &text[..idx], &text[idx + 2..]);
    let line = clean[..idx].matches('\n').count() as u32;
    let line_start = clean[..idx].rfind('\n').map_or(0, |i| i + 1);
    let character = clean[line_start..idx].encode_utf16().count() as u32;
    (clean, Position::new(line, character))
}

// ---------------------------------------------------------------------------
// Workspace fixtures
// ---------------------------------------------------------------------------

/// A fixture directory copied into a temp dir (so the server can read it like a
/// real project and tests can edit it).
pub struct TempWorkspace {
    dir: tempfile::TempDir,
}

impl TempWorkspace {
    pub fn empty() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }
    pub fn copy_fixture(name: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        copy_dir(&fixtures_dir().join(name), dir.path());
        Self { dir }
    }
    pub fn path(&self) -> &Path {
        self.dir.path()
    }
    pub fn file(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }
    pub fn uri(&self, rel: &str) -> Uri {
        csscls::uri::path_to_uri(&self.file(rel)).unwrap()
    }
    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.file(rel)).unwrap()
    }
    pub fn write(&self, rel: &str, text: &str) {
        let p = self.file(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    /// Annotated CSS locations of `name` across every `.css` file. Files
    /// without markers (the ignored ones) contribute nothing.
    pub fn expected_locations(&self, name: &str) -> Vec<Location> {
        self.expected(name, "css", Syntax::Css)
    }

    /// Annotated HTML usages of `name` across every `.html` file.
    pub fn expected_usages(&self, name: &str) -> Vec<Location> {
        self.expected(name, "html", Syntax::Html)
    }

    /// Definitions and usages together, as `references` with declarations returns them.
    pub fn expected_references(&self, name: &str) -> Vec<Location> {
        let mut v = self.expected_locations(name);
        v.extend(self.expected_usages(name));
        sort_locations(&mut v);
        v
    }

    fn expected(&self, name: &str, ext: &str, syntax: Syntax) -> Vec<Location> {
        let mut out = Vec::new();
        for path in walk_files(self.path(), ext) {
            let src = std::fs::read_to_string(&path).unwrap();
            let uri = csscls::uri::path_to_uri(&path).unwrap();
            for (e, range) in expected_ranges_in(&src, syntax) {
                if e.name == name {
                    out.push(Location {
                        uri: uri.clone(),
                        range,
                    });
                }
            }
        }
        sort_locations(&mut out);
        out
    }

    /// Cursor position `chars_in` characters into the `nth` annotated occurrence
    /// of `name` in `rel`.
    pub fn cursor_on(&self, rel: &str, name: &str, nth: usize, chars_in: u32) -> Position {
        let src = self.read(rel);
        let syntax = if rel.ends_with(".css") {
            Syntax::Css
        } else {
            Syntax::Html
        };
        let (_, r) = expected_ranges_in(&src, syntax)
            .into_iter()
            .filter(|(e, _)| e.name == name)
            .nth(nth)
            .unwrap_or_else(|| panic!("no occurrence #{nth} of `{name}` in {rel}"));
        Position::new(r.start.line, r.start.character + chars_in)
    }
}

pub fn sort_locations(locs: &mut [Location]) {
    locs.sort_by_key(|l| {
        (
            l.uri.as_str().to_owned(),
            l.range.start.line,
            l.range.start.character,
        )
    });
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

fn walk_files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            out.extend(walk_files(&p, ext));
        } else if p.extension().is_some_and(|e| e == ext) {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// Apply LSP text edits (UTF-16 positions, all relative to the original text).
pub fn apply_edits(text: &str, edits: &[lsp_types::TextEdit]) -> String {
    let li = csscls::line_index::LineIndex::new(text);
    let mut spans: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|e| {
            (
                li.offset(text, e.range.start),
                li.offset(text, e.range.end),
                e.new_text.as_str(),
            )
        })
        .collect();
    spans.sort_by_key(|s| std::cmp::Reverse(s.0));
    let mut out = text.to_owned();
    for (start, end, new) in spans {
        out.replace_range(start..end, new);
    }
    out
}

// ---------------------------------------------------------------------------
// In-process LSP client
// ---------------------------------------------------------------------------

const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Default, Clone)]
pub struct ClientOptions {
    pub workspace: Option<PathBuf>,
    pub dynamic_watcher_registration: bool,
    /// Sent as `initializationOptions`.
    pub init_options: Option<serde_json::Value>,
}

type ServerResult = csscls::error::Result<()>;

pub struct TestClient {
    conn: Connection,
    server: Option<JoinHandle<ServerResult>>,
    next_id: i32,
    /// Responses that arrived while waiting for a different one.
    stash: Vec<Response>,
    pub notifications: Vec<Notification>,
    /// Requests the server sent us (each is auto-answered with `null`).
    pub server_requests: Vec<Request>,
    pub init: InitializeResult,
}

impl TestClient {
    pub fn start(opts: ClientOptions) -> Self {
        let (server_conn, client_conn) = Connection::memory();
        let server = std::thread::spawn(move || csscls::server::run(server_conn));

        let mut client = Self {
            conn: client_conn,
            server: Some(server),
            next_id: 0,
            stash: Vec::new(),
            notifications: Vec::new(),
            server_requests: Vec::new(),
            init: InitializeResult::default(),
        };

        let folders = opts.workspace.as_ref().map(|p| {
            vec![WorkspaceFolder {
                uri: csscls::uri::path_to_uri(p).unwrap(),
                name: "test".into(),
            }]
        });
        let params = InitializeParams {
            workspace_folders: folders,
            initialization_options: opts.init_options.clone(),
            capabilities: ClientCapabilities {
                workspace: Some(WorkspaceClientCapabilities {
                    did_change_watched_files: Some(DidChangeWatchedFilesClientCapabilities {
                        dynamic_registration: Some(opts.dynamic_watcher_registration),
                        relative_pattern_support: None,
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        client.init = client.request::<lsp_types::request::Initialize>(params);
        client.notify::<lsp_types::notification::Initialized>(lsp_types::InitializedParams {});
        client
    }

    pub fn notify<N: lsp_types::notification::Notification>(&self, params: N::Params) {
        self.conn
            .sender
            .send(Message::Notification(Notification::new(
                N::METHOD.into(),
                params,
            )))
            .unwrap();
    }

    /// Send a request without waiting for its response.
    pub fn send<R: lsp_types::request::Request>(&mut self, params: R::Params) -> RequestId {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        self.conn
            .sender
            .send(Message::Request(Request::new(
                id.clone(),
                R::METHOD.into(),
                params,
            )))
            .unwrap();
        id
    }

    pub fn wait_raw(&mut self, id: &RequestId) -> Response {
        if let Some(i) = self.stash.iter().position(|r| &r.id == id) {
            return self.stash.remove(i);
        }
        loop {
            match self.conn.receiver.recv_timeout(TIMEOUT) {
                Ok(Message::Response(r)) if &r.id == id => return r,
                Ok(Message::Response(r)) => self.stash.push(r),
                Ok(Message::Notification(n)) => self.notifications.push(n),
                Ok(Message::Request(req)) => {
                    let reply = Response::new_ok(req.id.clone(), serde_json::Value::Null);
                    self.conn.sender.send(Message::Response(reply)).unwrap();
                    self.server_requests.push(req);
                }
                Err(e) => panic!(
                    "no response to {id:?} ({e}); notifications so far: {:?}",
                    self.notifications
                ),
            }
        }
    }

    pub fn wait<R: lsp_types::request::Request>(&mut self, id: RequestId) -> R::Result {
        match self.wait_raw(&id).response_result {
            Ok(value) => serde_json::from_value(value).unwrap(),
            Err(err) => panic!("{} failed: {err:?}", R::METHOD),
        }
    }

    pub fn request<R: lsp_types::request::Request>(&mut self, params: R::Params) -> R::Result {
        let id = self.send::<R>(params);
        self.wait::<R>(id)
    }

    /// Like `request`, but returns the error response instead of panicking.
    pub fn try_request<R: lsp_types::request::Request>(
        &mut self,
        params: R::Params,
    ) -> Result<R::Result, ResponseError> {
        let id = self.send::<R>(params);
        self.wait_raw(&id)
            .response_result
            .map(|v| serde_json::from_value(v).unwrap())
    }

    pub fn request_err<R: lsp_types::request::Request>(
        &mut self,
        params: R::Params,
    ) -> ResponseError {
        let id = self.send::<R>(params);
        self.wait_raw(&id)
            .response_result
            .expect_err("expected an error response")
    }

    /// A request with an arbitrary method name and JSON params.
    pub fn raw_request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ResponseError> {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        self.conn
            .sender
            .send(Message::Request(Request::new(
                id.clone(),
                method.into(),
                params,
            )))
            .unwrap();
        self.wait_raw(&id).response_result
    }

    /// Wait up to `timeout` for a notification with this method (already-received ones count).
    pub fn wait_notification(&mut self, method: &str, timeout: Duration) -> Option<Notification> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(i) = self.notifications.iter().position(|n| n.method == method) {
                return Some(self.notifications.remove(i));
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.conn.receiver.recv_timeout(left) {
                Ok(Message::Notification(n)) => self.notifications.push(n),
                Ok(Message::Response(r)) => self.stash.push(r),
                Ok(Message::Request(req)) => {
                    let reply = Response::new_ok(req.id.clone(), serde_json::Value::Null);
                    self.conn.sender.send(Message::Response(reply)).unwrap();
                    self.server_requests.push(req);
                }
                Err(_) => return None,
            }
        }
    }

    /// Wait until the server has gone idle on everything sent so far, by
    /// round-tripping a request. Valid because the server is sequential.
    pub fn barrier(&mut self) {
        let _ = self.request::<lsp_types::request::HoverRequest>(hover_params(
            &Uri::from_str("file:///barrier.css").unwrap(),
            Position::new(0, 0),
        ));
    }

    // Convenience wrappers ---------------------------------------------------

    pub fn open(&self, uri: &Uri, language_id: &str, text: &str) {
        self.notify::<lsp_types::notification::DidOpenTextDocument>(
            lsp_types::DidOpenTextDocumentParams {
                text_document: lsp_types::TextDocumentItem {
                    uri: uri.clone(),
                    language_id: language_id.into(),
                    version: 1,
                    text: text.into(),
                },
            },
        );
    }

    pub fn change(&self, uri: &Uri, version: i32, text: &str) {
        self.notify::<lsp_types::notification::DidChangeTextDocument>(
            lsp_types::DidChangeTextDocumentParams {
                text_document: lsp_types::VersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version,
                },
                content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: text.into(),
                }],
            },
        );
    }

    pub fn close(&self, uri: &Uri) {
        self.notify::<lsp_types::notification::DidCloseTextDocument>(
            lsp_types::DidCloseTextDocumentParams {
                text_document: lsp_types::TextDocumentIdentifier { uri: uri.clone() },
            },
        );
    }

    pub fn definition(
        &mut self,
        uri: &Uri,
        pos: Position,
    ) -> Option<lsp_types::GotoDefinitionResponse> {
        self.request::<lsp_types::request::GotoDefinition>(lsp_types::GotoDefinitionParams {
            text_document_position_params: position_params(uri, pos),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        })
    }

    pub fn definition_locations(&mut self, uri: &Uri, pos: Position) -> Vec<Location> {
        match self.definition(uri, pos) {
            Some(lsp_types::GotoDefinitionResponse::Array(v)) => v,
            Some(lsp_types::GotoDefinitionResponse::Scalar(l)) => vec![l],
            other => panic!("expected locations, got {other:?}"),
        }
    }

    pub fn hover(&mut self, uri: &Uri, pos: Position) -> Option<lsp_types::Hover> {
        self.request::<lsp_types::request::HoverRequest>(hover_params(uri, pos))
    }

    pub fn references(
        &mut self,
        uri: &Uri,
        pos: Position,
        include_declaration: bool,
    ) -> Option<Vec<Location>> {
        self.request::<lsp_types::request::References>(lsp_types::ReferenceParams {
            text_document_position: position_params(uri, pos),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: lsp_types::ReferenceContext {
                include_declaration,
            },
        })
    }

    /// Orderly shutdown; asserts the server thread exits cleanly.
    pub fn shutdown(mut self) {
        let _: () = self.request::<lsp_types::request::Shutdown>(());
        self.notify::<lsp_types::notification::Exit>(());
        let handle = self.server.take().unwrap();
        let deadline = std::time::Instant::now() + TIMEOUT;
        while !handle.is_finished() {
            assert!(std::time::Instant::now() < deadline, "server did not exit");
            std::thread::sleep(Duration::from_millis(5));
        }
        handle
            .join()
            .expect("server panicked")
            .expect("server returned an error");
    }
}

pub fn position_params(uri: &Uri, pos: Position) -> lsp_types::TextDocumentPositionParams {
    lsp_types::TextDocumentPositionParams {
        text_document: lsp_types::TextDocumentIdentifier { uri: uri.clone() },
        position: pos,
    }
}

pub fn hover_params(uri: &Uri, pos: Position) -> lsp_types::HoverParams {
    lsp_types::HoverParams {
        text_document_position_params: position_params(uri, pos),
        work_done_progress_params: Default::default(),
    }
}

pub fn uri(s: &str) -> Uri {
    Uri::from_str(s).unwrap()
}

/// Position `chars_in` characters into the first occurrence of `needle`.
pub fn position_of(text: &str, needle: &str, chars_in: usize) -> Position {
    let idx = text
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` not in {text:?}"))
        + chars_in;
    let line = text[..idx].matches('\n').count() as u32;
    let line_start = text[..idx].rfind('\n').map_or(0, |i| i + 1);
    Position::new(line, text[line_start..idx].encode_utf16().count() as u32)
}

/// Flatten a `WorkspaceEdit` (`changes` form) to uri -> edits.
pub fn edits_by_uri(
    edit: &lsp_types::WorkspaceEdit,
) -> std::collections::BTreeMap<String, Vec<lsp_types::TextEdit>> {
    edit.changes
        .as_ref()
        .expect("server must return `changes`")
        .iter()
        .map(|(u, e)| (u.as_str().to_owned(), e.clone()))
        .collect()
}

pub fn rename_params(uri: &Uri, pos: Position, new_name: &str) -> lsp_types::RenameParams {
    lsp_types::RenameParams {
        text_document_position: position_params(uri, pos),
        new_name: new_name.into(),
        work_done_progress_params: Default::default(),
    }
}

/// The most recent diagnostics published for `uri`; empty if none were ever published.
pub fn latest_diagnostics(client: &mut TestClient, uri: &Uri) -> Vec<lsp_types::Diagnostic> {
    client.barrier();
    let mut latest = Vec::new();
    for notification in &client.notifications {
        if notification.method != "textDocument/publishDiagnostics" {
            continue;
        }
        let Ok(params) = serde_json::from_value::<lsp_types::PublishDiagnosticsParams>(
            notification.params.clone(),
        ) else {
            continue;
        };
        if &params.uri == uri {
            latest = params.diagnostics;
        }
    }
    latest
}

/// (class name, severity) for each diagnostic, sorted by name.
pub fn diagnostic_summary(
    diagnostics: &[lsp_types::Diagnostic],
) -> Vec<(String, lsp_types::DiagnosticSeverity)> {
    let mut found: Vec<(String, lsp_types::DiagnosticSeverity)> = diagnostics
        .iter()
        .map(|diagnostic| {
            let name = diagnostic
                .message
                .split('`')
                .nth(1)
                .unwrap_or("")
                .to_owned();
            (
                name,
                diagnostic
                    .severity
                    .unwrap_or(lsp_types::DiagnosticSeverity::ERROR),
            )
        })
        .collect();
    found.sort_by(|left, right| left.0.cmp(&right.0));
    found
}

// ---- shared sessions and request helpers -----------------------------------

/// The `site` fixture copied to a temp dir, with a server started on it.
pub fn site_with(options: Option<serde_json::Value>) -> (TempWorkspace, TestClient) {
    let workspace = TempWorkspace::copy_fixture("site");
    let client = TestClient::start(ClientOptions {
        workspace: Some(workspace.path().to_path_buf()),
        init_options: options,
        ..Default::default()
    });
    (workspace, client)
}

pub fn site() -> (TempWorkspace, TestClient) {
    site_with(None)
}

/// A fixture from `fixtures/scenarios/<name>` with a server started on it.
pub fn scenario(name: &str) -> (TempWorkspace, TestClient) {
    let workspace = TempWorkspace::copy_fixture(&format!("scenarios/{name}"));
    let client = TestClient::start(ClientOptions {
        workspace: Some(workspace.path().to_path_buf()),
        ..Default::default()
    });
    (workspace, client)
}

/// The raw completion response at `position` in an already-known document.
pub fn complete(
    client: &mut TestClient,
    uri: &Uri,
    position: Position,
) -> Option<lsp_types::CompletionResponse> {
    client.request::<lsp_types::request::Completion>(lsp_types::CompletionParams {
        text_document_position: position_params(uri, position),
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
        context: None,
    })
}

/// The items of a completion response. Panics if the server returned none.
pub fn labels(response: Option<lsp_types::CompletionResponse>) -> Vec<String> {
    match response.expect("expected completion items") {
        lsp_types::CompletionResponse::Array(items) => {
            items.into_iter().map(|item| item.label).collect()
        }
        lsp_types::CompletionResponse::List(list) => {
            list.items.into_iter().map(|item| item.label).collect()
        }
    }
}

/// Completion items at `position`; no response means no items.
pub fn complete_items(
    client: &mut TestClient,
    uri: &Uri,
    position: Position,
) -> Vec<lsp_types::CompletionItem> {
    match complete(client, uri, position) {
        Some(lsp_types::CompletionResponse::Array(items)) => items,
        Some(lsp_types::CompletionResponse::List(list)) => list.items,
        None => Vec::new(),
    }
}

/// Opens `marked` (text with a `$0` cursor) as a buffer and completes at the cursor.
pub fn complete_marked(
    client: &mut TestClient,
    uri: &Uri,
    language_id: &str,
    marked: &str,
) -> Vec<lsp_types::CompletionItem> {
    let (text, position) = parse_cursor(marked);
    client.open(uri, language_id, &text);
    complete_items(client, uri, position)
}

/// The markdown of a hover response. Panics if there is no hover.
pub fn hover_text(hover: Option<lsp_types::Hover>) -> String {
    match hover.expect("expected hover").contents {
        lsp_types::HoverContents::Markup(markup) => markup.value,
        other => panic!("unexpected hover contents {other:?}"),
    }
}

// ---- multi-file fixtures ----------------------------------------------------
//
// A whole workspace as one string, for level-2 tests:
//
//     //- /a.css
//     .btn { }
//     //- /Page.svelte
//     <p class="$0">
//
// A `//- /path` line starts a file. `$0` marks the cursor in at most one file.

pub struct FixtureFile {
    pub path: String,
    pub text: String,
}

pub struct Fixture {
    pub files: Vec<FixtureFile>,
    /// Index into `files` and the position of the `$0` marker, if there is one.
    pub cursor: Option<(usize, Position)>,
}

pub fn parse_fixture(source: &str) -> Fixture {
    let mut files: Vec<FixtureFile> = Vec::new();
    let mut cursor = None;
    for line in source.split_inclusive('\n') {
        if let Some(path) = line.strip_prefix("//- ") {
            files.push(FixtureFile {
                path: path.trim().trim_start_matches('/').to_owned(),
                text: String::new(),
            });
            continue;
        }
        let file = files
            .last_mut()
            .expect("a fixture must start with a `//- /path` line");
        file.text.push_str(line);
    }
    for (index, file) in files.iter_mut().enumerate() {
        if file.text.contains("$0") {
            assert!(cursor.is_none(), "a fixture may contain only one `$0`");
            let (text, position) = parse_cursor(&file.text);
            file.text = text;
            cursor = Some((index, position));
        }
    }
    Fixture { files, cursor }
}

/// A workspace and server built from a multi-file fixture. Every file is on
/// disk before the server starts, so the initial scan sees all of them; the
/// cursor file is also opened as a live buffer.
pub struct Session {
    pub workspace: TempWorkspace,
    pub client: TestClient,
    pub uri: Uri,
    pub position: Position,
    pub language_id: String,
}

impl Session {
    pub fn start(source: &str) -> Self {
        let fixture = parse_fixture(source);
        let (index, position) = fixture.cursor.expect("the fixture needs a `$0` cursor");
        let workspace = TempWorkspace::empty();
        for file in &fixture.files {
            workspace.write(&file.path, &file.text);
        }
        let client = TestClient::start(ClientOptions {
            workspace: Some(workspace.path().to_path_buf()),
            ..Default::default()
        });
        let cursor_file = &fixture.files[index];
        let language_id = cursor_file
            .path
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_owned();
        let uri = workspace.uri(&cursor_file.path);
        client.open(&uri, &language_id, &cursor_file.text);
        Self {
            workspace,
            client,
            uri,
            position,
            language_id,
        }
    }
}

/// Labels offered at the `$0` cursor of a multi-file fixture.
pub fn completion_labels(source: &str) -> Vec<String> {
    let mut session = Session::start(source);
    let items = complete_items(&mut session.client, &session.uri, session.position);
    session.client.shutdown();
    items.into_iter().map(|item| item.label).collect()
}
