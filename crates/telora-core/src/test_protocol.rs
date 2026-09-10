//! Deferred test value protocol shared by native constructors and VM consumers.
use crate::SystemDataFormat;
use std::path::Path;

/// Finite bounds on deferred expansion and host-retained fixture data.
#[derive(Clone, Copy, Debug)]
pub struct TestLimits {
    pub cases: usize,
    pub depth: usize,
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

#[derive(Clone, Debug)]
pub(crate) struct TestDescription {
    pub(crate) kind: TestKind,
    pub(crate) expected: Option<String>,
    pub(crate) sources: Vec<String>,
    pub(crate) origin: Option<crate::Loc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TestKind {
    ShouldOk,
    ShouldFail,
    ShouldFailWith,
    Fixtures,
}

pub(crate) const TEST_NATIVE_TYPE: crate::value::NativeTypeId = crate::value::NativeTypeId {
    module: crate::value::NativeModuleId(crate::mir::NativeTypeId::TEST.module),
    local: crate::mir::NativeTypeId::TEST.slot,
};
