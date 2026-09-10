use super::*;

#[test]
fn semantic_interface_consumes_the_solved_graph_without_renumbering() {
    use crate::types::{TypeDescriptor, TypeNode, TypeScheme};
    let mut types = TypeGraph::default();
    types.intern_module_interface(&ModuleInterface {
        concrete_types: BTreeMap::from([
            ("Retained".into(), TypeDescriptor::Bytes),
            ("Element".into(), TypeDescriptor::Int),
            ("Items".into(), TypeDescriptor::Array(Box::new(TypeDescriptor::Int))),
        ]),
        ..Default::default()
    });
    let retained = types.named("Retained").unwrap();
    let int = types.named("Element").unwrap();
    let array = types.named("Items").unwrap();
    let count = types.nodes().len();
    let interface = ModuleInterface {
        exports: BTreeMap::from([("items".into(), TypeScheme {
            parameters: Vec::new(), constraints: Vec::new(),
            body: TypeDescriptor::Array(Box::new(TypeDescriptor::Int)),
        })]),
        ..Default::default()
    };
    let snapshot = SemanticModuleInterface::with_types(&interface, types);
    assert_eq!(snapshot.types.node(retained), &TypeNode::Bytes);
    assert_eq!(snapshot.types.node(array), &TypeNode::Array(int));
    let TypeNode::Struct(fields) = snapshot.types.node(snapshot.result_type) else { panic!("exports") };
    assert_eq!(fields["items"], array);
    assert_eq!(snapshot.types.nodes().len(), count + 1);
}
use crate::{Engine, EngineConfig, Quota, TextRange};
use std::fs;
use std::future::Future;
use std::task::{Context, Poll, Waker};
use std::time::{SystemTime, UNIX_EPOCH};

fn fixture_dir() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("telora-semantic-test-{unique}"));
    fs::create_dir(&path).unwrap();
    path
}

fn engine() -> Engine {
    Engine::new(EngineConfig {
        module_quota: Quota::with_fuel(1_000_000),
        session_quota: Quota::with_fuel(1_000_000),
        data_limits: crate::DataLimits::default(),
    })
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(result) = future.as_mut().poll(&mut context) {
            return result;
        }
    }
}

fn completion_at(snapshot: &WorkspaceSnapshot, needle: &str) -> Option<CompletionResult> {
    let (source, offset) = snapshot
        .sources()
        .files()
        .find_map(|file| {
            file.text()
                .to_string()
                .find(needle)
                .map(|offset| (file.id(), offset + needle.len()))
        })
        .expect("completion text");
    let context = crate::query::QueryContext::current(crate::query::RevisionClock::default());
    block_on(snapshot.query_completion_at(
        &context,
        Location::new(source, TextRange::at(offset as u32)),
    ))
    .expect("completion query")
}

include!("part-01.rs");
