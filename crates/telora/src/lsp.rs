use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use async_lsp::lsp_types as lsp;
use async_lsp::{
    AnyEvent, AnyNotification, AnyRequest, ClientSocket, ErrorCode, LspService, RequestId,
    ResponseError,
};
use lsp::notification::Notification as _;
use lsp::request::Request as _;
use serde::de::DeserializeOwned;
use telora_core::{
    CancellationToken, CompletionKind, DocumentVersion, Engine, EngineConfig, FactState, Location,
    PositionEncoding, QueryError, TextEdit, TextPosition, TextRange, Workspace, WorkspaceSnapshot,
};
use tower_service::Service;

pub fn run_stdio(root: PathBuf, config: EngineConfig) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let local = tokio::task::LocalSet::new();
    runtime.block_on(local.run_until(async move {
        let (main_loop, _) =
            async_lsp::MainLoop::new_server(|client| Server::new(root, config, client));

        #[cfg(unix)]
        let (stdin, stdout) = (
            async_lsp::stdio::PipeStdin::lock_tokio()?,
            async_lsp::stdio::PipeStdout::lock_tokio()?,
        );
        #[cfg(not(unix))]
        let (stdin, stdout) = (
            tokio_util::compat::TokioAsyncReadCompatExt::compat(tokio::io::stdin()),
            tokio_util::compat::TokioAsyncWriteCompatExt::compat_write(tokio::io::stdout()),
        );

        main_loop.run_buffered(stdin, stdout).await
    }))?;
    Ok(())
}

type RequestFuture =
    Pin<Box<dyn Future<Output = Result<serde_json::Value, ResponseError>> + 'static>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Lifecycle {
    Uninitialized,
    AwaitingInitialized,
    Running,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NotificationOutcome {
    Continue,
    CleanExit,
    AbnormalExit,
}

struct State {
    fallback_root: PathBuf,
    engine_config: EngineConfig,
    workspace: Option<Rc<Workspace>>,
    client: ClientSocket,
    lifecycle: Lifecycle,
    encoding: PositionEncoding,
    pending: Rc<RefCell<HashMap<RequestId, CancellationToken>>>,
    documents: HashMap<PathBuf, i32>,
}

pub struct Server {
    state: Rc<RefCell<State>>,
}

impl Server {
    pub fn new(root: PathBuf, engine_config: EngineConfig, client: ClientSocket) -> Self {
        Self {
            state: Rc::new(RefCell::new(State {
                fallback_root: root,
                engine_config,
                workspace: None,
                client,
                lifecycle: Lifecycle::Uninitialized,
                encoding: PositionEncoding::Utf16,
                pending: Rc::new(RefCell::new(HashMap::new())),
                documents: HashMap::new(),
            })),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        cancel_all(&self.state.borrow().pending);
    }
}

impl Service<AnyRequest> for Server {
    type Response = serde_json::Value;
    type Error = ResponseError;
    type Future = RequestFuture;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: AnyRequest) -> Self::Future {
        let state = Rc::clone(&self.state);
        Box::pin(async move { dispatch_request(state, request).await })
    }
}

impl LspService for Server {
    fn notify(&mut self, notification: AnyNotification) -> ControlFlow<async_lsp::Result<()>> {
        match dispatch_notification(Rc::clone(&self.state), notification) {
            Ok(NotificationOutcome::Continue) => ControlFlow::Continue(()),
            Ok(NotificationOutcome::CleanExit) => ControlFlow::Break(Ok(())),
            Ok(NotificationOutcome::AbnormalExit) => ControlFlow::Break(Err(
                async_lsp::Error::Protocol("exit received before shutdown".to_owned()),
            )),
            Err(error) => {
                eprintln!("telora lsp notification error: {error}");
                ControlFlow::Continue(())
            }
        }
    }

    fn emit(&mut self, _: AnyEvent) -> ControlFlow<async_lsp::Result<()>> {
        ControlFlow::Continue(())
    }
}

async fn dispatch_request(
    state: Rc<RefCell<State>>,
    request: AnyRequest,
) -> Result<serde_json::Value, ResponseError> {
    if request.method == lsp::request::Initialize::METHOD {
        return initialize(&state, decode(request.params)?);
    }
    if request.method == lsp::request::Shutdown::METHOD {
        let mut state = state.borrow_mut();
        if state.lifecycle != Lifecycle::Running {
            return Err(protocol_error(
                ErrorCode::INVALID_REQUEST,
                "server is not running",
            ));
        }
        state.lifecycle = Lifecycle::Shutdown;
        cancel_all(&state.pending);
        return encode(());
    }
    if state.borrow().lifecycle != Lifecycle::Running {
        return Err(protocol_error(
            ErrorCode::SERVER_NOT_INITIALIZED,
            "server is not initialized",
        ));
    }

    let token = CancellationToken::default();
    let pending = Rc::clone(&state.borrow().pending);
    if pending.borrow().contains_key(&request.id) {
        return Err(protocol_error(
            ErrorCode::INVALID_REQUEST,
            "duplicate request id",
        ));
    }
    pending
        .borrow_mut()
        .insert(request.id.clone(), token.clone());
    let id = request.id.clone();
    let result = semantic_request(&state, request, token).await;
    pending.borrow_mut().remove(&id);
    result
}

async fn semantic_request(
    state: &Rc<RefCell<State>>,
    request: AnyRequest,
    token: CancellationToken,
) -> Result<serde_json::Value, ResponseError> {
    let (snapshot, context, encoding) = {
        let state = state.borrow();
        let workspace = state.workspace.as_ref().ok_or_else(content_modified)?;
        let snapshot = workspace.published().ok_or_else(content_modified)?;
        (
            snapshot,
            workspace.cancellable_context(token),
            state.encoding,
        )
    };

    if request.method == lsp::request::HoverRequest::METHOD {
        let params: lsp::HoverParams = decode(request.params)?;
        let location =
            request_location(&snapshot, &params.text_document_position_params, encoding)?;
        let definition = definition_at(&snapshot, &context, location).await?;
        let ty = snapshot
            .query_type_at(&context, location)
            .await
            .map_err(query_error)?
            .and_then(|ty| snapshot.types().display(ty));
        let ty = definition
            .and_then(|definition| definition.scheme.clone())
            .or(ty);
        let contents = match (definition, ty) {
            (Some(definition), Some(ty)) => Some(format!("{}: {ty}", definition.name)),
            (Some(definition), None) => Some(format!("{}: unknown", definition.name)),
            (None, Some(ty)) => Some(ty),
            (None, None) => snapshot
                .expression_at(location)
                .map(|expression| fact_state_name(&expression.ty.state).to_owned()),
        };
        let hover_range = snapshot
            .reference_at(location)
            .map(|reference| reference.location)
            .or_else(|| {
                snapshot
                    .definition_at(location)
                    .map(|definition| definition.location)
            })
            .or_else(|| {
                snapshot
                    .expression_at(location)
                    .map(|expression| expression.location)
            });
        let hover = contents.map(|contents| lsp::Hover {
            contents: lsp::HoverContents::Scalar(lsp::MarkedString::String(contents)),
            range: hover_range.and_then(|range| to_lsp_range(&snapshot, range, encoding)),
        });
        return encode(hover);
    }
    if request.method == lsp::request::GotoDefinition::METHOD {
        let params: lsp::GotoDefinitionParams = decode(request.params)?;
        let location =
            request_location(&snapshot, &params.text_document_position_params, encoding)?;
        let target = definition_at(&snapshot, &context, location)
            .await?
            .and_then(|definition| to_location(&snapshot, definition.location, encoding));
        return encode(target.map(lsp::GotoDefinitionResponse::Scalar));
    }
    if request.method == lsp::request::References::METHOD {
        let params: lsp::ReferenceParams = decode(request.params)?;
        let location = request_location(&snapshot, &params.text_document_position, encoding)?;
        let Some(definition) = definition_at(&snapshot, &context, location).await? else {
            return encode(Vec::<lsp::Location>::new());
        };
        let mut locations = snapshot
            .query_references_of(&context, definition.id)
            .await
            .map_err(query_error)?
            .into_iter()
            .filter_map(|reference| to_location(&snapshot, reference.location, encoding))
            .collect::<Vec<_>>();
        if params.context.include_declaration
            && let Some(location) = to_location(&snapshot, definition.location, encoding)
        {
            locations.push(location);
        }
        locations.sort_by(|left, right| {
            left.uri
                .as_str()
                .cmp(right.uri.as_str())
                .then_with(|| left.range.start.line.cmp(&right.range.start.line))
                .then_with(|| left.range.start.character.cmp(&right.range.start.character))
        });
        return encode(locations);
    }
    if request.method == lsp::request::Completion::METHOD {
        let params: lsp::CompletionParams = decode(request.params)?;
        let location = request_location(&snapshot, &params.text_document_position, encoding)?;
        let completion = snapshot
            .query_completion_at(&context, location)
            .await
            .map_err(query_error)?;
        let items = completion.map_or_else(Vec::new, |completion| {
            let replacement = Location::new(location.source, completion.replacement);
            let range = to_lsp_range(&snapshot, replacement, encoding);
            completion
                .candidates
                .into_iter()
                .filter_map(|candidate| {
                    let range = range?;
                    Some(lsp::CompletionItem {
                        label: candidate.label.clone(),
                        kind: Some(match candidate.kind {
                            CompletionKind::ModuleExport => lsp::CompletionItemKind::MODULE,
                            CompletionKind::StructField => lsp::CompletionItemKind::FIELD,
                        }),
                        detail: snapshot.types().display(candidate.ty),
                        sort_text: Some(candidate.label.clone()),
                        text_edit: Some(lsp::CompletionTextEdit::Edit(lsp::TextEdit {
                            range,
                            new_text: candidate.label,
                        })),
                        ..lsp::CompletionItem::default()
                    })
                })
                .collect()
        });
        return encode(Some(lsp::CompletionResponse::List(lsp::CompletionList {
            is_incomplete: false,
            items,
        })));
    }
    Err(protocol_error(
        ErrorCode::METHOD_NOT_FOUND,
        "method not found",
    ))
}

fn initialize(
    state: &Rc<RefCell<State>>,
    params: lsp::InitializeParams,
) -> Result<serde_json::Value, ResponseError> {
    let mut state = state.borrow_mut();
    if state.lifecycle != Lifecycle::Uninitialized {
        return Err(protocol_error(
            ErrorCode::INVALID_REQUEST,
            "already initialized",
        ));
    }
    let root = initialize_root(&params).unwrap_or_else(|| state.fallback_root.clone());
    state.fallback_root = root.clone();
    state.workspace = root
        .is_file()
        .then(|| {
            Workspace::new(root, Engine::new(state.engine_config))
                .map(Rc::new)
                .map_err(|error| protocol_error(ErrorCode::INTERNAL_ERROR, error))
        })
        .transpose()?;
    state.encoding = negotiate_encoding(
        params
            .capabilities
            .general
            .and_then(|general| general.position_encodings),
    );
    state.lifecycle = Lifecycle::AwaitingInitialized;
    encode(lsp::InitializeResult {
        capabilities: lsp::ServerCapabilities {
            position_encoding: Some(lsp_encoding(state.encoding)),
            text_document_sync: Some(lsp::TextDocumentSyncCapability::Options(
                lsp::TextDocumentSyncOptions {
                    open_close: Some(true),
                    change: Some(lsp::TextDocumentSyncKind::INCREMENTAL),
                    ..lsp::TextDocumentSyncOptions::default()
                },
            )),
            hover_provider: Some(lsp::HoverProviderCapability::Simple(true)),
            definition_provider: Some(lsp::OneOf::Left(true)),
            references_provider: Some(lsp::OneOf::Left(true)),
            completion_provider: Some(lsp::CompletionOptions {
                resolve_provider: Some(false),
                trigger_characters: Some(vec![".".to_owned()]),
                ..lsp::CompletionOptions::default()
            }),
            ..lsp::ServerCapabilities::default()
        },
        server_info: Some(lsp::ServerInfo {
            name: "telora".to_owned(),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        }),
    })
}

fn dispatch_notification(
    state: Rc<RefCell<State>>,
    notification: AnyNotification,
) -> Result<NotificationOutcome, String> {
    if notification.method == lsp::notification::Exit::METHOD {
        let clean = state.borrow().lifecycle == Lifecycle::Shutdown;
        cancel_all(&state.borrow().pending);
        return Ok(if clean {
            NotificationOutcome::CleanExit
        } else {
            NotificationOutcome::AbnormalExit
        });
    }
    if notification.method == lsp::notification::Cancel::METHOD {
        let params: lsp::CancelParams =
            serde_json::from_value(notification.params).map_err(|error| error.to_string())?;
        let id = cancel_id(params.id);
        if let Some(token) = state.borrow().pending.borrow_mut().remove(&id) {
            token.cancel();
        }
        return Ok(NotificationOutcome::Continue);
    }
    if notification.method == lsp::notification::Initialized::METHOD {
        let mut state = state.borrow_mut();
        if state.lifecycle != Lifecycle::AwaitingInitialized {
            return Err("initialized notification received out of order".to_owned());
        }
        state.lifecycle = Lifecycle::Running;
        return Ok(NotificationOutcome::Continue);
    }
    if state.borrow().lifecycle != Lifecycle::Running {
        return Err("notification received while server is not running".to_owned());
    }

    if notification.method == lsp::notification::DidOpenTextDocument::METHOD {
        let params: lsp::DidOpenTextDocumentParams =
            serde_json::from_value(notification.params).map_err(|error| error.to_string())?;
        let path = uri_path(&params.text_document.uri)?;
        let mut borrowed = state.borrow_mut();
        if borrowed.documents.contains_key(&path) {
            return Err(format!("document is already open: {}", path.display()));
        }
        if borrowed.workspace.is_none() {
            borrowed.workspace = Some(Rc::new(
                Workspace::new(&path, Engine::new(borrowed.engine_config))
                    .map_err(|error| error.to_string())?,
            ));
        }
        borrowed
            .workspace
            .as_ref()
            .expect("running server has workspace")
            .open(
                &path,
                DocumentVersion(params.text_document.version.into()),
                params.text_document.text,
            )
            .map_err(|error| error.to_string())?;
        borrowed
            .documents
            .insert(path, params.text_document.version);
        drop(borrowed);
        schedule_rebuild(state);
        return Ok(NotificationOutcome::Continue);
    }
    if notification.method == lsp::notification::DidChangeTextDocument::METHOD {
        let params: lsp::DidChangeTextDocumentParams =
            serde_json::from_value(notification.params).map_err(|error| error.to_string())?;
        apply_changes(&state, params)?;
        schedule_rebuild(state);
        return Ok(NotificationOutcome::Continue);
    }
    if notification.method == lsp::notification::DidCloseTextDocument::METHOD {
        let params: lsp::DidCloseTextDocumentParams =
            serde_json::from_value(notification.params).map_err(|error| error.to_string())?;
        let path = uri_path(&params.text_document.uri)?;
        let mut borrowed = state.borrow_mut();
        borrowed
            .workspace
            .as_ref()
            .expect("running server has workspace")
            .close(&path)
            .map_err(|error| error.to_string())?;
        borrowed.documents.remove(&path);
        drop(borrowed);
        schedule_rebuild(state);
    }
    Ok(NotificationOutcome::Continue)
}

fn apply_changes(
    state: &Rc<RefCell<State>>,
    params: lsp::DidChangeTextDocumentParams,
) -> Result<(), String> {
    let path = uri_path(&params.text_document.uri)?;
    let borrowed = state.borrow();
    let workspace = borrowed
        .workspace
        .as_ref()
        .expect("running server has workspace");
    let document = workspace
        .document(&path)
        .map_err(|error| error.to_string())?;
    let mut text = document.text().clone();
    let mut edits = Vec::new();
    for change in params.content_changes {
        let edit = match change.range {
            Some(range) => TextEdit::Replace {
                range: TextRange::new(
                    text.offset(from_position(range.start), borrowed.encoding)
                        .map_err(|error| error.to_string())?,
                    text.offset(from_position(range.end), borrowed.encoding)
                        .map_err(|error| error.to_string())?,
                )
                .map_err(|error| error.to_string())?,
                replacement: change.text,
            },
            None => TextEdit::Full(change.text),
        };
        text = text
            .apply(std::slice::from_ref(&edit))
            .map_err(|error| error.to_string())?;
        edits.push(edit);
    }
    workspace
        .change(
            &path,
            document.version(),
            DocumentVersion(params.text_document.version.into()),
            &edits,
        )
        .map_err(|error| error.to_string())?;
    drop(borrowed);
    state
        .borrow_mut()
        .documents
        .insert(path, params.text_document.version);
    Ok(())
}

fn schedule_rebuild(state: Rc<RefCell<State>>) {
    let workspace = Rc::clone(
        state
            .borrow()
            .workspace
            .as_ref()
            .expect("running server has workspace"),
    );
    tokio::task::spawn_local(async move {
        let context = workspace.context();
        let rebuilt = workspace.rebuild(&context).await;
        let Ok(snapshot) = rebuilt else { return };
        if context.check().is_err() {
            return;
        }
        publish_diagnostics(&state, &snapshot).await;
    });
}

async fn publish_diagnostics(state: &Rc<RefCell<State>>, snapshot: &WorkspaceSnapshot) {
    let (client, encoding, documents) = {
        let state = state.borrow();
        (
            state.client.clone(),
            state.encoding,
            state.documents.clone(),
        )
    };
    for (path, version) in documents {
        let Some(file) = snapshot
            .sources()
            .files()
            .find(|file| Path::new(file.name.as_ref()) == path)
        else {
            continue;
        };
        let Ok(uri) = lsp::Url::from_file_path(&path) else {
            continue;
        };
        let diagnostics = snapshot
            .diagnostics()
            .iter()
            .filter_map(|diagnostic| {
                let primary = diagnostic.labels.iter().find(|label| label.primary)?;
                if primary.location.source != file.id() {
                    return None;
                }
                Some(lsp::Diagnostic {
                    range: to_lsp_range(snapshot, primary.location, encoding)?,
                    severity: Some(match diagnostic.severity {
                        telora_core::source::Severity::Error => lsp::DiagnosticSeverity::ERROR,
                        telora_core::source::Severity::Warning => lsp::DiagnosticSeverity::WARNING,
                        telora_core::source::Severity::Info => lsp::DiagnosticSeverity::INFORMATION,
                    }),
                    message: diagnostic.message.clone(),
                    related_information: Some(
                        diagnostic
                            .labels
                            .iter()
                            .filter(|label| !label.primary)
                            .filter_map(|label| {
                                Some(lsp::DiagnosticRelatedInformation {
                                    location: to_location(snapshot, label.location, encoding)?,
                                    message: label.message.clone(),
                                })
                            })
                            .collect(),
                    ),
                    ..lsp::Diagnostic::default()
                })
            })
            .collect();
        let _ =
            client.notify::<lsp::notification::PublishDiagnostics>(lsp::PublishDiagnosticsParams {
                uri,
                diagnostics,
                version: Some(version),
            });
    }
}

async fn definition_at<'a>(
    snapshot: &'a WorkspaceSnapshot,
    context: &telora_core::QueryContext,
    location: Location,
) -> Result<Option<&'a telora_core::Definition>, ResponseError> {
    if let Some(reference) = snapshot
        .query_reference_at(context, location)
        .await
        .map_err(query_error)?
    {
        return Ok(reference.definition.and_then(|id| snapshot.definition(id)));
    }
    snapshot
        .query_definition_at(context, location)
        .await
        .map_err(query_error)
}

fn request_location(
    snapshot: &WorkspaceSnapshot,
    params: &lsp::TextDocumentPositionParams,
    encoding: PositionEncoding,
) -> Result<Location, ResponseError> {
    let path = uri_path(&params.text_document.uri)
        .map_err(|message| protocol_error(ErrorCode::INVALID_PARAMS, message))?;
    let module = snapshot
        .module_by_path(&path)
        .ok_or_else(content_modified)?;
    let source = module.source.ok_or_else(content_modified)?;
    let offset = snapshot
        .sources()
        .get(source)
        .text()
        .offset(from_position(params.position), encoding)
        .map_err(|error| protocol_error(ErrorCode::INVALID_PARAMS, error))?;
    Ok(Location::new(source, TextRange::at(offset)))
}

fn to_location(
    snapshot: &WorkspaceSnapshot,
    location: Location,
    encoding: PositionEncoding,
) -> Option<lsp::Location> {
    let file = snapshot.sources().get(location.source);
    Some(lsp::Location::new(
        lsp::Url::from_file_path(Path::new(file.name.as_ref())).ok()?,
        to_lsp_range(snapshot, location, encoding)?,
    ))
}

fn to_lsp_range(
    snapshot: &WorkspaceSnapshot,
    location: Location,
    encoding: PositionEncoding,
) -> Option<lsp::Range> {
    let text = snapshot.sources().get(location.source).text();
    Some(lsp::Range::new(
        to_position(text.position(location.start, encoding).ok()?),
        to_position(text.position(location.end, encoding).ok()?),
    ))
}

fn negotiate_encoding(offered: Option<Vec<lsp::PositionEncodingKind>>) -> PositionEncoding {
    let Some(offered) = offered else {
        return PositionEncoding::Utf16;
    };
    [
        (lsp::PositionEncodingKind::UTF8, PositionEncoding::Utf8),
        (lsp::PositionEncodingKind::UTF16, PositionEncoding::Utf16),
        (lsp::PositionEncodingKind::UTF32, PositionEncoding::Utf32),
    ]
    .into_iter()
    .find_map(|(kind, encoding)| offered.contains(&kind).then_some(encoding))
    .unwrap_or(PositionEncoding::Utf16)
}

#[allow(deprecated)]
fn initialize_root(params: &lsp::InitializeParams) -> Option<PathBuf> {
    params
        .workspace_folders
        .as_ref()
        .and_then(|folders| folders.first())
        .and_then(|folder| folder.uri.to_file_path().ok())
        .or_else(|| params.root_uri.as_ref()?.to_file_path().ok())
        .or_else(|| params.root_path.as_ref().map(PathBuf::from))
}

fn lsp_encoding(encoding: PositionEncoding) -> lsp::PositionEncodingKind {
    match encoding {
        PositionEncoding::Utf8 => lsp::PositionEncodingKind::UTF8,
        PositionEncoding::Utf16 => lsp::PositionEncodingKind::UTF16,
        PositionEncoding::Utf32 => lsp::PositionEncodingKind::UTF32,
    }
}

fn from_position(position: lsp::Position) -> TextPosition {
    TextPosition::new(position.line, position.character)
}
fn to_position(position: TextPosition) -> lsp::Position {
    lsp::Position::new(position.line, position.character)
}

fn uri_path(uri: &lsp::Url) -> Result<PathBuf, String> {
    uri.to_file_path()
        .map_err(|()| format!("unsupported document URI: {uri}"))
}

fn cancel_id(id: lsp::NumberOrString) -> RequestId {
    match id {
        lsp::NumberOrString::Number(number) => RequestId::Number(number),
        lsp::NumberOrString::String(string) => RequestId::String(string),
    }
}

fn cancel_all(pending: &Rc<RefCell<HashMap<RequestId, CancellationToken>>>) {
    for (_, token) in pending.borrow_mut().drain() {
        token.cancel();
    }
}

fn decode<T: DeserializeOwned>(value: serde_json::Value) -> Result<T, ResponseError> {
    serde_json::from_value(value).map_err(|error| protocol_error(ErrorCode::INVALID_PARAMS, error))
}

fn encode(value: impl serde::Serialize) -> Result<serde_json::Value, ResponseError> {
    serde_json::to_value(value).map_err(|error| protocol_error(ErrorCode::INTERNAL_ERROR, error))
}

fn query_error(error: QueryError) -> ResponseError {
    match error {
        QueryError::Cancelled => protocol_error(ErrorCode::REQUEST_CANCELLED, error),
        QueryError::StaleRevision { .. } | QueryError::SnapshotRevision { .. } => {
            content_modified()
        }
    }
}

fn content_modified() -> ResponseError {
    protocol_error(ErrorCode::CONTENT_MODIFIED, "workspace content changed")
}

fn protocol_error(code: ErrorCode, message: impl std::fmt::Display) -> ResponseError {
    ResponseError::new(code, message)
}

fn fact_state_name(state: &FactState) -> &'static str {
    match state {
        FactState::Known => "known",
        FactState::Unknown(_) => "unknown",
        FactState::Conflicted(_) => "conflicted",
        FactState::Incomputable(_) => "incomputable",
    }
}

#[cfg(test)]
mod tests {
    use std::task::{Context, Poll, Waker};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use telora_core::Quota;

    fn config() -> EngineConfig {
        EngineConfig {
            module_quota: Quota::with_fuel(100_000),
            session_quota: Quota::with_fuel(100_000),
        }
    }

    fn fixture_loop() -> (PathBuf, Rc<RefCell<State>>, async_lsp::MainLoop<Server>) {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("telora-lsp-test-{unique}"));
        std::fs::create_dir_all(&root).expect("create fixture root");
        let captured = Rc::new(RefCell::new(None));
        let (main_loop, _) = async_lsp::MainLoop::new_server({
            let root = root.clone();
            let captured = Rc::clone(&captured);
            move |client| {
                let server = Server::new(root, config(), client);
                *captured.borrow_mut() = Some(Rc::clone(&server.state));
                server
            }
        });
        let state = captured.borrow_mut().take().expect("captured server state");
        (root, state, main_loop)
    }

    fn fixture() -> (PathBuf, Rc<RefCell<State>>) {
        let (root, state, _) = fixture_loop();
        (root, state)
    }

    fn initialize_state(root: &Path, state: &Rc<RefCell<State>>) -> lsp::InitializeResult {
        let mut params = lsp::InitializeParams::default();
        #[allow(deprecated)]
        {
            params.root_uri = Some(lsp::Url::from_directory_path(root).expect("fixture URI"));
        }
        let value = initialize(state, params).expect("initialize server");
        let initialized: AnyNotification = serde_json::from_value(serde_json::json!({
            "method": "initialized",
            "params": {}
        }))
        .expect("initialized notification");
        assert_eq!(
            dispatch_notification(Rc::clone(state), initialized).expect("finish initialization"),
            NotificationOutcome::Continue
        );
        serde_json::from_value(value).expect("initialize result")
    }

    fn request(id: i64, method: &str, params: serde_json::Value) -> AnyRequest {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "method": method,
            "params": params
        }))
        .expect("request")
    }

    fn frame(message: serde_json::Value) -> Vec<u8> {
        let body = serde_json::to_vec(&message).expect("serialize message");
        let mut framed = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        framed.extend(body);
        framed
    }

    async fn semantic_fixture(source: &str) -> (PathBuf, Rc<RefCell<State>>, lsp::Url) {
        let (root, state) = fixture();
        initialize_state(&root, &state);
        let path = root.join("main.telora");
        let uri = lsp::Url::from_file_path(&path).expect("document URI");
        if state.borrow().workspace.is_none() {
            state.borrow_mut().workspace = Some(Rc::new(
                Workspace::new(&path, Engine::new(config())).expect("create document workspace"),
            ));
        }
        let workspace = Rc::clone(
            state
                .borrow()
                .workspace
                .as_ref()
                .expect("initialized workspace"),
        );
        workspace
            .open(&path, DocumentVersion(1), source)
            .expect("open source");
        state.borrow_mut().documents.insert(path.clone(), 1);
        let context = workspace.context();
        workspace.rebuild(&context).await.expect("build snapshot");
        (path, state, uri)
    }

    async fn disk_semantic_fixture(source: &str) -> (PathBuf, Rc<RefCell<State>>, lsp::Url) {
        let (root, state) = fixture();
        let path = root.join("main.telora");
        std::fs::write(&path, source).expect("write source");
        initialize_state(&root, &state);
        let workspace = Rc::new(
            Workspace::new(&path, Engine::new(config())).expect("create document workspace"),
        );
        let context = workspace.context();
        workspace.rebuild(&context).await.expect("build snapshot");
        state.borrow_mut().workspace = Some(workspace);
        let uri = lsp::Url::from_file_path(&path).expect("document URI");
        (path, state, uri)
    }

    async fn completion_response(
        state: Rc<RefCell<State>>,
        id: i64,
        uri: lsp::Url,
        position: lsp::Position,
    ) -> lsp::CompletionList {
        let response: Option<lsp::CompletionResponse> = serde_json::from_value(
            dispatch_request(
                state,
                request(
                    id,
                    lsp::request::Completion::METHOD,
                    serde_json::json!({
                        "textDocument": { "uri": uri },
                        "position": position
                    }),
                ),
            )
            .await
            .expect("completion response"),
        )
        .expect("completion result");
        match response.expect("completion list") {
            lsp::CompletionResponse::List(list) => list,
            lsp::CompletionResponse::Array(_) => panic!("expected complete list"),
        }
    }

    #[test]
    fn negotiates_supported_position_encodings() {
        assert_eq!(negotiate_encoding(None), PositionEncoding::Utf16);
        assert_eq!(
            negotiate_encoding(Some(vec![lsp::PositionEncodingKind::UTF16])),
            PositionEncoding::Utf16
        );
        assert_eq!(
            negotiate_encoding(Some(vec![
                lsp::PositionEncodingKind::UTF32,
                lsp::PositionEncodingKind::UTF8,
            ])),
            PositionEncoding::Utf8
        );
    }

    #[test]
    fn initialize_uses_client_root_and_advertises_incremental_sync() {
        let (root, state) = fixture();
        let result = initialize_state(&root, &state);
        assert_eq!(state.borrow().lifecycle, Lifecycle::Running);
        assert_eq!(
            result.capabilities.position_encoding,
            Some(lsp::PositionEncodingKind::UTF16)
        );
        assert!(matches!(
            result.capabilities.text_document_sync,
            Some(lsp::TextDocumentSyncCapability::Options(
                lsp::TextDocumentSyncOptions {
                    open_close: Some(true),
                    change: Some(lsp::TextDocumentSyncKind::INCREMENTAL),
                    ..
                }
            ))
        ));
        let completion = result
            .capabilities
            .completion_provider
            .expect("completion capability");
        assert_eq!(completion.resolve_provider, Some(false));
        assert_eq!(completion.trigger_characters, Some(vec![".".to_owned()]));
    }

    #[test]
    fn applies_ordered_utf16_changes_transactionally() {
        let (root, state) = fixture();
        initialize_state(&root, &state);
        let path = root.join("main.telora");
        let uri = lsp::Url::from_file_path(&path).expect("document URI");
        {
            let mut state = state.borrow_mut();
            state.workspace = Some(Rc::new(
                Workspace::new(&path, Engine::new(config())).expect("create document workspace"),
            ));
            state
                .workspace
                .as_ref()
                .expect("workspace")
                .open(&path, DocumentVersion(1), "a😀c")
                .expect("open document");
            state.documents.insert(path.clone(), 1);
        }

        apply_changes(
            &state,
            lsp::DidChangeTextDocumentParams {
                text_document: lsp::VersionedTextDocumentIdentifier { uri, version: 2 },
                content_changes: vec![
                    lsp::TextDocumentContentChangeEvent {
                        range: Some(lsp::Range::new(
                            lsp::Position::new(0, 1),
                            lsp::Position::new(0, 3),
                        )),
                        range_length: Some(2),
                        text: "中".to_owned(),
                    },
                    lsp::TextDocumentContentChangeEvent {
                        range: Some(lsp::Range::new(
                            lsp::Position::new(0, 2),
                            lsp::Position::new(0, 3),
                        )),
                        range_length: Some(1),
                        text: "d".to_owned(),
                    },
                ],
            },
        )
        .expect("apply changes");

        let document = state
            .borrow()
            .workspace
            .as_ref()
            .expect("workspace")
            .document(&path)
            .expect("open document");
        assert_eq!(document.version(), DocumentVersion(2));
        assert_eq!(document.text().to_string(), "a中d");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn notifications_apply_full_changes_and_reject_bad_transactions() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let (root, state) = fixture();
                initialize_state(&root, &state);
                let path = root.join("new.telora");
                let uri = lsp::Url::from_file_path(&path).expect("document URI");
                let open: AnyNotification = serde_json::from_value(serde_json::json!({
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": uri,
                            "languageId": "telora",
                            "version": 1,
                            "text": "a😀c"
                        }
                    }
                }))
                .expect("open notification");
                assert_eq!(
                    dispatch_notification(Rc::clone(&state), open).expect("open document"),
                    NotificationOutcome::Continue
                );

                let full: AnyNotification = serde_json::from_value(serde_json::json!({
                    "method": "textDocument/didChange",
                    "params": {
                        "textDocument": { "uri": uri, "version": 2 },
                        "contentChanges": [{ "text": "valid" }]
                    }
                }))
                .expect("change notification");
                dispatch_notification(Rc::clone(&state), full).expect("full change");
                let workspace = Rc::clone(state.borrow().workspace.as_ref().expect("workspace"));
                let revision = workspace.revision();
                assert_eq!(
                    workspace
                        .document(&path)
                        .expect("document")
                        .text()
                        .to_string(),
                    "valid"
                );

                let out_of_order: AnyNotification = serde_json::from_value(serde_json::json!({
                    "method": "textDocument/didChange",
                    "params": {
                        "textDocument": { "uri": uri, "version": 2 },
                        "contentChanges": [{ "text": "wrong" }]
                    }
                }))
                .expect("change notification");
                assert!(dispatch_notification(Rc::clone(&state), out_of_order).is_err());
                assert_eq!(workspace.revision(), revision);

                let invalid_boundary: AnyNotification = serde_json::from_value(serde_json::json!({
                    "method": "textDocument/didChange",
                    "params": {
                        "textDocument": { "uri": uri, "version": 3 },
                        "contentChanges": [{
                            "range": {
                                "start": { "line": 0, "character": 99 },
                                "end": { "line": 0, "character": 99 }
                            },
                            "text": "wrong"
                        }]
                    }
                }))
                .expect("change notification");
                assert!(dispatch_notification(Rc::clone(&state), invalid_boundary).is_err());
                assert_eq!(workspace.revision(), revision);
                assert_eq!(
                    workspace
                        .document(&path)
                        .expect("document")
                        .text()
                        .to_string(),
                    "valid"
                );

                let close: AnyNotification = serde_json::from_value(serde_json::json!({
                    "method": "textDocument/didClose",
                    "params": { "textDocument": { "uri": uri } }
                }))
                .expect("close notification");
                dispatch_notification(Rc::clone(&state), close).expect("close document");
                assert!(workspace.document(path).is_err());
                assert!(state.borrow().documents.is_empty());
            })
            .await;
    }

    #[test]
    fn cancel_notification_sets_registered_telora_token() {
        let (_, state) = fixture();
        let token = CancellationToken::default();
        state
            .borrow()
            .pending
            .borrow_mut()
            .insert(RequestId::Number(7), token.clone());
        let notification: AnyNotification = serde_json::from_value(serde_json::json!({
            "method": "$/cancelRequest",
            "params": { "id": 7 }
        }))
        .expect("cancel notification");

        assert_eq!(
            dispatch_notification(state, notification).expect("dispatch cancellation"),
            NotificationOutcome::Continue
        );
        assert!(token.is_cancelled());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cancellation_reaches_a_pending_query_checkpoint() {
        let (_, state, uri) = semantic_fixture("let value = 1;\nvalue").await;
        let mut future = Box::pin(dispatch_request(
            Rc::clone(&state),
            request(
                41,
                lsp::request::HoverRequest::METHOD,
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": 1, "character": 1 }
                }),
            ),
        ));
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
        assert!(
            state
                .borrow()
                .pending
                .borrow()
                .contains_key(&RequestId::Number(41))
        );

        let cancel: AnyNotification = serde_json::from_value(serde_json::json!({
            "method": "$/cancelRequest",
            "params": { "id": 41 }
        }))
        .expect("cancel notification");
        assert_eq!(
            dispatch_notification(Rc::clone(&state), cancel).expect("cancel request"),
            NotificationOutcome::Continue
        );
        let error = future.await.expect_err("cancelled response");
        assert_eq!(error.code, ErrorCode::REQUEST_CANCELLED);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn edit_makes_a_pending_query_content_modified() {
        let (path, state, uri) = semantic_fixture("let value = 1;\nvalue").await;
        let mut future = Box::pin(dispatch_request(
            Rc::clone(&state),
            request(
                42,
                lsp::request::HoverRequest::METHOD,
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": 1, "character": 1 }
                }),
            ),
        ));
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));

        let workspace = Rc::clone(state.borrow().workspace.as_ref().expect("workspace"));
        workspace
            .change(
                &path,
                DocumentVersion(1),
                DocumentVersion(2),
                &[TextEdit::Full("let value = 2;\nvalue".to_owned())],
            )
            .expect("advance revision");
        let error = future.await.expect_err("stale response");
        assert_eq!(error.code, ErrorCode::CONTENT_MODIFIED);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn duplicate_ids_and_shutdown_with_pending_work_are_deterministic() {
        let (root, state) = fixture();
        initialize_state(&root, &state);
        let original = CancellationToken::default();
        state
            .borrow()
            .pending
            .borrow_mut()
            .insert(RequestId::Number(9), original.clone());
        let duplicate = dispatch_request(
            Rc::clone(&state),
            request(9, lsp::request::HoverRequest::METHOD, serde_json::json!({})),
        )
        .await
        .expect_err("duplicate request ID");
        assert_eq!(duplicate.code, ErrorCode::INVALID_REQUEST);
        assert!(!original.is_cancelled());

        dispatch_request(
            Rc::clone(&state),
            request(10, lsp::request::Shutdown::METHOD, serde_json::Value::Null),
        )
        .await
        .expect("shutdown response");
        assert!(original.is_cancelled());
        assert!(state.borrow().pending.borrow().is_empty());

        let unknown_cancel: AnyNotification = serde_json::from_value(serde_json::json!({
            "method": "$/cancelRequest",
            "params": { "id": "missing" }
        }))
        .expect("cancel notification");
        assert_eq!(
            dispatch_notification(state, unknown_cancel).expect("ignore unknown ID"),
            NotificationOutcome::Continue
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hover_definition_and_references_use_the_published_snapshot() {
        let (_, state, uri) = semantic_fixture("let value = 1;\nvalue").await;
        let position = serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": 1, "character": 1 }
        });
        let hover: Option<lsp::Hover> = serde_json::from_value(
            dispatch_request(
                Rc::clone(&state),
                request(50, lsp::request::HoverRequest::METHOD, position.clone()),
            )
            .await
            .expect("hover response"),
        )
        .expect("hover result");
        let hover = hover.expect("hover information");
        assert!(matches!(
            hover.contents,
            lsp::HoverContents::Scalar(lsp::MarkedString::String(ref text))
                if text.contains("value")
        ));
        assert_eq!(hover.range.expect("hover range").start.line, 1);

        let definition: Option<lsp::GotoDefinitionResponse> = serde_json::from_value(
            dispatch_request(
                Rc::clone(&state),
                request(51, lsp::request::GotoDefinition::METHOD, position.clone()),
            )
            .await
            .expect("definition response"),
        )
        .expect("definition result");
        let Some(lsp::GotoDefinitionResponse::Scalar(definition)) = definition else {
            panic!("expected scalar definition");
        };
        assert_eq!(definition.range.start.line, 0);

        let references: Vec<lsp::Location> = serde_json::from_value(
            dispatch_request(
                state,
                request(
                    52,
                    lsp::request::References::METHOD,
                    serde_json::json!({
                        "textDocument": position["textDocument"].clone(),
                        "position": position["position"].clone(),
                        "context": { "includeDeclaration": true }
                    }),
                ),
            )
            .await
            .expect("references response"),
        )
        .expect("references result");
        assert_eq!(references.len(), 2);
        assert_eq!(references[0].range.start.line, 0);
        assert_eq!(references[1].range.start.line, 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hover_reports_an_ordinary_expression_type() {
        let (_, state, uri) =
            semantic_fixture("let count = 1 + 2;\nexport { count as output };").await;
        let hover: Option<lsp::Hover> = serde_json::from_value(
            dispatch_request(
                state,
                request(
                    53,
                    lsp::request::HoverRequest::METHOD,
                    serde_json::json!({
                        "textDocument": { "uri": uri },
                        "position": { "line": 0, "character": 12 }
                    }),
                ),
            )
            .await
            .expect("hover response"),
        )
        .expect("hover result");
        assert!(matches!(
            hover.expect("expression hover").contents,
            lsp::HoverContents::Scalar(lsp::MarkedString::String(ref text)) if text == "Int"
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hover_reports_inferred_local_type_schemes() {
        let (_, state, uri) =
            semantic_fixture(
                "let identity = fn(value) { value };\nlet result = identity(1); export { result as output };",
            )
            .await;
        let hover: Option<lsp::Hover> = serde_json::from_value(
            dispatch_request(
                state,
                request(
                    54,
                    lsp::request::HoverRequest::METHOD,
                    serde_json::json!({
                        "textDocument": { "uri": uri },
                        "position": { "line": 1, "character": 15 }
                    }),
                ),
            )
            .await
            .expect("hover response"),
        )
        .expect("hover result");
        assert!(matches!(
            hover.expect("scheme hover").contents,
            lsp::HoverContents::Scalar(lsp::MarkedString::String(ref text))
                if text == "identity: for(A) Fn(A) -> A"
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn protocol_encodings_map_unicode_and_crlf_to_the_same_bytes() {
        let (_, state, uri) = semantic_fixture("\"😀\";\r\n1").await;
        let snapshot = state
            .borrow()
            .workspace
            .as_ref()
            .expect("workspace")
            .published()
            .expect("snapshot");
        let position = |line, character| lsp::TextDocumentPositionParams {
            text_document: lsp::TextDocumentIdentifier { uri: uri.clone() },
            position: lsp::Position::new(line, character),
        };
        assert_eq!(
            request_location(&snapshot, &position(0, 5), PositionEncoding::Utf8)
                .expect("UTF-8 position")
                .start,
            5
        );
        assert_eq!(
            request_location(&snapshot, &position(0, 3), PositionEncoding::Utf16)
                .expect("UTF-16 position")
                .start,
            5
        );
        assert_eq!(
            request_location(&snapshot, &position(0, 2), PositionEncoding::Utf32)
                .expect("UTF-32 position")
                .start,
            5
        );
        for encoding in [
            PositionEncoding::Utf8,
            PositionEncoding::Utf16,
            PositionEncoding::Utf32,
        ] {
            assert_eq!(
                request_location(&snapshot, &position(1, 0), encoding)
                    .expect("position after CRLF")
                    .start,
                9
            );
        }
        assert!(request_location(&snapshot, &position(0, 2), PositionEncoding::Utf16).is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn lsp_completion_maps_struct_fields_and_utf16_text_edits() {
        let source =
            "let face = \"😀\"; let value = {alpha: 1, beta: \"x\"}; let selected = value.alpha";
        let module_source = format!("{source}; export {{ selected as output }};");
        let (_, state, uri) = disk_semantic_fixture(&module_source).await;
        let document = state
            .borrow()
            .workspace
            .as_ref()
            .expect("workspace")
            .published()
            .expect("snapshot")
            .sources()
            .files()
            .find(|file| file.name.ends_with("main.telora"))
            .expect("main source")
            .text()
            .clone();
        let cursor = document
            .position(source.len() as u32, PositionEncoding::Utf16)
            .expect("UTF-16 cursor");
        let list = completion_response(
            state,
            80,
            uri,
            lsp::Position::new(cursor.line, cursor.character),
        )
        .await;
        assert!(!list.is_incomplete);
        assert_eq!(list.items.len(), 1);
        let item = &list.items[0];
        assert_eq!(item.label, "alpha");
        assert_eq!(item.kind, Some(lsp::CompletionItemKind::FIELD));
        assert_eq!(item.detail.as_deref(), Some("Int"));
        let Some(lsp::CompletionTextEdit::Edit(edit)) = &item.text_edit else {
            panic!("expected completion text edit");
        };
        assert_eq!(edit.new_text, "alpha");
        assert_eq!(
            edit.range.end,
            lsp::Position::new(cursor.line, cursor.character)
        );
        assert_eq!(edit.range.end.character - edit.range.start.character, 5);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn lsp_completion_maps_module_exports() {
        let (root, state) = fixture();
        let model = root.join("model.telora");
        let main = root.join("main.telora");
        std::fs::write(&model, "export def alpha = 1; export def beta = \"x\";")
            .expect("write model");
        let source = "import \"./model.telora\" as model; model.alpha";
        std::fs::write(&main, format!("{source}; export {{ model as output }};"))
            .expect("write main");
        initialize_state(&root, &state);
        let workspace = Rc::new(
            Workspace::new(&main, Engine::new(config())).expect("create document workspace"),
        );
        let context = workspace.context();
        workspace.rebuild(&context).await.expect("build snapshot");
        state.borrow_mut().workspace = Some(workspace);
        let uri = lsp::Url::from_file_path(main).expect("main URI");
        let list =
            completion_response(state, 81, uri, lsp::Position::new(0, source.len() as u32)).await;
        assert_eq!(list.items.len(), 1);
        assert_eq!(list.items[0].label, "alpha");
        assert_eq!(list.items[0].kind, Some(lsp::CompletionItemKind::MODULE));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn lsp_completion_supports_an_empty_prefix_in_recovered_source() {
        let (root, state) = fixture();
        let model = root.join("model.telora");
        let main = root.join("main.telora");
        std::fs::write(&model, "export def alpha = 1; export def beta = \"x\";")
            .expect("write model");
        let source = "import \"./model.telora\" as model; model.";
        std::fs::write(&main, format!("{source}\nexport {{ model as output }};"))
            .expect("write main");
        initialize_state(&root, &state);
        let workspace = Rc::new(
            Workspace::new(&main, Engine::new(config())).expect("create document workspace"),
        );
        let context = workspace.context();
        workspace.rebuild(&context).await.expect("build snapshot");
        state.borrow_mut().workspace = Some(workspace);
        let uri = lsp::Url::from_file_path(main).expect("main URI");
        let list =
            completion_response(state, 82, uri, lsp::Position::new(0, source.len() as u32)).await;
        assert_eq!(
            list.items
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
        for item in list.items {
            let Some(lsp::CompletionTextEdit::Edit(edit)) = item.text_edit else {
                panic!("expected completion text edit");
            };
            assert_eq!(edit.range.start, edit.range.end);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn completion_observes_cancellation_and_stale_revisions() {
        let source = "let value = {alpha: 1, beta: 2}; value.alpha";
        let (path, state, uri) = disk_semantic_fixture(source).await;
        let completion_request = || {
            request(
                82,
                lsp::request::Completion::METHOD,
                serde_json::json!({
                    "textDocument": { "uri": uri.clone() },
                    "position": { "line": 0, "character": source.len() }
                }),
            )
        };
        let mut cancelled = Box::pin(dispatch_request(Rc::clone(&state), completion_request()));
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        assert!(matches!(
            cancelled.as_mut().poll(&mut context),
            Poll::Pending
        ));
        let cancel: AnyNotification = serde_json::from_value(serde_json::json!({
            "method": "$/cancelRequest",
            "params": { "id": 82 }
        }))
        .expect("cancel notification");
        dispatch_notification(Rc::clone(&state), cancel).expect("cancel completion");
        assert_eq!(
            cancelled.await.expect_err("cancelled completion").code,
            ErrorCode::REQUEST_CANCELLED
        );

        let mut stale = Box::pin(dispatch_request(Rc::clone(&state), completion_request()));
        assert!(matches!(stale.as_mut().poll(&mut context), Poll::Pending));
        state
            .borrow()
            .workspace
            .as_ref()
            .expect("workspace")
            .open(
                path,
                DocumentVersion(1),
                "let value = {alpha: 2}; value.alpha",
            )
            .expect("advance workspace revision");
        assert_eq!(
            stale.await.expect_err("stale completion").code,
            ErrorCode::CONTENT_MODIFIED
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn transport_eof_cancels_pending_tokens() {
        let (_, state, main_loop) = fixture_loop();
        let token = CancellationToken::default();
        state
            .borrow()
            .pending
            .borrow_mut()
            .insert(RequestId::Number(61), token.clone());
        let mut output = futures::io::Cursor::new(Vec::new());
        let error = main_loop
            .run_buffered(futures::io::Cursor::new(Vec::new()), &mut output)
            .await
            .expect_err("EOF terminates transport");
        assert!(matches!(error, async_lsp::Error::Eof));
        assert!(token.is_cancelled());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn exit_before_shutdown_is_a_protocol_error() {
        let (root, _, main_loop) = fixture_loop();
        let _ = root;
        let input = frame(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "exit"
        }));
        let mut output = futures::io::Cursor::new(Vec::new());
        let error = main_loop
            .run_buffered(futures::io::Cursor::new(input), &mut output)
            .await
            .expect_err("early exit is abnormal");
        assert!(matches!(error, async_lsp::Error::Protocol(_)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn diagnostics_publish_current_errors_and_an_empty_clear() {
        let (root, state, main_loop) = fixture_loop();
        initialize_state(&root, &state);
        let path = root.join("main.telora");
        let workspace = Rc::new(
            Workspace::new(&path, Engine::new(config())).expect("create document workspace"),
        );
        workspace
            .open(
                &path,
                DocumentVersion(1),
                "def first = 1 / 0; def second = 2 / 0; export def output = 0;",
            )
            .expect("open invalid source");
        {
            let mut state = state.borrow_mut();
            state.workspace = Some(Rc::clone(&workspace));
            state.documents.insert(path.clone(), 1);
        }
        let context = workspace.context();
        let invalid = workspace.rebuild(&context).await.expect("invalid snapshot");
        assert_eq!(
            invalid
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.message.contains("division by zero"))
                .count(),
            2
        );
        publish_diagnostics(&state, &invalid).await;

        workspace
            .change(
                &path,
                DocumentVersion(1),
                DocumentVersion(2),
                &[TextEdit::Full("export def output = 1;".to_owned())],
            )
            .expect("fix source");
        state.borrow_mut().documents.insert(path, 2);
        let context = workspace.context();
        let valid = workspace.rebuild(&context).await.expect("valid snapshot");
        assert!(valid.diagnostics().is_empty());
        publish_diagnostics(&state, &valid).await;

        let mut input = Vec::new();
        input.extend(frame(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 70,
            "method": "shutdown"
        })));
        input.extend(frame(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "exit"
        })));
        let mut output = futures::io::Cursor::new(Vec::new());
        main_loop
            .run_buffered(futures::io::Cursor::new(input), &mut output)
            .await
            .expect("flush diagnostics");
        let output = String::from_utf8(output.into_inner()).expect("UTF-8 output");
        assert_eq!(output.matches("textDocument/publishDiagnostics").count(), 2);
        assert!(output.contains("\"version\":1"));
        assert!(output.contains("\"version\":2"));
        assert!(output.contains("\"diagnostics\":[]"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn main_loop_completes_initialize_shutdown_and_exit() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("telora-lsp-loop-test-{unique}"));
        std::fs::create_dir_all(&root).expect("create fixture root");
        let uri = lsp::Url::from_directory_path(&root).expect("fixture URI");
        let mut input = Vec::new();
        input.extend(frame(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "capabilities": {}, "rootUri": uri }
        })));
        input.extend(frame(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "initialized",
            "params": {}
        })));
        input.extend(frame(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "shutdown"
        })));
        input.extend(frame(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "exit"
        })));

        let (main_loop, _) =
            async_lsp::MainLoop::new_server(move |client| Server::new(root, config(), client));
        let mut output = futures::io::Cursor::new(Vec::new());
        main_loop
            .run_buffered(futures::io::Cursor::new(input), &mut output)
            .await
            .expect("run protocol lifecycle");
        let output = String::from_utf8(output.into_inner()).expect("UTF-8 protocol output");
        assert!(output.contains("\"id\":1"));
        assert!(output.contains("\"positionEncoding\":\"utf-16\""));
        assert!(output.contains("\"id\":2"));
        assert!(output.contains("\"result\":null"));
    }
}
