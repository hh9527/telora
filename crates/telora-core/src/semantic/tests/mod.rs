use super::*;
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
