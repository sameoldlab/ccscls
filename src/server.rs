//! The LSP message loop.
//!
//! One thread, strictly sequential: messages are handled in the order they
//! arrive, so a `didChange` is always applied before a later request sees the
//! index. (This is why the server is built on `lsp-server` rather than an
//! async framework that may reorder notifications.)
//!
//! Known limitation: the initial workspace scan runs on this thread, so
//! requests that arrive during a scan are answered after it finishes.
//!
//! Set `CSS_CLASS_LSP_LOG=1` to log every message and its handling time to stderr.

use crate::css::Span;
use crate::markup;
use crate::uri::{file_key, path_to_uri, uri_to_path};
use crate::workspace::{DefRef, FileEntry, FileKind, Workspace};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument,
    Notification as LspNotification,
};
use lsp_types::request::{
    Completion, GotoDefinition, HoverRequest, References, Request as LspRequest,
};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionOptions, CompletionParams, CompletionResponse,
    CompletionTextEdit, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidChangeWatchedFilesRegistrationOptions, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, FileChangeType, FileSystemWatcher, GlobPattern, GotoDefinitionParams,
    GotoDefinitionResponse, Hover, HoverContents, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, Location, MarkupContent, MarkupKind, OneOf, Range,
    ReferenceParams, Registration, RegistrationParams, ServerCapabilities, ServerInfo,
    TextDocumentPositionParams, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextEdit,
};
use serde::{Serialize, de::DeserializeOwned};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Instant;

type BoxError = Box<dyn Error + Send + Sync>;

const MAX_FILE_BYTES: u64 = 2_000_000;
const MAX_HOVER_RULES: usize = 5;
const MAX_HOVER_RULE_CHARS: usize = 2_000;

pub fn capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(TextDocumentSyncOptions {
            open_close: Some(true),
            change: Some(TextDocumentSyncKind::FULL),
            ..Default::default()
        })),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec!["\"".into(), "'".into(), " ".into()]),
            ..Default::default()
        }),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        ..Default::default()
    }
}

/// Run the server on `connection` until the client shuts it down.
pub fn run(connection: Connection) -> Result<(), BoxError> {
    let (id, params) = connection.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(params)?;
    let result = InitializeResult {
        capabilities: capabilities(),
        server_info: Some(ServerInfo {
            name: "css-class-lsp".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
    };
    connection.initialize_finish(id, serde_json::to_value(result)?)?;

    let mut server = Server::new(&connection, &params);
    if let Some(error) = server.ws.css_load_error() {
        eprintln!("[css-class-lsp] the CSS grammar failed to load, so CSS indexing is disabled: {error}");
    }
    // `initialize_finish` has already consumed the client's `initialized`
    // notification, so the post-handshake work happens here.
    server.on_initialized()?;
    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }
                let (method, started) = (req.method.clone(), Instant::now());
                server.handle_request(req)?;
                server.log(format_args!("request {method} ({:?})", started.elapsed()));
            }
            Message::Notification(note) => {
                let (method, started) = (note.method.clone(), Instant::now());
                server.handle_notification(note)?;
                server.log(format_args!("notification {method} ({:?})", started.elapsed()));
            }
            // replies to our own registerCapability
            Message::Response(_) => {}
        }
    }
    Ok(())
}

struct Server<'c> {
    conn: &'c Connection,
    ws: Workspace,
    roots: Vec<PathBuf>,
    can_register_watchers: bool,
    log: bool,
}

impl<'c> Server<'c> {
    // root_uri: still sent by older clients
    #[allow(deprecated)]
    fn new(conn: &'c Connection, params: &InitializeParams) -> Self {
        let mut roots: Vec<PathBuf> = params
            .workspace_folders
            .iter()
            .flatten()
            .filter_map(|f| uri_to_path(&f.uri))
            .collect();
        if roots.is_empty() {
            roots.extend(params.root_uri.as_ref().and_then(uri_to_path));
        }
        let can_register_watchers = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|w| w.did_change_watched_files.as_ref())
            .and_then(|d| d.dynamic_registration)
            .unwrap_or(false);
        Self {
            conn,
            ws: Workspace::new(),
            roots,
            can_register_watchers,
            log: std::env::var_os("CSS_CLASS_LSP_LOG").is_some(),
        }
    }

    fn log(&self, args: std::fmt::Arguments<'_>) {
        if self.log {
            eprintln!("[css-class-lsp] {args}");
        }
    }

    // ---- notifications -------------------------------------------------

    fn handle_notification(&mut self, note: Notification) -> Result<(), BoxError> {
        match note.method.as_str() {
            DidOpenTextDocument::METHOD => {
                if let Ok(p) = serde_json::from_value::<DidOpenTextDocumentParams>(note.params) {
                    self.on_open(p);
                }
            }
            DidChangeTextDocument::METHOD => {
                if let Ok(p) = serde_json::from_value::<DidChangeTextDocumentParams>(note.params) {
                    self.on_change(p);
                }
            }
            DidCloseTextDocument::METHOD => {
                if let Ok(p) = serde_json::from_value::<DidCloseTextDocumentParams>(note.params) {
                    self.on_close(p);
                }
            }
            DidChangeWatchedFiles::METHOD => {
                if let Ok(p) = serde_json::from_value::<DidChangeWatchedFilesParams>(note.params) {
                    self.on_watched_files(p);
                }
            }
            // unknown notifications must be ignored per the spec
            _ => {}
        }
        Ok(())
    }

    fn on_initialized(&mut self) -> Result<(), BoxError> {
        let started = Instant::now();
        for root in self.roots.clone() {
            self.scan(&root);
        }
        self.log(format_args!(
            "scanned {} files under {:?} in {:?}",
            self.ws.file_count(),
            self.roots,
            started.elapsed()
        ));
        if self.can_register_watchers {
            let options = DidChangeWatchedFilesRegistrationOptions {
                watchers: ["**/*.css", "**/*.html", "**/*.htm"]
                    .into_iter()
                    .map(|g| FileSystemWatcher {
                        glob_pattern: GlobPattern::String(g.into()),
                        kind: None,
                    })
                    .collect(),
            };
            let params = RegistrationParams {
                registrations: vec![Registration {
                    id: "css-class-lsp-watch".into(),
                    method: DidChangeWatchedFiles::METHOD.into(),
                    register_options: Some(serde_json::to_value(options)?),
                }],
            };
            self.conn.sender.send(Message::Request(Request::new(
                RequestId::from("css-class-lsp/register-watchers".to_string()),
                "client/registerCapability".into(),
                params,
            )))?;
        }
        Ok(())
    }

    fn scan(&mut self, root: &Path) {
        let walker = ignore::WalkBuilder::new(root)
            .require_git(false)
            .filter_entry(|e| e.file_name().to_str() != Some("node_modules"))
            .build();
        for entry in walker.flatten() {
            let path = entry.path();
            if entry.file_type().is_some_and(|t| t.is_file()) {
                if let Some(kind) = FileKind::detect(None, Some(path)) {
                    self.load_from_disk(path, kind);
                }
            }
        }
    }

    /// Index `path` from disk unless the client has it open (the buffer wins).
    fn load_from_disk(&mut self, path: &Path, kind: FileKind) {
        let Some(uri) = path_to_uri(path) else { return };
        if self.ws.is_open(&file_key(&uri)) {
            return;
        }
        let Ok(meta) = std::fs::metadata(path) else { return };
        if meta.len() > MAX_FILE_BYTES {
            return;
        }
        // Skips files that are not valid UTF-8.
        if let Ok(text) = std::fs::read_to_string(path) {
            self.ws.upsert(uri, text, kind, None);
        }
    }

    fn on_open(&mut self, p: DidOpenTextDocumentParams) {
        let doc = p.text_document;
        let path = uri_to_path(&doc.uri);
        if let Some(kind) = FileKind::detect(Some(&doc.language_id), path.as_deref()) {
            self.ws.upsert(doc.uri, doc.text, kind, Some(doc.version));
        }
    }

    fn on_change(&mut self, p: DidChangeTextDocumentParams) {
        let key = file_key(&p.text_document.uri);
        let Some(kind) = self.ws.get(&key).filter(|f| f.open_version.is_some()).map(|f| f.kind) else {
            return;
        };
        // We advertise full sync, so a compliant client sends the whole text.
        if let Some(change) = p.content_changes.into_iter().rev().find(|c| c.range.is_none()) {
            self.ws.upsert(
                p.text_document.uri,
                change.text,
                kind,
                Some(p.text_document.version),
            );
        }
    }

    fn on_close(&mut self, p: DidCloseTextDocumentParams) {
        let key = file_key(&p.text_document.uri);
        let Some(kind) = self.ws.get(&key).filter(|f| f.open_version.is_some()).map(|f| f.kind) else {
            return;
        };
        // Fall back to what is on disk, or forget the file if there is none.
        self.ws.remove(&key);
        if let Some(path) = uri_to_path(&p.text_document.uri) {
            self.load_from_disk(&path, kind);
        }
    }

    fn on_watched_files(&mut self, p: DidChangeWatchedFilesParams) {
        for event in p.changes {
            let Some(path) = uri_to_path(&event.uri) else { continue };
            let Some(kind) = FileKind::detect(None, Some(&path)) else { continue };
            if has_component(&path, "node_modules") {
                continue;
            }
            let key = file_key(&event.uri);
            if event.typ == FileChangeType::DELETED {
                if !self.ws.is_open(&key) {
                    self.ws.remove(&key);
                }
            } else {
                self.load_from_disk(&path, kind);
            }
        }
    }

    // ---- requests -------------------------------------------------------

    fn handle_request(&mut self, req: Request) -> Result<(), BoxError> {
        match req.method.as_str() {
            Completion::METHOD => self.dispatch::<Completion>(req, Self::completion),
            HoverRequest::METHOD => self.dispatch::<HoverRequest>(req, Self::hover),
            GotoDefinition::METHOD => self.dispatch::<GotoDefinition>(req, Self::definition),
            References::METHOD => self.dispatch::<References>(req, Self::references),
            other => {
                let msg = format!("method not supported: {other}");
                self.send(Response::new_err(req.id, ErrorCode::MethodNotFound as i32, msg))
            }
        }
    }

    fn dispatch<R>(&mut self, req: Request, f: fn(&Self, R::Params) -> R::Result) -> Result<(), BoxError>
    where
        R: LspRequest,
        R::Params: DeserializeOwned,
        R::Result: Serialize,
    {
        let response = match serde_json::from_value::<R::Params>(req.params) {
            Ok(params) => Response::new_ok(req.id, f(self, params)),
            Err(e) => Response::new_err(req.id, ErrorCode::InvalidParams as i32, e.to_string()),
        };
        self.send(response)
    }

    fn send(&self, response: Response) -> Result<(), BoxError> {
        self.conn.sender.send(Message::Response(response))?;
        Ok(())
    }

    /// The file and byte offset a position refers to.
    fn locate(&self, pos: &TextDocumentPositionParams) -> Option<(String, &FileEntry, usize)> {
        let key = file_key(&pos.text_document.uri);
        let file = self.ws.get(&key)?;
        let offset = file.line_index.offset(&file.text, pos.position);
        Some((key, file, offset))
    }

    fn completion(&self, p: CompletionParams) -> Option<CompletionResponse> {
        let (_, file, offset) = self.locate(&p.text_document_position)?;
        if file.kind != FileKind::Html {
            return None;
        }
        let ctx = markup::class_context_at(&file.text, offset)?;
        let range = range_of(file, ctx.replace);
        let items = self
            .ws
            .class_names()
            .into_iter()
            .map(|(name, rules)| CompletionItem {
                label: name.to_owned(),
                kind: Some(CompletionItemKind::CLASS),
                detail: Some(format!("{rules} rule{}", if rules == 1 { "" } else { "s" })),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range,
                    new_text: name.to_owned(),
                })),
                ..Default::default()
            })
            .collect();
        Some(CompletionResponse::Array(items))
    }

    fn hover(&self, p: HoverParams) -> Option<Hover> {
        let (key, file, offset) = self.locate(&p.text_document_position_params)?;
        let target = self.ws.target_at(&key, offset)?;
        let raw = &file.text[target.span.range()];
        let defs = self.ws.defs_named(target.name);
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: hover_markdown(raw, &defs),
            }),
            range: Some(range_of(file, target.span)),
        })
    }

    fn definition(&self, p: GotoDefinitionParams) -> Option<GotoDefinitionResponse> {
        let (key, _, offset) = self.locate(&p.text_document_position_params)?;
        let target = self.ws.target_at(&key, offset)?;
        let locations = self.def_locations(target.name);
        (!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations))
    }

    fn references(&self, p: ReferenceParams) -> Option<Vec<Location>> {
        let (key, _, offset) = self.locate(&p.text_document_position)?;
        let target = self.ws.target_at(&key, offset)?;
        let mut out = Vec::new();
        if p.context.include_declaration {
            out.extend(self.def_locations(target.name));
        }
        out.extend(self.ws.usages_named(target.name).iter().map(|u| Location {
            uri: u.file.uri.clone(),
            range: range_of(u.file, u.usage.span),
        }));
        out.sort_by_key(|l| (l.uri.as_str().to_owned(), l.range.start.line, l.range.start.character));
        Some(out)
    }

    fn def_locations(&self, name: &str) -> Vec<Location> {
        self.ws
            .defs_named(name)
            .iter()
            .map(|d| Location {
                uri: d.file.uri.clone(),
                range: range_of(d.file, d.def.name_span),
            })
            .collect()
    }
}

fn range_of(file: &FileEntry, span: Span) -> Range {
    Range::new(
        file.line_index.position(&file.text, span.start as usize),
        file.line_index.position(&file.text, span.end as usize),
    )
}

fn hover_markdown(raw_name: &str, defs: &[DefRef<'_>]) -> String {
    let n = defs.len();
    if n == 0 {
        return format!("**`.{raw_name}`** — no matching rule in the workspace CSS");
    }
    let mut out = format!("**`.{raw_name}`** — {n} rule{}", if n == 1 { "" } else { "s" });
    for d in defs.iter().take(MAX_HOVER_RULES) {
        let file_name = uri_to_path(&d.file.uri)
            .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
            .unwrap_or_else(|| d.file.uri.as_str().to_owned());
        let line = d.file.line_index.position(&d.file.text, d.def.name_span.start as usize).line + 1;
        out.push_str(&format!("\n\n`{file_name}:{line}`"));
        for ctx in &d.def.context {
            out.push_str(&format!(" · `{ctx}`"));
        }
        let rule = d.file.text[d.def.rule_span.range()].trim();
        let rule = match rule.char_indices().nth(MAX_HOVER_RULE_CHARS) {
            Some((i, _)) => format!("{}…", &rule[..i]),
            None => rule.to_owned(),
        };
        out.push_str(&format!("\n\n```css\n{rule}\n```"));
    }
    if n > MAX_HOVER_RULES {
        out.push_str(&format!("\n\n…and {} more", n - MAX_HOVER_RULES));
    }
    out
}

fn has_component(p: &Path, name: &str) -> bool {
    p.components().any(|c| c.as_os_str() == name)
}
