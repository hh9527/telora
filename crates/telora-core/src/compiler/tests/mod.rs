use super::*;
use crate::{Quota, RuntimeErrorKind};

fn run(source: &str) -> Result<ExecutionWorld, ExecutionError> {
    run_source("test", source, 10_000)
}

fn assert_int(value: &ExecutionWorld, expected: i64) {
    assert_eq!(value.value().as_int(), Some(expected));
}

fn assert_atom(value: &ExecutionWorld, expected: &str) {
    assert_eq!(value.value().as_atom().as_deref(), Some(expected));
}

include!("part-01.rs");
include!("part-02.rs");
