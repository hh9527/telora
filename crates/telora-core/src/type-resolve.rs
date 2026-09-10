//! Third MIR pass. All constraints use syntax slots and resolved SymbolIds.
use crate::ast::{BinaryOperator, BindingKind};
use crate::mir::*;
use crate::source::{Diagnostic, Location};

#[path = "type-resolve/arena.rs"]
mod arena;
#[cfg(test)]
#[path = "type-resolve/tests.rs"]
mod tests;

enum Task {
    Tuple {
        node: HirId,
        items: Vec<TypeSlotId>,
    },
    Member {
        node: HirId,
        receiver: TypeSlotId,
        name: String,
    },
    Numeric {
        node: HirId,
        operand: TypeSlotId,
    },
}
struct Solver<'a> {
    mir: &'a mut Mir,
    revision: usize,
    tasks: Vec<Task>,
}

pub fn resolve(mir: &mut Mir) {
    assert!(
        mir.symbols_closed,
        "type pass consumes a closed symbol result"
    );
    assert!(
        !mir.types_solved && mir.type_terms.is_empty(),
        "type pass runs once"
    );
    mir.required_types.resize(mir.hir.len(), false);
    let mut solver = Solver {
        mir,
        revision: 0,
        tasks: vec![],
    };
    for _ in 0..solver.mir.symbols.len() {
        let slot = solver.fresh();
        solver.mir.symbol_types.push(slot);
    }
    for index in 0..solver.mir.symbols.len() {
        let slot = solver.mir.symbol_types[index];
        let symbol = &solver.mir.symbols[index];
        let declarations = symbol.declarations.clone();
        let kind = symbol.kind;
        let outcome = symbol.resolution.clone();
        let name = symbol.name.clone();
        for node in declarations {
            solver.equal(slot, node.ty(), Some(solver.mir.hir[node.index()].location));
        }
        match outcome {
            ResolveState::Bound(target) if target.index() != index => {
                solver.equal(slot, solver.mir.symbol_types[target.index()], None)
            }
            ResolveState::Conflicted(origin) => solver.resolve_conflict(slot, origin),
            _ => {}
        }
        match kind {
            SymbolKind::Builtin => {
                if let Some(ty) = solver.builtin(&name) {
                    solver.equal(slot, ty, None);
                }
            }
            SymbolKind::Namespace(module) => {
                let ty = solver.structure(TypeConstructor::Namespace(module), vec![]);
                solver.equal(slot, ty, None);
            }
            SymbolKind::TypeParameter => {
                let ty =
                    solver.structure(TypeConstructor::Parameter(SymbolId(index as u32)), vec![]);
                let meta = solver.structure(TypeConstructor::Meta, vec![ty]);
                solver.equal(slot, meta, None);
            }
            _ => {}
        }
    }
    for index in 0..solver.mir.hir.len() {
        solver.generate(HirId(index as u32));
    }
    loop {
        let revision = solver.revision;
        let pending = std::mem::take(&mut solver.tasks);
        for task in pending {
            if let Some(task) = solver.solve_task(task) {
                solver.tasks.push(task);
            }
        }
        if solver.revision == revision {
            break;
        }
    }
    solver.finalize();
}

impl Solver<'_> {
    fn child(&self, node: HirId, role: Role) -> Option<HirId> {
        self.mir.hir[node.index()]
            .children
            .iter()
            .find(|edge| edge.role == role)
            .map(|edge| edge.node)
    }
    fn children(&self, node: HirId, role: Role) -> Vec<HirId> {
        self.mir.hir[node.index()]
            .children
            .iter()
            .filter(|edge| edge.role == role)
            .map(|edge| edge.node)
            .collect()
    }
    fn same(&mut self, node: HirId, other: TypeSlotId) {
        self.equal(node.ty(), other, Some(self.mir.hir[node.index()].location));
    }
    fn assign(&mut self, node: HirId, constructor: TypeConstructor, arguments: Vec<TypeSlotId>) {
        let ty = self.structure(constructor, arguments);
        self.same(node, ty);
    }
    fn builtin(&mut self, name: &str) -> Option<TypeSlotId> {
        let constructor = match name {
            "Int" => TypeConstructor::Int,
            "Float" => TypeConstructor::Float,
            "String" => TypeConstructor::String,
            "Bytes" => TypeConstructor::Bytes,
            "Bool" => TypeConstructor::Bool,
            "Never" => TypeConstructor::Never,
            "Unit" => TypeConstructor::Tuple,
            _ => return None,
        };
        let ty = self.structure(constructor, vec![]);
        Some(self.structure(TypeConstructor::Meta, vec![ty]))
    }
    fn resolve_conflict(&mut self, slot: TypeSlotId, origin: ConflictId) {
        let id = self
            .mir
            .type_conflicts
            .iter()
            .position(|conflict| conflict.resolve_origin == Some(origin))
            .map(|id| TypeConflictId(id as u32))
            .unwrap_or_else(|| {
                let id = TypeConflictId(self.mir.type_conflicts.len() as u32);
                self.mir.type_conflicts.push(TypeConflict {
                    left: slot,
                    right: slot,
                    location: None,
                    message: format!("symbol conflict {origin:?}"),
                    resolve_origin: Some(origin),
                });
                id
            });
        let root = self.root(slot);
        self.mir.ty_slots[root.index()] = TypeState::Conflicted(id);
    }
    fn generate(&mut self, node: HirId) {
        self.mir.required_types[node.index()] = true;
        if let Some(slot) = self.mir.hir[node.index()].resolution {
            match self.mir.resolve_slots[slot.index()].clone() {
                ResolveState::Bound(symbol) => {
                    let source = &self.mir.symbols[symbol.index()];
                    if source.kind == SymbolKind::Builtin {
                        let name = source.name.clone();
                        // Each occurrence supplies rigid type evidence without
                        // letting a bad annotation poison the intrinsic itself.
                        if let Some(ty) = self.builtin(&name) {
                            self.same(node, ty);
                        }
                    } else {
                        self.same(node, self.mir.symbol_types[symbol.index()]);
                    }
                    return;
                }
                ResolveState::Conflicted(origin) => {
                    self.resolve_conflict(node.ty(), origin);
                    return;
                }
                ResolveState::Unresolved => return,
                ResolveState::Member { receiver, name } => {
                    let HirKind::Name(name) = &self.mir.hir[name.index()].kind else {
                        unreachable!()
                    };
                    self.tasks.push(Task::Member {
                        node,
                        receiver: receiver.ty(),
                        name: name.clone(),
                    });
                    return;
                }
                ResolveState::Pending => unreachable!("symbol pass is authoritative"),
            }
        }
        match &self.mir.hir[node.index()].kind {
            HirKind::TypeOperation(
                operation @ (TypeOperation::Function | TypeOperation::Tuple | TypeOperation::Unit),
            ) => {
                let constructor = if *operation == TypeOperation::Function {
                    TypeConstructor::Function
                } else {
                    TypeConstructor::Tuple
                };
                let mut raw = vec![];
                for argument in self.children(node, Role::Argument) {
                    let slot = self.fresh();
                    self.assign(argument, TypeConstructor::Meta, vec![slot]);
                    raw.push(slot);
                }
                let ty = self.structure(constructor, raw);
                self.assign(node, TypeConstructor::Meta, vec![ty]);
            }
            HirKind::TypeSyntax => {
                let operand = self.child(node, Role::Operand).unwrap();
                self.same(node, operand.ty());
            }
            HirKind::Int(_) => self.assign(node, TypeConstructor::Int, vec![]),
            HirKind::Float(_) => self.assign(node, TypeConstructor::Float, vec![]),
            HirKind::String(_) => self.assign(node, TypeConstructor::String, vec![]),
            HirKind::Bytes(_) => self.assign(node, TypeConstructor::Bytes, vec![]),
            HirKind::Tuple => {
                let items = self
                    .children(node, Role::Item)
                    .into_iter()
                    .map(HirId::ty)
                    .collect();
                self.tasks.push(Task::Tuple { node, items });
            }
            HirKind::Array => {
                let item = self.fresh();
                for child in self.children(node, Role::Item) {
                    self.equal(item, child.ty(), Some(self.mir.hir[child.index()].location));
                }
                self.assign(node, TypeConstructor::Array, vec![item]);
            }
            HirKind::Dict => {
                let mut fields = vec![];
                for field in self.children(node, Role::Field) {
                    let (Some(name), Some(value)) = (
                        self.child(field, Role::Name),
                        self.child(field, Role::Value),
                    ) else {
                        return;
                    };
                    let HirKind::Name(name) = &self.mir.hir[name.index()].kind else {
                        unreachable!()
                    };
                    fields.push((name.clone(), value.ty()));
                }
                fields.sort_by(|a, b| a.0.cmp(&b.0));
                self.assign(
                    node,
                    TypeConstructor::Record(fields.iter().map(|(name, _)| name.clone()).collect()),
                    fields.into_iter().map(|(_, slot)| slot).collect(),
                );
            }
            HirKind::Block => {
                if let Some(value) = self.child(node, Role::Result) {
                    self.same(node, value.ty());
                }
            }
            HirKind::DictField => {
                if let Some(value) = self.child(node, Role::Value) {
                    self.same(node, value.ty());
                }
            }
            HirKind::Binding { kind, .. } => {
                if matches!(kind, BindingKind::OpenImport | BindingKind::Export) {
                    self.mir.required_types[node.index()] = false;
                    return;
                }
                if !matches!(
                    kind,
                    BindingKind::Import
                        | BindingKind::Decl
                        | BindingKind::Native
                        | BindingKind::NativeType
                ) {
                    if let Some(value) = self.child(node, Role::Value) {
                        self.same(node, value.ty());
                    }
                }
                self.annotation(node);
            }
            HirKind::Parameter | HirKind::ReturnType => self.annotation(node),
            HirKind::Closure => {
                let result = self.child(node, Role::ReturnType).unwrap();
                let body = self.child(node, Role::Body).unwrap();
                self.equal(
                    result.ty(),
                    body.ty(),
                    Some(self.mir.hir[body.index()].location),
                );
                let mut args = self
                    .children(node, Role::Parameter)
                    .into_iter()
                    .map(HirId::ty)
                    .collect::<Vec<_>>();
                args.push(result.ty());
                self.assign(node, TypeConstructor::Function, args);
            }
            HirKind::Call => {
                let callee = self.child(node, Role::Callee).unwrap();
                let mut args = self
                    .children(node, Role::Argument)
                    .into_iter()
                    .map(HirId::ty)
                    .collect::<Vec<_>>();
                args.push(node.ty());
                let shape = self.structure(TypeConstructor::Function, args);
                self.equal(
                    callee.ty(),
                    shape,
                    Some(self.mir.hir[node.index()].location),
                );
            }
            HirKind::Binary(operator) => {
                let operator = *operator;
                let left = self.child(node, Role::Left).unwrap();
                let right = self.child(node, Role::Right).unwrap();
                self.equal(
                    left.ty(),
                    right.ty(),
                    Some(self.mir.hir[node.index()].location),
                );
                match operator {
                    BinaryOperator::Add
                    | BinaryOperator::Subtract
                    | BinaryOperator::Multiply
                    | BinaryOperator::Divide
                    | BinaryOperator::Remainder => {
                        self.same(node, left.ty());
                        self.tasks.push(Task::Numeric {
                            node,
                            operand: left.ty(),
                        });
                    }
                    BinaryOperator::Equal
                    | BinaryOperator::NotEqual
                    | BinaryOperator::LessThan
                    | BinaryOperator::LessThanOrEqual
                    | BinaryOperator::GreaterThan
                    | BinaryOperator::GreaterThanOrEqual => {
                        self.assign(node, TypeConstructor::Bool, vec![])
                    }
                    _ => self.unsupported(node),
                }
            }
            HirKind::If => {
                let condition = self.child(node, Role::Condition).unwrap();
                self.assign(condition, TypeConstructor::Bool, vec![]);
                for role in [Role::Then, Role::Else] {
                    let branch = self.child(node, role).unwrap();
                    self.same(node, branch.ty());
                }
            }
            HirKind::TypeAscription => {
                let value = self.child(node, Role::Value).unwrap();
                let target = self.child(node, Role::Target).unwrap();
                self.same(node, value.ty());
                self.assign(target, TypeConstructor::Meta, vec![value.ty()]);
            }
            HirKind::Name(_) | HirKind::Text(_) => self.mir.required_types[node.index()] = false,
            _ => self.unsupported(node),
        }
    }
    fn unsupported(&mut self, node: HirId) {
        self.mir.diagnostics.push(Diagnostic::error(
            format!(
                "new type pass has no evidence rule yet for {:?}",
                self.mir.hir[node.index()].kind
            ),
            self.mir.hir[node.index()].location,
        ));
    }
    fn annotation(&mut self, node: HirId) {
        if let Some(annotation) = self.child(node, Role::Annotation) {
            self.assign(annotation, TypeConstructor::Meta, vec![node.ty()]);
        }
    }
    fn solve_task(&mut self, task: Task) -> Option<Task> {
        let (node, dependencies) = match &task {
            Task::Tuple { node, items } => (*node, items.clone()),
            Task::Member { node, receiver, .. } => (*node, vec![*receiver]),
            Task::Numeric { node, operand } => (*node, vec![*operand]),
        };
        for dependency in dependencies {
            if let TypeState::Conflicted(id) = self.mir.ty_slots[self.root(dependency).index()] {
                let root = self.root(node.ty());
                self.mir.ty_slots[root.index()] = TypeState::Conflicted(id);
                self.revision += 1;
                return None;
            }
        }
        match task {
            Task::Tuple { node, items } => {
                let mut raw = vec![];
                let mut metadata = 0;
                for &item in &items {
                    let Some(term) = self.term(item) else {
                        return Some(Task::Tuple { node, items });
                    };
                    if term.constructor == TypeConstructor::Meta {
                        metadata += 1;
                        raw.push(term.arguments[0]);
                    }
                }
                let expected_meta = self
                    .term(node.ty())
                    .is_some_and(|term| term.constructor == TypeConstructor::Meta);
                if metadata == items.len() && (metadata > 0 || expected_meta) {
                    let tuple = self.structure(TypeConstructor::Tuple, raw);
                    self.assign(node, TypeConstructor::Meta, vec![tuple]);
                } else if metadata == 0 {
                    self.assign(node, TypeConstructor::Tuple, items);
                } else {
                    self.conflict(
                        node.ty(),
                        node.ty(),
                        Some(self.mir.hir[node.index()].location),
                        "tuple mixes types and values".into(),
                    );
                }
            }
            Task::Member {
                node,
                receiver,
                name,
            } => {
                let Some(term) = self.term(receiver) else {
                    return Some(Task::Member {
                        node,
                        receiver,
                        name,
                    });
                };
                if let TypeConstructor::Record(fields) = &term.constructor {
                    if let Some(index) = fields.iter().position(|field| *field == name) {
                        let ty = term.arguments[index];
                        self.same(node, ty);
                    } else {
                        self.conflict(
                            node.ty(),
                            node.ty(),
                            Some(self.mir.hir[node.index()].location),
                            format!("unknown field {name:?}"),
                        );
                    }
                } else {
                    self.conflict(
                        node.ty(),
                        node.ty(),
                        Some(self.mir.hir[node.index()].location),
                        "field receiver is not a record".into(),
                    );
                }
            }
            Task::Numeric { node, operand } => {
                let Some(term) = self.term(operand) else {
                    return Some(Task::Numeric { node, operand });
                };
                if !matches!(
                    term.constructor,
                    TypeConstructor::Int | TypeConstructor::Float | TypeConstructor::Never
                ) {
                    self.conflict(
                        node.ty(),
                        operand,
                        Some(self.mir.hir[node.index()].location),
                        "numeric operand required".into(),
                    );
                }
            }
        }
        None
    }
}
