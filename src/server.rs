//! The LSP message loop.
//!
//! One thread, strictly sequential: messages are handled in the order they
//! arrive, so a `didChange` is always applied before a later request sees the
//! index. (This is why the server is built on `lsp-server` rather than an
//! async framework that may reorder notifications.)
//!
//! `Server` is plain state with no borrowed data: handlers take a request and
//! return the messages to send, and only `run` touches the connection.
//!
//! Failure policy: the loop ends only on shutdown or a closed transport. A
//! handler that panics is caught at the message boundary and answered with an
//! internal-error response; the server keeps serving.
//!
//! Known limitation: the initial workspace scan runs on this thread, so
//! requests that arrive during a scan are answered after it finishes.
//!
//! Set `CSS_CLASS_LSP_LOG=1` to log every message and its handling time to stderr.

use crate::analysis::{Role, Span};
use crate::error::{Context, Result};
use crate::languages::Language;
use crate::uri::{file_key, path_to_uri, uri_to_path};
use crate::workspace::{FileEntry, OccurrenceId, Workspace};
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
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::time::Instant;

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

/// Run `work`, turning a panic into its message instead of unwinding further.
pub fn catch_panic<T>(work: impl FnOnce() -> T) -> std::result::Result<T, String> {
    std::panic::catch_unwind(AssertUnwindSafe(work)).map_err(|payload| {
        payload
            .downcast_ref::<&str>()
            .map(|message| (*message).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_owned())
    })
}

/// Run the server on `connection` until the client shuts it down.
pub fn run(connection: Connection) -> Result<()> {
    let (id, params) = connection
        .initialize_start()
        .context("waiting for the initialize request")?;
    let params: InitializeParams =
        serde_json::from_value(params).context("reading the initialize parameters")?;
    let result = InitializeResult {
        capabilities: capabilities(),
        server_info: Some(ServerInfo {
            name: "css-class-lsp".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
    };
    let result = serde_json::to_value(result).context("encoding the initialize result")?;
    connection
        .initialize_finish(id, result)
        .context("finishing the initialize handshake")?;

    let mut server = Server::new(&params);
    if let Some(error) = server.workspace.load_error() {
        eprintln!("[css-class-lsp] a grammar failed to load, so its files are not indexed: {error}");
    }
    // `initialize_finish` has already consumed the client's `initialized`
    // notification, so the post-handshake work happens here.
    for message in server.on_initialized().context("starting up")? {
        send(&connection, message)?;
    }
    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                if connection
                    .handle_shutdown(&request)
                    .context("handling the shutdown request")?
                {
                    return Ok(());
                }
                let (method, started) = (request.method.clone(), Instant::now());
                for outgoing in server.handle_request(request) {
                    send(&connection, outgoing)?;
                }
                server.log(format_args!("request {method} ({:?})", started.elapsed()));
            }
            Message::Notification(notification) => {
                let (method, started) = (notification.method.clone(), Instant::now());
                server.handle_notification(notification);
                server.log(format_args!("notification {method} ({:?})", started.elapsed()));
            }
            // Replies to our own registerCapability request.
            Message::Response(_) => {}
        }
    }
    Ok(())
}

fn send(connection: &Connection, message: Message) -> Result<()> {
    connection
        .sender
        .send(message)
        .context("sending a message to the client")
}

struct Server {
    workspace: Workspace,
    roots: Vec<PathBuf>,
    can_register_watchers: bool,
    log_enabled: bool,
}

impl Server {
    #[allow(deprecated)]
    fn new(params: &InitializeParams) -> Self {
        let mut roots: Vec<PathBuf> = params
            .workspace_folders
            .iter()
            .flatten()
            .filter_map(|folder| uri_to_path(&folder.uri))
            .collect();
        if roots.is_empty() {
            // `root_uri` is deprecated but older clients still send only that.
            roots.extend(params.root_uri.as_ref().and_then(uri_to_path));
        }
        let can_register_watchers = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.did_change_watched_files.as_ref())
            .and_then(|watched| watched.dynamic_registration)
            .unwrap_or(false);
        let mut workspace = Workspace::new();
        workspace.set_roots(roots.clone());
        Self {
            workspace,
            roots,
            can_register_watchers,
            log_enabled: std::env::var_os("CSS_CLASS_LSP_LOG").is_some(),
        }
    }

    fn log(&self, args: std::fmt::Arguments<'_>) {
        if self.log_enabled {
            eprintln!("[css-class-lsp] {args}");
        }
    }

    // ---- notifications -------------------------------------------------

    fn handle_notification(&mut self, notification: Notification) {
        let method = notification.method.clone();
        let outcome = catch_panic(|| self.apply_notification(notification));
        if let Err(message) = outcome {
            eprintln!("[css-class-lsp] panic while handling {method}: {message}");
        }
    }

    fn apply_notification(&mut self, notification: Notification) {
        match notification.method.as_str() {
            DidOpenTextDocument::METHOD => {
                if let Ok(params) = serde_json::from_value::<DidOpenTextDocumentParams>(notification.params) {
                    self.on_open(params);
                }
            }
            DidChangeTextDocument::METHOD => {
                if let Ok(params) = serde_json::from_value::<DidChangeTextDocumentParams>(notification.params) {
                    self.on_change(params);
                }
            }
            DidCloseTextDocument::METHOD => {
                if let Ok(params) = serde_json::from_value::<DidCloseTextDocumentParams>(notification.params) {
                    self.on_close(params);
                }
            }
            DidChangeWatchedFiles::METHOD => {
                if let Ok(params) = serde_json::from_value::<DidChangeWatchedFilesParams>(notification.params) {
                    self.on_watched_files(params);
                }
            }
            // Unknown notifications must be ignored per the spec.
            _ => {}
        }
    }

    /// Scan the workspace and, if the client allows it, ask it to watch our files.
    fn on_initialized(&mut self) -> Result<Vec<Message>> {
        let started = Instant::now();
        for root in self.roots.clone() {
            self.scan(&root);
        }
        self.log(format_args!(
            "scanned {} files under {:?} in {:?}",
            self.workspace.file_count(),
            self.roots,
            started.elapsed()
        ));
        if !self.can_register_watchers {
            return Ok(Vec::new());
        }
        let options = DidChangeWatchedFilesRegistrationOptions {
            watchers: Language::watch_globs()
                .into_iter()
                .map(|glob| FileSystemWatcher { glob_pattern: GlobPattern::String(glob), kind: None })
                .collect(),
        };
        let params = RegistrationParams {
            registrations: vec![Registration {
                id: "css-class-lsp-watch".into(),
                method: DidChangeWatchedFiles::METHOD.into(),
                register_options: Some(
                    serde_json::to_value(options).context("encoding the watcher registration")?,
                ),
            }],
        };
        Ok(vec![Message::Request(Request::new(
            RequestId::from("css-class-lsp/register-watchers".to_string()),
            "client/registerCapability".into(),
            params,
        ))])
    }

    fn scan(&mut self, root: &Path) {
        let walker = ignore::WalkBuilder::new(root)
            .require_git(false)
            .filter_entry(|entry| entry.file_name().to_str() != Some("node_modules"))
            .build();
        for entry in walker.flatten() {
            let path = entry.path();
            if entry.file_type().is_some_and(|kind| kind.is_file()) {
                if let Some(language) = Language::detect(None, Some(path)) {
                    self.load_from_disk(path, language);
                }
            }
        }
    }

    /// Index `path` from disk unless the client has it open (the buffer wins).
    fn load_from_disk(&mut self, path: &Path, language: Language) {
        let Some(uri) = path_to_uri(path) else { return };
        if self.workspace.is_open(&file_key(&uri)) {
            return;
        }
        let Ok(metadata) = std::fs::metadata(path) else { return };
        if metadata.len() > MAX_FILE_BYTES {
            return;
        }
        // Skips files that are not valid UTF-8.
        if let Ok(text) = std::fs::read_to_string(path) {
            self.workspace.upsert(uri, text, language, None);
        }
    }

    fn on_open(&mut self, params: DidOpenTextDocumentParams) {
        let document = params.text_document;
        let path = uri_to_path(&document.uri);
        if let Some(language) = Language::detect(Some(&document.language_id), path.as_deref()) {
            self.workspace
                .upsert(document.uri, document.text, language, Some(document.version));
        }
    }

    fn on_change(&mut self, params: DidChangeTextDocumentParams) {
        let key = file_key(&params.text_document.uri);
        let Some(language) = self.open_language(&key) else {
            return;
        };
        // We advertise full sync, so a compliant client sends the whole text.
        let whole_text_change = params
            .content_changes
            .into_iter()
            .rev()
            .find(|change| change.range.is_none());
        if let Some(change) = whole_text_change {
            self.workspace.upsert(
                params.text_document.uri,
                change.text,
                language,
                Some(params.text_document.version),
            );
        }
    }

    fn on_close(&mut self, params: DidCloseTextDocumentParams) {
        let key = file_key(&params.text_document.uri);
        let Some(language) = self.open_language(&key) else {
            return;
        };
        // Fall back to what is on disk, or forget the file if there is none.
        self.workspace.remove(&key);
        if let Some(path) = uri_to_path(&params.text_document.uri) {
            self.load_from_disk(&path, language);
        }
    }

    fn on_watched_files(&mut self, params: DidChangeWatchedFilesParams) {
        for event in params.changes {
            let Some(path) = uri_to_path(&event.uri) else { continue };
            let Some(language) = Language::detect(None, Some(&path)) else { continue };
            if path.components().any(|part| part.as_os_str() == "node_modules") {
                continue;
            }
            let key = file_key(&event.uri);
            if event.typ == FileChangeType::DELETED {
                if !self.workspace.is_open(&key) {
                    self.workspace.remove(&key);
                }
            } else {
                self.load_from_disk(&path, language);
            }
        }
    }

    /// The language of a file the client currently has open.
    fn open_language(&self, key: &str) -> Option<Language> {
        self.workspace
            .file_by_key(key)
            .filter(|file| file.open_version.is_some())
            .map(|file| file.language)
    }

    // ---- requests -------------------------------------------------------

    fn handle_request(&self, request: Request) -> Vec<Message> {
        match request.method.as_str() {
            Completion::METHOD => self.dispatch::<Completion>(request, Self::completion),
            HoverRequest::METHOD => self.dispatch::<HoverRequest>(request, Self::hover),
            GotoDefinition::METHOD => self.dispatch::<GotoDefinition>(request, Self::definition),
            References::METHOD => self.dispatch::<References>(request, Self::references),
            other => {
                let message = format!("method not supported: {other}");
                let code = ErrorCode::MethodNotFound as i32;
                vec![Message::Response(Response::new_err(request.id, code, message))]
            }
        }
    }

    fn dispatch<R>(&self, request: Request, handler: fn(&Self, R::Params) -> R::Result) -> Vec<Message>
    where
        R: LspRequest,
        R::Params: DeserializeOwned,
        R::Result: Serialize,
    {
        let id = request.id;
        let response = match serde_json::from_value::<R::Params>(request.params) {
            Ok(params) => match catch_panic(|| handler(self, params)) {
                Ok(result) => Response::new_ok(id, result),
                Err(message) => {
                    eprintln!("[css-class-lsp] panic while handling a request: {message}");
                    let code = ErrorCode::InternalError as i32;
                    Response::new_err(id, code, format!("internal error: {message}"))
                }
            },
            Err(error) => Response::new_err(id, ErrorCode::InvalidParams as i32, error.to_string()),
        };
        vec![Message::Response(response)]
    }

    /// The occurrence the cursor is on, whichever kind of file it is in.
    fn occurrence_under_cursor(&self, params: &TextDocumentPositionParams) -> Option<OccurrenceId> {
        let key = file_key(&params.text_document.uri);
        let file = self.workspace.file_by_key(&key)?;
        let offset = file.line_index.offset(&file.text, params.position);
        self.workspace.occurrence_at(&key, offset)
    }

    fn completion(&self, params: CompletionParams) -> Option<CompletionResponse> {
        let position = &params.text_document_position;
        let key = file_key(&position.text_document.uri);
        let file = self.workspace.file_by_key(&key)?;
        let offset = file.line_index.offset(&file.text, position.position);
        let context = file.language.class_context_at(&file.text, offset)?;
        let range = range_of(file, context.replace);
        let items = self
            .workspace
            .class_names()
            .into_iter()
            .map(|(name, rule_count)| {
                let name = self.workspace.name(name);
                CompletionItem {
                    label: name.to_owned(),
                    kind: Some(CompletionItemKind::CLASS),
                    detail: Some(format!("{rule_count} rule{}", if rule_count == 1 { "" } else { "s" })),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                        range,
                        new_text: name.to_owned(),
                    })),
                    ..Default::default()
                }
            })
            .collect();
        Some(CompletionResponse::Array(items))
    }

    fn hover(&self, params: HoverParams) -> Option<Hover> {
        let id = self.occurrence_under_cursor(&params.text_document_position_params)?;
        let (file, occurrence) = self.workspace.resolve(id)?;
        let raw_name = file.text.get(occurrence.name_span.range())?;
        let definitions = self.workspace.occurrences_named(Role::Definition, occurrence.name);
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: self.hover_markdown(raw_name, &definitions),
            }),
            range: Some(range_of(file, occurrence.name_span)),
        })
    }

    fn definition(&self, params: GotoDefinitionParams) -> Option<GotoDefinitionResponse> {
        let id = self.occurrence_under_cursor(&params.text_document_position_params)?;
        let (_, occurrence) = self.workspace.resolve(id)?;
        let locations = self.locations(self.workspace.occurrences_named(Role::Definition, occurrence.name));
        (!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations))
    }

    fn references(&self, params: ReferenceParams) -> Option<Vec<Location>> {
        let id = self.occurrence_under_cursor(&params.text_document_position)?;
        let (_, occurrence) = self.workspace.resolve(id)?;
        let mut found = Vec::new();
        if params.context.include_declaration {
            found.extend(self.locations(self.workspace.occurrences_named(Role::Definition, occurrence.name)));
        }
        found.extend(self.locations(self.workspace.occurrences_named(Role::Usage, occurrence.name)));
        found.sort_by_key(|location| {
            (location.uri.as_str().to_owned(), location.range.start.line, location.range.start.character)
        });
        Some(found)
    }

    fn locations(&self, ids: Vec<OccurrenceId>) -> Vec<Location> {
        ids.into_iter()
            .filter_map(|id| self.workspace.resolve(id))
            .map(|(file, occurrence)| Location {
                uri: file.uri.clone(),
                range: range_of(file, occurrence.name_span),
            })
            .collect()
    }

    fn hover_markdown(&self, raw_name: &str, definitions: &[OccurrenceId]) -> String {
        let count = definitions.len();
        if count == 0 {
            return format!("**`.{raw_name}`** — no matching rule in the workspace CSS");
        }
        let mut markdown = format!(
            "**`.{raw_name}`** — {count} rule{}",
            if count == 1 { "" } else { "s" }
        );
        for id in definitions.iter().take(MAX_HOVER_RULES) {
            let Some((file, occurrence)) = self.workspace.resolve(*id) else {
                continue;
            };
            let file_name = uri_to_path(&file.uri)
                .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()))
                .unwrap_or_else(|| file.uri.as_str().to_owned());
            let line = file
                .line_index
                .position(&file.text, occurrence.name_span.start as usize)
                .line
                + 1;
            markdown.push_str(&format!("\n\n`{file_name}:{line}`"));
            for context in self.workspace.context_names(file, occurrence) {
                markdown.push_str(&format!(" · `{context}`"));
            }
            let rule = file.text.get(occurrence.rule_span.range()).unwrap_or("").trim();
            let rule = match rule.char_indices().nth(MAX_HOVER_RULE_CHARS) {
                Some((end, _)) => format!("{}…", rule.get(..end).unwrap_or(rule)),
                None => rule.to_owned(),
            };
            markdown.push_str(&format!("\n\n```css\n{rule}\n```"));
        }
        if count > MAX_HOVER_RULES {
            markdown.push_str(&format!("\n\n…and {} more", count - MAX_HOVER_RULES));
        }
        markdown
    }
}

fn range_of(file: &FileEntry, span: Span) -> Range {
    Range::new(
        file.line_index.position(&file.text, span.start as usize),
        file.line_index.position(&file.text, span.end as usize),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catch_panic_returns_the_message() {
        assert_eq!(catch_panic(|| 7), Ok(7));
        assert_eq!(catch_panic(|| -> u32 { panic!("static message") }), Err("static message".to_owned()));
        let formatted = format!("formatted {}", 5);
        assert_eq!(catch_panic(move || -> u32 { panic!("{formatted}") }), Err("formatted 5".to_owned()));
    }

    #[test]
    fn a_panicking_handler_does_not_stop_later_work() {
        let first = catch_panic(|| -> u32 { panic!("boom") });
        let second = catch_panic(|| 2 + 2);
        assert!(first.is_err());
        assert_eq!(second, Ok(4));
    }
}
