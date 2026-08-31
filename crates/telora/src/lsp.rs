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
    CONFIG_FILE, CancellationToken, CompletionKind, DocumentVersion, Engine, EngineConfig,
    FactState, Location, PositionEncoding, QueryError, TextEdit, TextPosition, TextRange,
    Workspace, WorkspaceSnapshot,
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
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "schema": "telora.lsp/v1",
                        "record": "error",
                        "message": error.to_string(),
                    })
                );
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
            lsp_workspace(&root, state.engine_config)
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
                lsp_workspace(&path, borrowed.engine_config).map_err(|error| error.to_string())?,
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

fn lsp_workspace(path: &Path, config: EngineConfig) -> Result<Workspace, String> {
    let configured = path
        .parent()
        .unwrap_or(path)
        .ancestors()
        .any(|directory| directory.join(CONFIG_FILE).is_file());
    if configured {
        let packages = crate::package_host::prepare(path)?;
        Workspace::new_in_workspace(path, Engine::new(config), packages)
            .map_err(|error| error.to_string())
    } else {
        Workspace::new(path, Engine::new(config)).map_err(|error| error.to_string())
    }
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
        let Some(source) = snapshot
            .module_by_path(&path)
            .and_then(|module| module.source)
        else {
            continue;
        };
        let file = snapshot.sources().get(source);
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
    let path = snapshot
        .module_by_source(location.source)?
        .path
        .as_deref()?;
    Some(lsp::Location::new(
        lsp::Url::from_file_path(path).ok()?,
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
#[path = "lsp/tests/mod.rs"]
mod tests;
