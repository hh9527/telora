//! Third MIR pass. All constraints use syntax slots and resolved SymbolIds.
use crate::ast::{BinaryOperator, BindingKind, BlameAction, UnaryOperator};
use crate::mir::*;
use crate::source::{Diagnostic, Location};

#[path = "type-resolve/arena.rs"]
mod arena;
#[path = "type-resolve/constraints.rs"]
mod constraints;
#[path = "type-resolve/definitions.rs"]
mod definitions;
#[path = "type-resolve/alias-cycles.rs"]
mod alias_cycles;
#[path = "type-resolve/evidence.rs"]
mod evidence;
#[path = "type-resolve/instances.rs"]
mod instances;
#[path = "type-resolve/layouts.rs"]
mod layouts;
#[path = "type-resolve/members.rs"]
mod members;
#[path = "type-resolve/properties.rs"]
mod properties;
#[cfg(test)]
#[path = "type-resolve/tests.rs"]
mod tests;

enum Task {
    ShapeEqual { left: TypeSlotId, right: TypeSlotId, location: Option<Location> },
    Unchecked { node: HirId, argument: TypeSlotId },
    RefineInstance {
        source: TypeSlotId,
        target: TypeSlotId,
        arguments: Vec<(SymbolId, TypeSlotId)>,
        location: Option<Location>,
        constructor: TypeConstructor,
    },
    BoundContext {
        node: HirId,
        subject: TypeSlotId,
        bound: TypeSlotId,
    },
    DiagnosticInput {
        node: HirId,
        input: TypeSlotId,
    },
    Instantiate {
        source: TypeSlotId,
        target: TypeSlotId,
        arguments: Vec<(SymbolId, TypeSlotId)>,
        location: Option<Location>,
    },
    Call {
        node: HirId,
        callee: TypeSlotId,
        arguments: Vec<TypeSlotId>,
    },
    Join {
        node: HirId,
        values: Vec<TypeSlotId>,
    },
    Block {
        node: HirId,
        statements: Vec<TypeSlotId>,
        result: TypeSlotId,
    },
    Projection {
        node: HirId,
        receiver: TypeSlotId,
        index: Option<usize>,
    },
    ConstructorPattern {
        node: HirId,
        constructor: TypeSlotId,
        payload: Option<TypeSlotId>,
    },
    Fit {
        node: HirId,
        expected: TypeSlotId,
        actual: TypeSlotId,
    },
    Tuple {
        node: HirId,
        items: Vec<TypeSlotId>,
    },
    Member {
        node: HirId,
        receiver: TypeSlotId,
        name: String,
    },
    Not {
        node: HirId,
        operand: TypeSlotId,
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
    nominal_index: Vec<Option<usize>>,
    nominal_owner: Vec<Option<SymbolId>>,
    return_slots: Vec<Option<TypeSlotId>>,
    pending_blocks: Vec<bool>,
    administrative: Vec<bool>,
    decorator_contexts: Vec<Option<TypeSlotId>>,
    property_declarations: Vec<(TypeSlotId, PropertySite, HirId)>,
    check_declarations: Vec<(TypeSlotId, PropertySite, HirId)>,
    bottom_candidates: Vec<TypeSlotId>,
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
    mir.member_selections.resize(mir.hir.len(), None);
    mir.type_instances.resize_with(mir.hir.len(), Vec::new);
    let mut solver = Solver::new(mir);
    for _ in 0..solver.mir.symbols.len() {
        let slot = solver.fresh();
        solver.mir.symbol_types.push(slot);
    }
    solver.prepare_definitions();
    solver.prepare_properties();
    for index in 0..solver.mir.symbols.len() {
        let slot = solver.mir.symbol_types[index];
        let symbol = &solver.mir.symbols[index];
        let declarations = symbol.declarations.clone();
        let kind = symbol.kind;
        let outcome = symbol.resolution.clone();
        for node in declarations {
            solver.equal(slot, node.ty(), Some(solver.mir.hir[node.index()].location));
        }
        match outcome {
            ResolveState::Bound(target)
                if target.index() != index && kind != SymbolKind::Pattern =>
            {
                solver.equal(slot, solver.mir.symbol_types[target.index()], None)
            }
            ResolveState::Conflicted(origin) => solver.resolve_conflict(slot, origin),
            _ => {}
        }
        match kind {
            SymbolKind::Declaration(BindingKind::NativeType) => {
                if let Some(ty) = solver.native_type(SymbolId(index as u32)) {
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
    solver.reject_alias_cycles();
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
            if !solver.finish_literals() && !solver.finish_bottoms() {
                break;
            }
        }
    }
    solver.finalize();
    solver.finalize_properties();
    solver.finalize_checks();
    solver.prove_bounds();
    solver.materialize_instances();
    solver.mir.types_solved = true;
}

impl Solver<'_> {
    fn new(mir: &mut Mir) -> Solver<'_> {
        mir.value_adjustments.resize(mir.hir.len(), None);
        Solver {
            nominal_index: vec![None; mir.symbols.len()],
            nominal_owner: vec![None; mir.hir.len()],
            return_slots: vec![None; mir.hir.len()],
            pending_blocks: vec![false; mir.hir.len()],
            administrative: vec![false; mir.hir.len()],
            decorator_contexts: vec![None; mir.hir.len()],
            property_declarations: vec![],
            check_declarations: vec![],
            bottom_candidates: vec![],
            mir,
            revision: 0,
            tasks: vec![],
        }
    }
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
        if self.administrative[node.index()] {
            return;
        }
        self.mir.required_types[node.index()] = true;
        if let Some(slot) = self.mir.hir[node.index()].resolution {
            match self.mir.resolve_slots[slot.index()].clone() {
                ResolveState::Bound(symbol) => {
                    let source = &self.mir.symbols[symbol.index()];
                    if source.native_type.is_some() {
                        // Each occurrence supplies rigid type evidence without
                        // letting a bad annotation poison the intrinsic itself.
                        if let Some(ty) = self.native_type(symbol) {
                            self.same(node, ty);
                        }
                    } else {
                        self.reference_type(node, symbol);
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
                TypeOperation::Struct | TypeOperation::Enum | TypeOperation::Newtype,
            ) => self.type_operation(node),
            HirKind::TypeMember { .. } => {}
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
            HirKind::InterpolatedString => self.assign(node, TypeConstructor::String, vec![]),
            HirKind::Unary(operator) => {
                let operator = *operator;
                let operand = self.child(node, Role::Operand).unwrap();
                match operator {
                    UnaryOperator::Not => {
                        self.same(node, operand.ty());
                        self.tasks.push(Task::Not { node, operand: operand.ty() });
                    }
                    UnaryOperator::LogicalNot => {
                        self.assign(operand, TypeConstructor::Bool, vec![]);
                        self.assign(node, TypeConstructor::Bool, vec![]);
                    }
                    UnaryOperator::BitNot => {
                        self.assign(operand, TypeConstructor::Int, vec![]);
                        self.assign(node, TypeConstructor::Int, vec![]);
                    }
                    UnaryOperator::Negate => {
                        self.same(node, operand.ty());
                        self.tasks.push(Task::Numeric {
                            node,
                            operand: operand.ty(),
                        });
                    }
                }
            }
            HirKind::Index | HirKind::TupleProjection(_) => {
                let index = if let HirKind::TupleProjection(index) = self.mir.hir[node.index()].kind
                {
                    Some(index)
                } else {
                    None
                };
                let receiver = self.child(node, Role::Receiver).unwrap().ty();
                self.tasks.push(Task::Projection {
                    node,
                    receiver,
                    index,
                });
            }
            HirKind::TypeMetadata => {
                let operand = self.child(node, Role::Operand).unwrap();
                let raw = self.fresh();
                self.assign(operand, TypeConstructor::Meta, vec![raw]);
                self.assign(node, TypeConstructor::TypeOf, vec![raw]);
            }
            HirKind::Debug { .. } => {
                let value = self.child(node, Role::Value).unwrap();
                self.same(node, value.ty());
            }
            HirKind::Return => {
                let value = self.child(node, Role::Value).unwrap();
                if let Some(result) = self.return_slots[node.index()] {
                    self.fit(node, result, value.ty());
                } else {
                    self.mir.diagnostics.push(Diagnostic::error(
                        "return outside a function",
                        self.mir.hir[node.index()].location,
                    ));
                }
                self.assign(node, TypeConstructor::Never, vec![]);
            }
            HirKind::Panic | HirKind::Raise(_) => {
                let action = if let HirKind::Raise(action) = self.mir.hir[node.index()].kind {
                    action
                } else {
                    BlameAction::Fail
                };
                let message = self.child(node, Role::Value).unwrap();
                if matches!(action, BlameAction::Warn | BlameAction::Raise) {
                    self.tasks.push(Task::DiagnosticInput {
                        node,
                        input: message.ty(),
                    });
                } else {
                    self.assign(message, TypeConstructor::String, vec![]);
                }
                match action {
                    BlameAction::Warn => {
                        let payload = self.fresh();
                        self.assign(node, TypeConstructor::Option, vec![payload]);
                    }
                    BlameAction::Build => self.assign(
                        node,
                        TypeConstructor::Native(NativeTypeId::BLAME_ERROR),
                        vec![],
                    ),
                    BlameAction::Raise | BlameAction::Fail => {
                        self.assign(node, TypeConstructor::Never, vec![])
                    }
                }
            }
            HirKind::Wildcard | HirKind::PatternField => {}
            HirKind::StructPattern => {
                for field in self.children(node, Role::Field) {
                    let name = self.child(field, Role::Name).unwrap();
                    let HirKind::Name(name) = &self.mir.hir[name.index()].kind else {
                        unreachable!()
                    };
                    self.tasks.push(Task::Member {
                        node: field,
                        receiver: node.ty(),
                        name: name.clone(),
                    });
                    let pattern = self.child(field, Role::Pattern).unwrap();
                    self.equal(
                        field.ty(),
                        pattern.ty(),
                        Some(self.mir.hir[field.index()].location),
                    );
                }
            }
            HirKind::TuplePattern => {
                let items = self
                    .children(node, Role::Item)
                    .into_iter()
                    .map(HirId::ty)
                    .collect();
                self.assign(node, TypeConstructor::Tuple, items);
            }
            HirKind::ConstructorPattern => {
                let constructor = self.child(node, Role::Callee).unwrap().ty();
                let payload = self.child(node, Role::Pattern).map(HirId::ty);
                self.tasks.push(Task::ConstructorPattern {
                    node,
                    constructor,
                    payload,
                });
            }
            HirKind::MatchArm { .. } => {
                if let Some(guard) = self.child(node, Role::Guard) {
                    self.assign(guard, TypeConstructor::Bool, vec![]);
                }
                let value = self.child(node, Role::Value).unwrap();
                self.same(node, value.ty());
            }
            HirKind::Match => {
                let value = self.child(node, Role::Value).unwrap();
                let arms = self.children(node, Role::Arm);
                for &arm in &arms {
                    let pattern = self.child(arm, Role::Pattern).unwrap();
                    self.equal(
                        value.ty(),
                        pattern.ty(),
                        Some(self.mir.hir[pattern.index()].location),
                    );
                }
                self.tasks.push(Task::Join {
                    node,
                    values: arms.into_iter().map(|arm| self.child(arm, Role::Value).unwrap().ty()).collect(),
                });
            }
            HirKind::IfLet | HirKind::LetElse => {
                let pattern = self.child(node, Role::Pattern).unwrap();
                let value = self.child(node, Role::Value).unwrap();
                self.equal(
                    value.ty(),
                    pattern.ty(),
                    Some(self.mir.hir[pattern.index()].location),
                );
                if matches!(self.mir.hir[node.index()].kind, HirKind::IfLet) {
                    let values = [Role::Then, Role::Else]
                        .into_iter()
                        .map(|r| self.child(node, r).unwrap().ty())
                        .collect();
                    self.tasks.push(Task::Join { node, values });
                } else {
                    let otherwise = self.child(node, Role::Else).unwrap();
                    self.assign(otherwise, TypeConstructor::Never, vec![]);
                    let body = self.child(node, Role::Body).unwrap();
                    self.same(node, body.ty());
                }
            }
            HirKind::Tuple => {
                let items = self
                    .children(node, Role::Item)
                    .into_iter()
                    .map(HirId::ty)
                    .collect();
                self.tasks.push(Task::Tuple { node, items });
            }
            HirKind::Array => {
                let items = self
                    .children(node, Role::Item)
                    .into_iter()
                    .map(HirId::ty)
                    .collect();
                self.assign(node, TypeConstructor::ArrayLiteral, items);
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
                    self.pending_blocks[node.index()] = true;
                    self.tasks.push(Task::Block {
                        node,
                        statements: self.children(node, Role::Binding).into_iter()
                            .filter(|binding| matches!(self.mir.hir[binding.index()].kind,
                                HirKind::Binding { kind: BindingKind::Let | BindingKind::Def | BindingKind::Impl, .. }))
                            .filter_map(|binding| self.child(binding, Role::Value))
                            .map(HirId::ty).collect(),
                        result: value.ty(),
                    });
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
                        self.fit(node, node.ty(), value.ty());
                    }
                }
                self.annotation(node);
            }
            HirKind::Parameter | HirKind::ReturnType => self.annotation(node),
            HirKind::TypeParameter => {
                for bound in self.children(node, Role::Bound) {
                    self.tasks.push(Task::BoundContext {
                        node: bound,
                        subject: node.ty(),
                        bound: bound.ty(),
                    });
                }
            }
            HirKind::Decorator { configured } => {
                let configured = *configured;
                let callee = self.child(node, Role::Callee).unwrap().ty();
                let provider = if configured {
                    let mut arguments = self
                        .children(node, Role::Argument)
                        .into_iter()
                        .map(HirId::ty)
                        .collect::<Vec<_>>();
                    let provider = self.fresh();
                    arguments.push(provider);
                    let factory = self.structure(TypeConstructor::Function, arguments);
                    self.equal(callee, factory, Some(self.mir.hir[node.index()].location));
                    provider
                } else {
                    callee
                };
                let context = self.decorator_contexts[node.index()]
                    .unwrap_or_else(|| self.structure(TypeConstructor::Type, vec![]));
                let previous = self.structure(TypeConstructor::Option, vec![node.ty()]);
                self.tasks.push(Task::Call {
                    node,
                    callee: provider,
                    arguments: vec![context, previous],
                });
            }
            HirKind::ConstructionCheck { configured } => {
                if !*configured || self.children(node, Role::Argument).len() != 1 {
                    self.mir.diagnostics.push(Diagnostic::error("@check requires exactly one check function", self.mir.hir[node.index()].location));
                    self.mir.required_types[node.index()] = false;
                } else if !self.check_declarations.iter().any(|(_, _, check)| *check == node) {
                    self.mir.diagnostics.push(Diagnostic::error("@check is supported on structs, newtypes and payload variants", self.mir.hir[node.index()].location));
                    self.mir.required_types[node.index()] = false;
                }
            }
            HirKind::Closure => {
                let result = self.child(node, Role::ReturnType).unwrap();
                let body = self.child(node, Role::Body).unwrap();
                self.fit(body, result.ty(), body.ty());
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
                let args = self
                    .children(node, Role::Argument)
                    .into_iter()
                    .map(HirId::ty)
                    .collect::<Vec<_>>();
                if self.mir.hir[callee.index()].resolution.is_some_and(|slot| matches!(self.mir.resolve_slots[slot.index()], ResolveState::Bound(symbol) if matches!(self.mir.symbols[symbol.index()].kind, SymbolKind::Parameter | SymbolKind::Pattern))) {
                    let mut shape = args.clone(); shape.push(node.ty());
                    let shape = self.structure(TypeConstructor::Function, shape);
                    self.equal(callee.ty(), shape, Some(self.mir.hir[node.index()].location));
                } else { self.tasks.push(Task::Call { node, callee: callee.ty(), arguments: args }); }
            }
            HirKind::TypeApply => {
                let callee = self.child(node, Role::Callee).unwrap();
                let parameters = self.mir.type_instances[callee.index()].clone();
                let arguments = self.children(node, Role::Argument);
                if parameters.is_empty() || parameters.len() != arguments.len() {
                    self.conflict(
                        node.ty(),
                        node.ty(),
                        Some(self.mir.hir[node.index()].location),
                        "explicit type application requires a generic binding with matching arity"
                            .into(),
                    );
                } else {
                    for ((_, parameter), argument) in parameters.into_iter().zip(arguments) {
                        self.assign(argument, TypeConstructor::Meta, vec![parameter]);
                    }
                    self.same(node, callee.ty());
                }
            }
            HirKind::InferredTypeArgument => {}
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
                    BinaryOperator::And | BinaryOperator::Or => {
                        self.assign(left, TypeConstructor::Bool, vec![]);
                        self.assign(node, TypeConstructor::Bool, vec![]);
                    }
                    BinaryOperator::BitAnd | BinaryOperator::BitOr | BinaryOperator::BitXor => {
                        self.assign(left, TypeConstructor::Int, vec![]);
                        self.assign(node, TypeConstructor::Int, vec![]);
                    }
                    _ => self.unsupported(node),
                }
            }
            HirKind::If => {
                let condition = self.child(node, Role::Condition).unwrap();
                self.assign(condition, TypeConstructor::Bool, vec![]);
                let values = [Role::Then, Role::Else]
                    .into_iter()
                    .map(|r| self.child(node, r).unwrap().ty())
                    .collect();
                self.tasks.push(Task::Join { node, values });
            }
            HirKind::CheckedCast => {
                let target = self.child(node, Role::Target).unwrap();
                let ty = self.fresh();
                self.assign(target, TypeConstructor::Meta, vec![ty]);
                let message = self.structure(TypeConstructor::String, vec![]);
                self.assign(node, TypeConstructor::Result, vec![ty, message]);
            }
            HirKind::TypeAscription => {
                let value = self.child(node, Role::Value).unwrap();
                let target = self.child(node, Role::Target).unwrap();
                self.same(node, value.ty());
                self.assign(target, TypeConstructor::Meta, vec![value.ty()]);
            }
            HirKind::Name(_) | HirKind::NativeTypeSlot(_) => {
                self.mir.required_types[node.index()] = false
            }
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
        let task = match self.solve_constraint(task) {
            Ok(done) => return done,
            Err(task) => task,
        };
        let (node, dependencies) = match &task {
            Task::Tuple { node, items } => (*node, items.clone()),
            Task::Member { node, receiver, .. } => (*node, vec![*receiver]),
            Task::Numeric { node, operand } | Task::Not { node, operand } => (*node, vec![*operand]),
            _ => unreachable!(),
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
                    self.assign(node, TypeConstructor::TupleLiteral, items);
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
            } => return self.member(node, receiver, name),
            Task::Not { node, operand } => {
                let Some(term) = self.term(operand) else {
                    return Some(Task::Not { node, operand });
                };
                if !matches!(term.constructor, TypeConstructor::Bool | TypeConstructor::Int | TypeConstructor::Never) {
                    self.conflict(node.ty(), operand, Some(self.mir.hir[node.index()].location), "Bool or Int operand required for !".into());
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
            _ => unreachable!(),
        }
        None
    }
}
