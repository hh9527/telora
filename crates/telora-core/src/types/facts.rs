//! Diagnostic facts retained by the legacy partial type analyzer.
use crate::hir::HirDefinitionId;

macro_rules! compact_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(u32);

        impl $name {
            pub const fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

compact_id!(DiagnosticId);

impl DiagnosticId {
    pub(crate) const fn from_index(index: usize) -> Self {
        Self(index as u32)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FactIdentity {
    HirDefinition(HirDefinitionId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UnknownReason {
    MissingSyntax,
    InvalidSyntax,
    UnresolvedName,
    BlockedBy(FactIdentity),
    UnavailableDependency,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Conflict {
    DuplicateDefinition,
    IncompatibleContract,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IncomputableReason {
    QuotaExceeded,
    RuntimeOnly,
    UnsupportedOperation,
    CyclicEvaluation,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FactState {
    Known,
    Unknown(UnknownReason),
    Conflicted(Conflict),
    Incomputable(IncomputableReason),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticFact<T> {
    pub value: Option<T>,
    pub state: FactState,
    pub causes: Vec<FactIdentity>,
    pub diagnostics: Vec<DiagnosticId>,
}

impl<T> SemanticFact<T> {
    pub fn known(value: T) -> Self {
        Self {
            value: Some(value),
            state: FactState::Known,
            causes: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    pub fn unknown(reason: UnknownReason) -> Self {
        Self {
            value: None,
            state: FactState::Unknown(reason),
            causes: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    pub fn conflicted(value: Option<T>, conflict: Conflict) -> Self {
        Self {
            value,
            state: FactState::Conflicted(conflict),
            causes: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    pub fn incomputable(value: Option<T>, reason: IncomputableReason) -> Self {
        Self {
            value,
            state: FactState::Incomputable(reason),
            causes: Vec::new(),
            diagnostics: Vec::new(),
        }
    }
}
