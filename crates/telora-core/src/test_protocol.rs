//! Deferred test value protocol shared by the Wasm runtime and fixture Host.
use crate::SystemDataFormat;
use std::path::Path;

/// Coarse stopping bounds on deferred expansion and cumulative fixture input.
#[derive(Clone, Copy, Debug)]
pub struct TestLimits {
    pub cases: usize,
    pub depth: usize,
    /// Cumulative source bytes admitted, not a measurement of heap allocation.
    pub fixture_bytes: usize,
}

impl Default for TestLimits {
    fn default() -> Self {
        Self {
            cases: 10_000,
            depth: 64,
            fixture_bytes: 256 * 1024 * 1024,
        }
    }
}

/// A private host key, never a module identity or diagnostic label.
#[derive(Clone, Debug)]
pub struct TestSource {
    pub key: String,
    pub format: SystemDataFormat,
}

pub trait TestHost {
    fn resolve(
        &mut self,
        declaring_module: &str,
        declaring_path: Option<&Path>,
        source: &str,
    ) -> Result<TestSource, String>;
    fn read(&mut self, source: &TestSource, max_bytes: usize) -> Result<String, String>;
}

#[derive(Default)]
pub struct TestContext<'a> {
    pub host: Option<&'a mut dyn TestHost>,
    pub limits: TestLimits,
    pub module_paths: std::collections::HashMap<String, std::path::PathBuf>,
}
