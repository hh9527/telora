#![cfg_attr(not(test), allow(dead_code))]

use crate::Location;
use std::collections::HashMap;
use std::hash::Hash;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct FailureId(u32);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum FailureClass {
    Recoverable,
    Terminal,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum FailureOperation {
    Unary,
    Binary,
    Field,
    Index,
    Call,
    NativeCall,
    Condition,
    Match,
    Array,
    Tuple,
    Tagged,
    Dict,
    Interpolation,
    Binding,
    ModuleResult,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct EvaluationUnitId(u32);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum EvaluationUnitKind {
    Binding,
    DefinitionGroup,
    ContainerChild,
    ModuleResult,
    Metadata,
}

impl EvaluationUnitKind {
    const fn failure_operation(self) -> FailureOperation {
        match self {
            Self::Binding | Self::DefinitionGroup | Self::Metadata => FailureOperation::Binding,
            Self::ContainerChild => FailureOperation::Other,
            Self::ModuleResult => FailureOperation::ModuleResult,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EvaluationUnit {
    pub(crate) id: EvaluationUnitId,
    pub(crate) kind: EvaluationUnitKind,
    pub(crate) location: Location,
    pub(crate) dependencies: Box<[EvaluationUnitId]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EvaluationPlan {
    units: Box<[EvaluationUnit]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum EvaluationPlanError {
    Empty,
    NonSequentialId,
    DependencyNotPrior,
    DuplicateDependency,
    DependencyNotOrdered,
    MissingModuleResult,
    MultipleModuleResults,
    ModuleResultNotLast,
}

impl EvaluationPlan {
    pub(crate) fn new(units: Vec<EvaluationUnit>) -> Result<Self, EvaluationPlanError> {
        if units.is_empty() {
            return Err(EvaluationPlanError::Empty);
        }
        let mut module_result = None;
        for (index, unit) in units.iter().enumerate() {
            if unit.id.0 as usize != index {
                return Err(EvaluationPlanError::NonSequentialId);
            }
            let mut previous = None;
            for dependency in &unit.dependencies {
                if dependency.0 as usize >= index {
                    return Err(EvaluationPlanError::DependencyNotPrior);
                }
                if let Some(previous) = previous {
                    if previous == *dependency {
                        return Err(EvaluationPlanError::DuplicateDependency);
                    }
                    if previous > *dependency {
                        return Err(EvaluationPlanError::DependencyNotOrdered);
                    }
                }
                previous = Some(*dependency);
            }
            if unit.kind == EvaluationUnitKind::ModuleResult
                && module_result.replace(index).is_some()
            {
                return Err(EvaluationPlanError::MultipleModuleResults);
            }
        }
        let Some(module_result) = module_result else {
            return Err(EvaluationPlanError::MissingModuleResult);
        };
        if module_result + 1 != units.len() {
            return Err(EvaluationPlanError::ModuleResultNotLast);
        }
        Ok(Self {
            units: units.into_boxed_slice(),
        })
    }

    pub(crate) fn units(&self) -> &[EvaluationUnit] {
        &self.units
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum EvaluationPolicy {
    Strict,
    BestEffort,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UnitFailure<R> {
    pub(crate) class: FailureClass,
    pub(crate) failure: R,
}

impl<R> UnitFailure<R> {
    pub(crate) const fn new(class: FailureClass, failure: R) -> Self {
        Self { class, failure }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum EvaluationUnitState<V> {
    Pending,
    Value(V),
    Never(FailureId),
}

#[derive(Debug)]
pub(crate) struct BestEffortSession<V, R> {
    states: Vec<EvaluationUnitState<V>>,
    root_failures: Vec<FailureId>,
    arena: FailureArena<R>,
    root_budget_exhausted: bool,
}

impl<V, R> BestEffortSession<V, R> {
    pub(crate) fn run(
        plan: &EvaluationPlan,
        limits: FailureLimits,
        max_root_failures: usize,
        mut checkpoint: impl FnMut() -> Result<(), R>,
        mut execute: impl FnMut(&EvaluationUnit, &[&V]) -> Result<V, UnitFailure<R>>,
    ) -> Result<Self, R> {
        let mut session = Self {
            states: Vec::with_capacity(plan.units.len()),
            root_failures: Vec::new(),
            arena: FailureArena::new(limits),
            root_budget_exhausted: false,
        };
        for unit in plan.units() {
            checkpoint()?;
            let mut dependency_values = Vec::with_capacity(unit.dependencies.len());
            let mut failed_dependencies = Vec::new();
            for dependency in &unit.dependencies {
                match &session.states[dependency.0 as usize] {
                    EvaluationUnitState::Value(value) => dependency_values.push(value),
                    EvaluationUnitState::Never(failure) => {
                        failed_dependencies.push(*failure);
                    }
                    EvaluationUnitState::Pending => {
                        unreachable!("validated plans only depend on completed prior units")
                    }
                }
            }
            if !failed_dependencies.is_empty() {
                let failure = session.arena.propagate_causes(
                    unit.kind.failure_operation(),
                    Some(unit.location),
                    failed_dependencies,
                );
                session.states.push(EvaluationUnitState::Never(failure));
                continue;
            }
            if session.root_failures.len() >= max_root_failures {
                session.root_budget_exhausted = true;
                session
                    .states
                    .resize_with(plan.units.len(), || EvaluationUnitState::Pending);
                break;
            }
            match execute(unit, &dependency_values) {
                Ok(value) => session.states.push(EvaluationUnitState::Value(value)),
                Err(UnitFailure { class, failure }) => {
                    let outcome = session.arena.root(class, failure)?;
                    let id = outcome.failure().expect("recoverable root returns Never");
                    session.root_failures.push(id);
                    session.states.push(EvaluationUnitState::Never(id));
                }
            }
        }
        checkpoint()?;
        Ok(session)
    }

    pub(crate) fn states(&self) -> &[EvaluationUnitState<V>] {
        &self.states
    }

    pub(crate) fn root_failures(&self) -> &[FailureId] {
        &self.root_failures
    }

    pub(crate) const fn root_budget_exhausted(&self) -> bool {
        self.root_budget_exhausted
    }

    pub(crate) fn output(&self) -> Option<&V> {
        if !self.root_failures.is_empty() || self.root_budget_exhausted {
            return None;
        }
        match self.states.last()? {
            EvaluationUnitState::Value(value) => Some(value),
            EvaluationUnitState::Pending | EvaluationUnitState::Never(_) => None,
        }
    }

    pub(crate) fn arena(&self) -> &FailureArena<R> {
        &self.arena
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FailureLimits {
    pub(crate) max_propagated_nodes: usize,
    pub(crate) max_causes_per_node: usize,
    pub(crate) max_render_depth: usize,
}

impl FailureLimits {
    pub(crate) const fn new(
        max_propagated_nodes: usize,
        max_causes_per_node: usize,
        max_render_depth: usize,
    ) -> Self {
        Self {
            max_propagated_nodes,
            max_causes_per_node,
            max_render_depth,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum EvalOutcome<T> {
    Value(T),
    Never(FailureId),
}

impl<T> EvalOutcome<T> {
    pub(crate) fn map<U>(self, map: impl FnOnce(T) -> U) -> EvalOutcome<U> {
        match self {
            Self::Value(value) => EvalOutcome::Value(map(value)),
            Self::Never(failure) => EvalOutcome::Never(failure),
        }
    }

    pub(crate) const fn failure(&self) -> Option<FailureId> {
        match self {
            Self::Value(_) => None,
            Self::Never(failure) => Some(*failure),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum FailureNode<R> {
    Root {
        failure: R,
    },
    Propagated {
        operation: FailureOperation,
        location: Option<Location>,
        causes: Box<[FailureId]>,
    },
    Truncated {
        causes: Box<[FailureId]>,
    },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct PropagationKey {
    operation: FailureOperation,
    location: Option<Location>,
    causes: Box<[FailureId]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LineageStep<'a, R> {
    Root(&'a R),
    Propagated {
        operation: FailureOperation,
        location: Option<Location>,
    },
    Truncated,
}

#[derive(Debug)]
pub(crate) struct FailureArena<R> {
    nodes: Vec<FailureNode<R>>,
    propagated: HashMap<PropagationKey, FailureId>,
    propagated_count: usize,
    truncated: Option<FailureId>,
    limits: FailureLimits,
}

impl<R> FailureArena<R> {
    pub(crate) fn new(limits: FailureLimits) -> Self {
        Self {
            nodes: Vec::new(),
            propagated: HashMap::new(),
            propagated_count: 0,
            truncated: None,
            limits,
        }
    }

    pub(crate) fn root(&mut self, class: FailureClass, failure: R) -> Result<EvalOutcome<()>, R> {
        if class == FailureClass::Terminal {
            return Err(failure);
        }
        let id = self.push(FailureNode::Root { failure });
        Ok(EvalOutcome::Never(id))
    }

    pub(crate) fn propagate<T>(
        &mut self,
        operation: FailureOperation,
        location: Option<Location>,
        inputs: &[EvalOutcome<T>],
    ) -> Option<EvalOutcome<()>> {
        let causes = inputs
            .iter()
            .filter_map(EvalOutcome::failure)
            .collect::<Vec<_>>();
        (!causes.is_empty())
            .then(|| EvalOutcome::Never(self.propagate_causes(operation, location, causes)))
    }

    pub(crate) fn propagate_causes(
        &mut self,
        operation: FailureOperation,
        location: Option<Location>,
        causes: impl IntoIterator<Item = FailureId>,
    ) -> FailureId {
        let causes = normalize_causes(causes, self.limits.max_causes_per_node);
        assert!(!causes.is_empty(), "propagation requires a Never cause");
        let key = PropagationKey {
            operation,
            location,
            causes: causes.clone().into_boxed_slice(),
        };
        if let Some(id) = self.propagated.get(&key) {
            return *id;
        }
        if self.propagated_count >= self.limits.max_propagated_nodes {
            if let Some(id) = self.truncated {
                return id;
            }
            let id = self.push(FailureNode::Truncated {
                causes: causes.into_boxed_slice(),
            });
            self.truncated = Some(id);
            return id;
        }
        let id = self.push(FailureNode::Propagated {
            operation,
            location,
            causes: causes.into_boxed_slice(),
        });
        self.propagated.insert(key, id);
        self.propagated_count += 1;
        id
    }

    pub(crate) fn node(&self, id: FailureId) -> Option<&FailureNode<R>> {
        self.nodes.get(id.0 as usize)
    }

    pub(crate) fn lineage(&self, start: FailureId) -> Vec<LineageStep<'_, R>> {
        let mut output = Vec::new();
        let mut current = start;
        for _ in 0..self.limits.max_render_depth {
            match self.node(current) {
                Some(FailureNode::Root { failure }) => {
                    output.push(LineageStep::Root(failure));
                    return output;
                }
                Some(FailureNode::Propagated {
                    operation,
                    location,
                    causes,
                }) => {
                    output.push(LineageStep::Propagated {
                        operation: *operation,
                        location: *location,
                    });
                    let Some(next) = causes.first() else {
                        output.push(LineageStep::Truncated);
                        return output;
                    };
                    current = *next;
                }
                Some(FailureNode::Truncated { causes }) => {
                    output.push(LineageStep::Truncated);
                    let Some(next) = causes.first() else {
                        return output;
                    };
                    current = *next;
                }
                None => {
                    output.push(LineageStep::Truncated);
                    return output;
                }
            }
        }
        output.push(LineageStep::Truncated);
        output
    }

    fn push(&mut self, node: FailureNode<R>) -> FailureId {
        let index = u32::try_from(self.nodes.len()).expect("failure arena exceeds u32::MAX nodes");
        self.nodes.push(node);
        FailureId(index)
    }
}

fn normalize_causes(causes: impl IntoIterator<Item = FailureId>, limit: usize) -> Vec<FailureId> {
    let mut normalized = Vec::new();
    for cause in causes {
        if normalized.contains(&cause) {
            continue;
        }
        if normalized.len() == limit {
            break;
        }
        normalized.push(cause);
    }
    normalized
}

#[cfg(test)]
#[path = "evaluation/tests/mod.rs"]
mod tests;
