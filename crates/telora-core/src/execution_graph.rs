//! Session-local demand evaluation. Static identities select tasks; only an
//! executed read requests one. Constructing a closure does not request the
//! globals referenced by its body. No resolver, type solver or old Engine is
//! involved, and a property chain publishes only its final reduced value.
use crate::{
    ast::BindingKind,
    mir::{HirId, PropertySite, ResolveState, SealedMir, SymbolId, SymbolKind, TypeId, TypeState},
    source::Location,
};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct NodeId(u32);
impl NodeId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PropertyKey {
    pub owner: TypeId,
    pub site: PropertySite,
    pub property: TypeId,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Task {
    Global {
        symbol: SymbolId,
        declaration: HirId,
    },
    Property {
        key: PropertyKey,
        providers: Box<[HirId]>,
    },
}

#[derive(Debug)]
pub struct Node {
    pub label: String,
    pub task: Task,
    pub ty: TypeId,
    pub location: Location,
}

/// Built once from the sealed graph, in stable symbol/property-record order.
/// Type definitions are already available and are not evaluation tasks.
#[derive(Debug)]
pub struct ExecutionGraph {
    nodes: Vec<Node>,
    globals: Vec<Option<NodeId>>,
    properties: BTreeMap<PropertyKey, NodeId>,
}

impl ExecutionGraph {
    pub fn from_mir(sealed: &SealedMir<'_>) -> Self {
        let mir = sealed.mir();
        let mut graph = Self {
            nodes: vec![],
            globals: vec![None; mir.symbols.len()],
            properties: BTreeMap::new(),
        };
        for (index, symbol) in mir.symbols.iter().enumerate() {
            if !matches!(
                symbol.kind,
                SymbolKind::Declaration(
                    BindingKind::Let | BindingKind::Def | BindingKind::Native | BindingKind::Decl
                )
            ) || !symbol.module.is_some_and(|module| {
                symbol.scope.is_some() && symbol.scope == mir.module_scopes[module.index()]
            }) {
                continue;
            }
            let declaration = *symbol
                .declarations
                .last()
                .expect("sealed global declaration");
            let TypeState::Known(ty) = mir.ty_slots[mir.symbol_types[index].index()] else {
                unreachable!("sealed global type")
            };
            let node = graph.push(Node {
                label: format!(
                    "{}::{}",
                    mir.modules[symbol.module.unwrap().index()].name,
                    symbol.name
                ),
                task: Task::Global {
                    symbol: SymbolId(index as u32),
                    declaration,
                },
                ty,
                location: mir.hir[declaration.index()].location,
            });
            graph.globals[index] = Some(node);
        }
        // Import/export aliases use the resolver's final target, never a name
        // search or another evaluation slot.
        for (index, symbol) in mir.symbols.iter().enumerate() {
            if let ResolveState::Bound(target) = symbol.resolution {
                graph.globals[index] = graph.globals[target.index()];
            }
        }
        for record in &mir.properties {
            let key = PropertyKey {
                owner: record.owner,
                site: record.site,
                property: record.property,
            };
            let node = graph.push(Node {
                label: format!(
                    "property({:?}, {:?}, {:?})",
                    key.owner, key.site, key.property
                ),
                task: Task::Property {
                    key,
                    providers: record.providers.clone().into_boxed_slice(),
                },
                ty: record.property,
                location: mir.hir[record.providers[0].index()].location,
            });
            assert!(
                graph.properties.insert(key, node).is_none(),
                "one reduced property per key"
            );
        }
        graph
    }

    fn push(&mut self, node: Node) -> NodeId {
        let id = NodeId(u32::try_from(self.nodes.len()).expect("execution graph exceeds u32"));
        self.nodes.push(node);
        id
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }
    pub fn global(&self, symbol: SymbolId) -> Option<NodeId> {
        self.globals.get(symbol.index()).copied().flatten()
    }
    /// Absence is determined by static evidence; no provider is run to find it.
    pub fn property(&self, key: PropertyKey) -> Option<NodeId> {
        self.properties.get(&key).copied()
    }
    pub fn evaluation<V>(&self) -> Evaluation<V> {
        Evaluation::new(self.nodes.len())
    }
}

/// The embedding executor owns diagnostics. All dependants may retain the same
/// failure ID without duplicating its diagnostic or retrying failed user code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FailureId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Pending,
    Running(usize),
    Ready(usize),
    Failed(FailureId),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Request<'a, V> {
    Start,
    Ready(&'a V),
}

#[derive(Debug, PartialEq, Eq)]
pub enum EvaluationError {
    /// User evaluation already failed; propagate this ID without retrying or diagnosing.
    Failed(FailureId),
    InvalidNode(NodeId),
    /// Includes the repeated node at both ends; unrelated callers are omitted.
    Cycle(Vec<NodeId>),
    /// A VM continuation can complete only its current, innermost task.
    CompletionOrder {
        requested: NodeId,
        active: Option<NodeId>,
    },
}

/// One table per session, shared by properties and globals. The VM keeps its
/// continuations separately; suspension requires no Rust recursion or graph copy.
pub struct Evaluation<V> {
    states: Vec<State>,
    values: Vec<V>,
    active: Vec<NodeId>,
    failed: bool,
}

impl<V> Evaluation<V> {
    fn new(nodes: usize) -> Self {
        Self {
            states: vec![State::Pending; nodes],
            values: vec![],
            active: vec![],
            failed: false,
        }
    }

    pub fn request(&mut self, node: NodeId) -> Result<Request<'_, V>, EvaluationError> {
        match self
            .states
            .get(node.index())
            .copied()
            .ok_or(EvaluationError::InvalidNode(node))?
        {
            State::Pending => {
                self.states[node.index()] = State::Running(self.active.len());
                self.active.push(node);
                Ok(Request::Start)
            }
            State::Running(start) => {
                let mut path = self.active[start..].to_vec();
                path.push(node);
                Err(EvaluationError::Cycle(path))
            }
            State::Ready(index) => Ok(Request::Ready(&self.values[index])),
            State::Failed(failure) => Err(EvaluationError::Failed(failure)),
        }
    }

    fn check_completion(&self, node: NodeId) -> Result<(), EvaluationError> {
        if self.active.last() == Some(&node) {
            return Ok(());
        }
        Err(EvaluationError::CompletionOrder {
            requested: node,
            active: self.active.last().copied(),
        })
    }

    pub fn complete(&mut self, node: NodeId, value: V) -> Result<(), EvaluationError> {
        self.check_completion(node)?;
        self.states[node.index()] = State::Ready(self.values.len());
        self.values.push(value);
        self.active.pop();
        Ok(())
    }

    pub fn fail(&mut self, node: NodeId, failure: FailureId) -> Result<(), EvaluationError> {
        self.check_completion(node)?;
        self.states[node.index()] = State::Failed(failure);
        self.failed = true;
        self.active.pop();
        Ok(())
    }

    /// Necessary session publication gate, not proof that all requested roots
    /// were evaluated. Unrequested nodes may remain pending under lazy semantics.
    pub fn can_publish(&self) -> bool {
        !self.failed && self.active.is_empty()
    }

    /// Abort active dependent computations after an uncaught task failure.
    pub fn fail_active(&mut self, failure: FailureId) {
        while let Some(node) = self.active.pop() {
            self.states[node.index()] = State::Failed(failure);
        }
        self.failed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_global_property_dependencies_suspend_and_reuse_results() {
        let mut session = Evaluation::new(4);
        let (property, config, other, unused) = (NodeId(0), NodeId(1), NodeId(2), NodeId(3));
        assert_eq!(session.request(property), Ok(Request::Start));
        assert_eq!(session.request(config), Ok(Request::Start));
        assert_eq!(session.request(other), Ok(Request::Start));
        assert!(!session.can_publish());
        // Partial reduce results stay in the VM continuation, outside this table.
        session.complete(other, 2).unwrap();
        assert_eq!(session.request(other), Ok(Request::Ready(&2)));
        session.complete(config, 3).unwrap();
        session.complete(property, 5).unwrap();
        assert_eq!(session.request(property), Ok(Request::Ready(&5)));
        assert_eq!(session.states[unused.index()], State::Pending);
        assert!(session.can_publish());
    }

    #[test]
    fn cycle_records_only_actual_reads_and_failure_does_not_retry() {
        let mut session = Evaluation::<i32>::new(4);
        let (root, property, global, independent) = (NodeId(0), NodeId(1), NodeId(2), NodeId(3));
        for node in [root, property, global] {
            assert_eq!(session.request(node), Ok(Request::Start));
        }
        assert_eq!(
            session.request(property),
            Err(EvaluationError::Cycle(vec![property, global, property]))
        );
        let failure = FailureId(7);
        for node in [global, property, root] {
            session.fail(node, failure).unwrap();
        }
        for _ in 0..3 {
            assert_eq!(
                session.request(property),
                Err(EvaluationError::Failed(failure))
            );
            assert_eq!(
                session.request(global),
                Err(EvaluationError::Failed(failure))
            );
        }
        assert_eq!(session.request(independent), Ok(Request::Start));
        session.complete(independent, 42).unwrap();
        assert_eq!(session.request(independent), Ok(Request::Ready(&42)));
        assert!(!session.can_publish());
    }

    #[test]
    fn out_of_order_completion_cannot_publish_an_unfinished_property() {
        let mut session = Evaluation::new(2);
        session.request(NodeId(0)).unwrap();
        session.request(NodeId(1)).unwrap();
        assert!(matches!(
            session.complete(NodeId(0), 1),
            Err(EvaluationError::CompletionOrder { .. })
        ));
        assert_eq!(
            session.request(NodeId(0)),
            Err(EvaluationError::Cycle(vec![
                NodeId(0),
                NodeId(1),
                NodeId(0)
            ]))
        );
        session.complete(NodeId(1), 2).unwrap();
        session.complete(NodeId(0), 3).unwrap();
        assert!(session.can_publish());
    }
}
