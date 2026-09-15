use super::{HirId, ModuleId, SymbolId};

/// A local operation on an existing MIR entity. No captured environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResolveTask {
    Symbol(SymbolId),
    Reference(HirId),
    Namespace(SymbolId),
    Constructor(SymbolId),
    ConstructorNamespace(HirId),
}

/// `None` means not yet queried or waiting, not a negative semantic result.
/// The graph keeps these facts so debug consumers never need to rerun queries.
#[derive(Default, Debug)]
pub struct ResolutionFacts {
    pub namespaces: Vec<Option<Option<ModuleId>>>,
    pub constructors: Vec<Option<bool>>,
    pub constructor_namespaces: Vec<Option<bool>>,
    pub waiting: std::collections::BTreeMap<ResolveTask, ResolveTask>,
}
