//! Deferred test value protocol shared by native constructors and VM consumers.
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
