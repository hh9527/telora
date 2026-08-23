use std::future::Future;
use std::task::{Context, Poll, Waker};
use std::time::{SystemTime, UNIX_EPOCH};

use super::*;
use crate::{EngineConfig, Quota, TextRange};

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

fn fixture(source: &str) -> (PathBuf, PathBuf) {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("telora-workspace-test-{unique}"));
    std::fs::create_dir_all(&directory).unwrap();
    let root = directory.join("main.telora");
    std::fs::write(&root, source).unwrap();
    (directory, root)
}

fn engine() -> Engine {
    Engine::new(EngineConfig {
        module_quota: Quota::with_fuel(1_000_000),
        session_quota: Quota::with_fuel(1_000_000),
        data_limits: crate::DataLimits::default(),
    })
}

include!("part-01.rs");
