//! Shared test tooling: fixture annotations, cursor markers, and an in-process
//! LSP client. Included by each integration test via `mod common;`.
#![allow(dead_code)]

use css_class_lsp::css::ClassDef;
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response, ResponseError};
use lsp_types::{
    ClientCapabilities, DidChangeWatchedFilesClientCapabilities, InitializeParams, InitializeResult,
    Location, Position, Range, Uri, WorkspaceClientCapabilities, WorkspaceFolder,
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
        let Some(classes) = marker(line, open, close) else { continue };
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

/// What the indexer actually found, in the same shape as the annotations.
pub fn describe(src: &str, defs: &[ClassDef]) -> Vec<Expected> {
    defs.iter()
        .map(|d| Expected {
            line: src[..d.name_span.start as usize].matches('\n').count() as u32,
            raw: src[d.name_span.range()].to_owned(),
            name: d.name.to_string(),
            ctx: d.context.iter().map(|c| c.to_string()).collect(),
        })
        .collect()
}

/// Same shape, for markup usages.
pub fn describe_usages(src: &str, usages: &[css_class_lsp::markup::Usage]) -> Vec<Expected> {
    usages
        .iter()
        .map(|u| Expected {
            line: src[..u.span.start as usize].matches('\n').count() as u32,
            raw: src[u.span.range()].to_owned(),
            name: u.name.to_string(),
            ctx: Vec::new(),
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
        css_class_lsp::uri::path_to_uri(&self.file(rel)).unwrap()
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
            let uri = css_class_lsp::uri::path_to_uri(&path).unwrap();
            for (e, range) in expected_ranges_in(&src, syntax) {
                if e.name == name {
                    out.push(Location { uri: uri.clone(), range });
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
        let syntax = if rel.ends_with(".css") { Syntax::Css } else { Syntax::Html };
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
        (l.uri.as_str().to_owned(), l.range.start.line, l.range.start.character)
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
    let li = css_class_lsp::line_index::LineIndex::new(text);
    let mut spans: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|e| (li.offset(text, e.range.start), li.offset(text, e.range.end), e.new_text.as_str()))
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

type ServerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

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
        let server = std::thread::spawn(move || css_class_lsp::server::run(server_conn));

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
                uri: css_class_lsp::uri::path_to_uri(p).unwrap(),
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
            .send(Message::Notification(Notification::new(N::METHOD.into(), params)))
            .unwrap();
    }

    /// Send a request without waiting for its response.
    pub fn send<R: lsp_types::request::Request>(&mut self, params: R::Params) -> RequestId {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        self.conn
            .sender
            .send(Message::Request(Request::new(id.clone(), R::METHOD.into(), params)))
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
                Err(e) => panic!("no response to {id:?} ({e}); notifications so far: {:?}", self.notifications),
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
    pub fn try_request<R: lsp_types::request::Request>(&mut self, params: R::Params) -> Result<R::Result, ResponseError> {
        let id = self.send::<R>(params);
        self.wait_raw(&id).response_result.map(|v| serde_json::from_value(v).unwrap())
    }

    pub fn request_err<R: lsp_types::request::Request>(&mut self, params: R::Params) -> ResponseError {
        let id = self.send::<R>(params);
        self.wait_raw(&id).response_result.expect_err("expected an error response")
    }

    /// A request with an arbitrary method name and JSON params.
    pub fn raw_request(&mut self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, ResponseError> {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        self.conn
            .sender
            .send(Message::Request(Request::new(id.clone(), method.into(), params)))
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
                text_document: lsp_types::VersionedTextDocumentIdentifier { uri: uri.clone(), version },
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

    pub fn definition(&mut self, uri: &Uri, pos: Position) -> Option<lsp_types::GotoDefinitionResponse> {
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

    pub fn references(&mut self, uri: &Uri, pos: Position, include_declaration: bool) -> Option<Vec<Location>> {
        self.request::<lsp_types::request::References>(lsp_types::ReferenceParams {
            text_document_position: position_params(uri, pos),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: lsp_types::ReferenceContext { include_declaration },
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
        handle.join().expect("server panicked").expect("server returned an error");
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
    let idx = text.find(needle).unwrap_or_else(|| panic!("`{needle}` not in {text:?}")) + chars_in;
    let line = text[..idx].matches('\n').count() as u32;
    let line_start = text[..idx].rfind('\n').map_or(0, |i| i + 1);
    Position::new(line, text[line_start..idx].encode_utf16().count() as u32)
}

/// Flatten a `WorkspaceEdit` (`changes` form) to uri -> edits.
pub fn edits_by_uri(edit: &lsp_types::WorkspaceEdit) -> std::collections::BTreeMap<String, Vec<lsp_types::TextEdit>> {
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
