use std::task::{Context, Poll, Waker};
use std::time::{SystemTime, UNIX_EPOCH};

use super::*;
use telora_core::Quota;

fn config() -> EngineConfig {
    EngineConfig {
        module_quota: Quota::with_fuel(100_000),
        session_quota: Quota::with_fuel(100_000),
        data_limits: telora_core::DataLimits::default(),
    }
}

fn fixture_loop() -> (PathBuf, Rc<RefCell<State>>, async_lsp::MainLoop<Server>) {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("telora-lsp-test-{unique}"));
    std::fs::create_dir_all(&root).expect("create fixture root");
    std::fs::write(root.join("telora-deps.json"), r#"{"name":"fixture"}"#)
        .expect("write fixture manifest");
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
    let workspace =
        Rc::new(Workspace::new(&path, Engine::new(config())).expect("create document workspace"));
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

include!("part-01.rs");
