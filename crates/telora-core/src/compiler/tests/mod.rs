use super::*;
use crate::{Quota, RuntimeErrorKind};

fn run(source: &str) -> Result<ExecutionWorld, ExecutionError> {
    run_source("test", source, 10_000)
}

include!("part-01.rs");
include!("part-02.rs");
