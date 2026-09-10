//! MIR-to-bytecode lowering. This module cannot resolve names, infer types or
//! access a VM. The retained LIR assembler only encodes the emitted operations.
use crate::{
    ast::{BinaryOperator as B, BindingKind},
    bytecode::{BytecodeFunction, Constant},
    execution_graph::ExecutionGraph,
    lir::{self, ConstantId, Function, Item, LabelId, Operation as O, RegisterId as R},
    mir::*,
    source::{Diagnostic, Origin, Severity, WithOrigin},
};

pub struct CompiledEntry {
    pub graph: ExecutionGraph,
    pub symbol: SymbolId,
    pub result_type: TypeId,
    pub bytecode: BytecodeFunction,
    pub native_links: Vec<NativeLink>,
    pub types: crate::type_image::TypeImage,
    pub eval_call: Option<EvalCall>,
    pub data_links: Vec<DataLink>,
}

#[derive(Debug)]
pub struct DataLink {
    pub constant: usize,
    pub module: ModuleId,
    pub name: String,
    pub ty: TypeId,
    pub location: crate::source::Location,
}
impl DataLink {
    pub(crate) fn key(&self) -> String {
        format!("\0mir-data:{}", self.module.index())
    }
}

pub struct EvalCall {
    pub bytecode: BytecodeFunction,
    pub result_type: TypeId,
}

/// Compile the fixed entry.Eval ABI adapter before any VM exists.
pub fn compile_eval(
    sealed: SealedMir<'_>,
    entry: SymbolId,
    value_type: TypeId,
) -> Result<CompiledEntry, Vec<Diagnostic>> {
    let mut artifact = compile(sealed, entry)?;
    let signature = match artifact.types.types[artifact.result_type.index()].constructor {
        TypeConstructor::Nominal(symbol) => artifact
            .types
            .definition(symbol)
            .filter(|d| d.parameters.is_empty())
            .and_then(|d| d.members.iter().find(|m| m.name == "evaluate"))
            .and_then(|m| m.payload)
            .map(|ty| &artifact.types.types[ty.index()]),
        _ => None,
    };
    if !signature.is_some_and(|ty| {
        ty.constructor == TypeConstructor::Function
            && ty.arguments.len() == 2
            && ty.arguments[1] == value_type
    }) {
        return Err(vec![Diagnostic {
            severity: Severity::Error,
            message: "invalid static entry.Eval evaluate signature".into(),
            labels: vec![],
            notes: vec![],
        }]);
    }
    use crate::bytecode::{Instruction as I, Register};
    artifact.eval_call = Some(EvalCall {
        result_type: value_type,
        bytecode: BytecodeFunction::with_signature(
            "<entry.Eval.evaluate>",
            2,
            0,
            4,
            vec![],
            vec![
                I::GetField {
                    dst: Register(2),
                    dict: Register(0),
                    field: "evaluate".into(),
                },
                I::Move {
                    dst: Register(3),
                    src: Register(1),
                },
                I::Call {
                    base: Register(2),
                    argument_count: 1,
                },
                I::Return { src: Register(2) },
            ],
        ),
    });
    Ok(artifact)
}

#[derive(Debug)]
pub struct NativeLink {
    pub constant: usize,
    pub symbol: SymbolId,
    pub module: Option<u32>,
    pub name: String,
    pub arity: usize,
    pub location: crate::source::Location,
}

pub fn compile(sealed: SealedMir<'_>, entry: SymbolId) -> Result<CompiledEntry, Vec<Diagnostic>> {
    let graph = ExecutionGraph::from_mir(&sealed);
    let (mir, types) = sealed.into_parts();
    let Some(symbol) = mir.symbols.get(entry.index()) else {
        return Err(vec![Diagnostic {
            severity: Severity::Error,
            message: "invalid codegen entry SymbolId".into(),
            labels: vec![],
            notes: vec![],
        }]);
    };
    let ResolveState::Bound(target) = symbol.resolution else {
        return Err(vec![Diagnostic {
            severity: Severity::Error,
            message: "codegen entry is not bound".into(),
            labels: vec![],
            notes: vec![],
        }]);
    };
    let Some(&declaration) = mir.symbols[target.index()].declarations.last() else {
        return Err(vec![Diagnostic {
            severity: Severity::Error,
            message: "codegen entry has no declaration".into(),
            labels: vec![],
            notes: vec![],
        }]);
    };
    let mut emitter = Emitter::new(mir, &graph, symbol.name.clone());
    let globals = reachable_globals(mir, target);
    // Native ABI values and injected data already exist before initialization.
    for &global in &globals {
        if !matches!(
            mir.symbols[global.index()].kind,
            SymbolKind::Declaration(BindingKind::Native | BindingKind::Decl)
        ) {
            continue;
        }
        let declaration = *mir.symbols[global.index()]
            .declarations
            .last()
            .expect("global declaration");
        emitter.expression(declaration).map_err(|d| vec![d])?;
    }
    for global in globals {
        if matches!(
            mir.symbols[global.index()].kind,
            SymbolKind::Declaration(BindingKind::Native | BindingKind::Decl)
        ) {
            continue;
        }
        let declaration = *mir.symbols[global.index()]
            .declarations
            .last()
            .expect("global declaration");
        let mut thunk = Emitter::new(mir, &graph, format!("global:{}", global.index()));
        let mut captures = vec![];
        for reference in referenced_globals(mir, declaration) {
            if let Some(value) = emitter.lookup(reference) {
                let register = thunk.register();
                thunk.locals.push((reference, register));
                captures.push(value);
            }
        }
        thunk.function.capture_count = captures.len() as u32;
        let value = thunk.expression(declaration).map_err(|d| vec![d])?;
        thunk.emit(declaration, O::Return { src: value });
        let dst = emitter.register();
        emitter.emit(
            declaration,
            O::MakeClosure {
                dst,
                function: Box::new(thunk.function),
                captures,
            },
        );
        let node = graph
            .global(global)
            .ok_or_else(|| vec![emitter.error(declaration, "global has no execution slot")])?;
        emitter.emit(declaration, O::InstallTask { node, src: dst });
    }
    let result = if let Some(value) = emitter.lookup(target) {
        value
    } else {
        let dst = emitter.register();
        let node = graph
            .global(target)
            .ok_or_else(|| vec![emitter.error(declaration, "entry has no execution slot")])?;
        emitter.emit(declaration, O::Demand { dst, node });
        dst
    };
    emitter.emit(declaration, O::Return { src: result });
    let result_type = emitter.ty(declaration).map_err(|d| vec![d])?;
    let bytecode = lir::assemble(emitter.function).map_err(|e| {
        vec![Diagnostic::error(
            e.message,
            mir.hir[declaration.index()].location,
        )]
    })?;
    Ok(CompiledEntry {
        symbol: entry,
        result_type,
        bytecode,
        native_links: emitter.native_links,
        types,
        eval_call: None,
        data_links: emitter.data_links,
        graph,
    })
}

fn runtime_children(mir: &Mir, node: HirId) -> impl Iterator<Item = HirId> + '_ {
    mir.hir[node.index()]
        .children
        .iter()
        .filter(move |edge| {
            // External declarations store signatures on their Value edges.
            // Their executable values are supplied by linking.
            !matches!(
                mir.member_selections[node.index()],
                Some(MemberSelection::EnumVariant { .. } | MemberSelection::Boolean(_))
            ) && !matches!(
                mir.hir[node.index()].kind,
                HirKind::Binding {
                    kind: BindingKind::Native | BindingKind::Decl,
                    ..
                }
            ) && !matches!(
                edge.role,
                Role::Annotation
                    | Role::TypeParameter
                    | Role::Bound
                    | Role::ReturnType
                    | Role::Decorator
                    | Role::Name
                    | Role::Target
            ) && (!matches!(mir.hir[node.index()].kind, HirKind::TypeApply)
                || edge.role == Role::Callee)
        })
        .map(|edge| edge.node)
}

/// Discover code to compile, including function bodies. This is deliberately
/// not an initialization order: references in uncalled bodies do not demand values.
fn reachable_globals(mir: &Mir, root: SymbolId) -> Vec<SymbolId> {
    let mut seen = std::collections::BTreeSet::new();
    let mut pending = vec![root];
    while let Some(symbol) = pending.pop() {
        if !seen.insert(symbol) {
            continue;
        }
        let declaration = *mir.symbols[symbol.index()]
            .declarations
            .last()
            .expect("global declaration");
        pending.extend(referenced_globals(mir, declaration));
    }
    seen.into_iter().collect()
}

fn referenced_globals(mir: &Mir, root: HirId) -> Vec<SymbolId> {
    let mut seen = std::collections::BTreeSet::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        if let Some(slot) = mir.hir[node.index()].resolution
            && let ResolveState::Bound(target) = mir.resolve_slots[slot.index()]
        {
            let symbol = &mir.symbols[target.index()];
            if let Some(module) = symbol.module
                && symbol.scope.is_some()
                && symbol.scope == mir.module_scopes[module.index()]
                && matches!(symbol.kind, SymbolKind::Declaration(_))
            {
                seen.insert(target);
            }
        }
        pending.extend(runtime_children(mir, node));
    }
    seen.into_iter().collect()
}

struct Emitter<'a> {
    mir: &'a Mir,
    graph: &'a ExecutionGraph,
    function: Function,
    locals: Vec<(SymbolId, R)>,
    next_label: u32,
    native_links: Vec<NativeLink>,
    data_links: Vec<DataLink>,
}

impl<'a> Emitter<'a> {
    fn new(mir: &'a Mir, graph: &'a ExecutionGraph, name: String) -> Self {
        Self {
            mir,
            graph,
            function: Function {
                name,
                memoized_interpreter: false,
                parameter_count: 0,
                capture_count: 0,
                register_count: 0,
                constants: vec![],
                items: vec![],
            },
            locals: vec![],
            next_label: 0,
            native_links: vec![],
            data_links: vec![],
        }
    }
    fn error(&self, node: HirId, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(message, self.mir.hir[node.index()].location)
    }
    fn ty(&self, node: HirId) -> Result<TypeId, Diagnostic> {
        match self.mir.ty_slots[node.ty().index()] {
            TypeState::Known(id) => Ok(id),
            _ => Err(self.error(node, "codegen encountered an unclosed type slot")),
        }
    }
    fn child(&self, node: HirId, role: Role) -> HirId {
        self.mir.hir[node.index()]
            .children
            .iter()
            .find(|e| e.role == role)
            .expect("HIR child")
            .node
    }
    fn children(&self, node: HirId, role: Role) -> Vec<HirId> {
        self.mir.hir[node.index()]
            .children
            .iter()
            .filter(|e| e.role == role)
            .map(|e| e.node)
            .collect()
    }
    fn register(&mut self) -> R {
        let id = R(self.function.register_count);
        self.function.register_count += 1;
        id
    }
    fn emit(&mut self, node: HirId, value: O) {
        self.function.items.push(Item::Operation(WithOrigin {
            value,
            origin: Origin::Source(self.mir.hir[node.index()].location),
        }));
    }
    fn label(&mut self) -> LabelId {
        let id = LabelId(self.next_label);
        self.next_label += 1;
        id
    }
    fn mark(&mut self, label: LabelId) {
        self.function.items.push(Item::Label(label));
    }
    fn constant(&mut self, node: HirId, value: Constant) -> R {
        let constant = ConstantId(self.function.constants.len() as u32);
        self.function.constants.push(value);
        let dst = self.register();
        self.emit(node, O::LoadConst { dst, constant });
        dst
    }
    fn lookup(&self, symbol: SymbolId) -> Option<R> {
        self.locals
            .iter()
            .rev()
            .find(|(s, _)| *s == symbol)
            .map(|(_, r)| *r)
    }

    fn expression(&mut self, node: HirId) -> Result<R, Diagnostic> {
        self.ty(node)?;
        if let Some(slot) = self.mir.hir[node.index()].resolution {
            if let ResolveState::Bound(symbol) = self.mir.resolve_slots[slot.index()] {
                if self.lookup(symbol).is_none()
                    && let Some(node_id) = self.graph.global(symbol)
                {
                    let dst = self.register();
                    self.emit(node, O::Demand { dst, node: node_id });
                    return Ok(dst);
                }
                return self.lookup(symbol).ok_or_else(|| {
                    self.error(
                        node,
                        "codegen global/module binding emission is not implemented yet",
                    )
                });
            }
        }
        let result = match &self.mir.hir[node.index()].kind {
            HirKind::Binding {
                kind: BindingKind::Decl,
                ..
            } if matches!(
                self.mir.modules[self.mir.hir[node.index()].module.index()].state,
                ModuleState::Data { .. }
            ) =>
            {
                let module = self.mir.hir[node.index()].module;
                let symbol = self.mir.hir_symbols[node.index()].expect("data declaration");
                self.data_links.push(DataLink {
                    constant: self.function.constants.len(),
                    module,
                    name: self.mir.modules[module.index()].name.clone(),
                    ty: self.ty(node)?,
                    location: self.mir.hir[node.index()].location,
                });
                let value = self.constant(node, Constant::Placeholder);
                self.locals.push((symbol, value));
                value
            }
            HirKind::Field
                if matches!(
                    self.mir.member_selections[node.index()],
                    Some(MemberSelection::Boolean(_))
                ) =>
            {
                let Some(MemberSelection::Boolean(value)) =
                    self.mir.member_selections[node.index()]
                else {
                    unreachable!()
                };
                self.constant(
                    node,
                    Constant::Atom(crate::Atom::builtin(if value {
                        crate::BuiltinAtom::True
                    } else {
                        crate::BuiltinAtom::False
                    })),
                )
            }
            HirKind::Field
                if matches!(
                    self.mir.member_selections[node.index()],
                    Some(MemberSelection::RecordField)
                ) =>
            {
                let slot = self.mir.hir[node.index()]
                    .resolution
                    .expect("resolved field");
                let ResolveState::Member { receiver, name } = self.mir.resolve_slots[slot.index()]
                else {
                    unreachable!()
                };
                let HirKind::Name(name) = &self.mir.hir[name.index()].kind else {
                    unreachable!()
                };
                let field = name.clone();
                let dict = self.expression(receiver)?;
                let dst = self.register();
                self.emit(node, O::GetField { dst, dict, field });
                dst
            }
            HirKind::Field if self.mir.member_selections[node.index()].is_some() => {
                let Some(MemberSelection::EnumVariant { index }) =
                    self.mir.member_selections[node.index()]
                else {
                    unreachable!()
                };
                let ty = self.ty(node)?;
                let signature = &self.mir.types[ty.index()];
                let owner = if signature.constructor == TypeConstructor::Function {
                    *signature.arguments.last().expect("constructor result")
                } else {
                    ty
                };
                if self
                    .mir
                    .properties
                    .iter()
                    .any(|property| property.owner == owner)
                {
                    return Err(self.error(
                        node,
                        "enum property execution lowering is not implemented yet",
                    ));
                }
                let mut pending = vec![owner];
                while let Some(id) = pending.pop() {
                    let ty = &self.mir.types[id.index()];
                    if matches!(ty.constructor, TypeConstructor::Parameter(_)) {
                        return Err(self.error(
                            node,
                            "generic constructor type witness lowering is not implemented yet",
                        ));
                    }
                    pending.extend(ty.arguments.iter().copied());
                }
                let dst = self.register();
                if signature.constructor == TypeConstructor::Function {
                    let owner = *signature.arguments.last().expect("constructor result");
                    let mut nested =
                        Self::new(self.mir, self.graph, format!("variant:{}", node.index()));
                    nested.function.parameter_count = 1;
                    let payload = nested.register();
                    let result = nested.register();
                    nested.emit(
                        node,
                        O::MakeVariant {
                            dst: result,
                            ty: owner,
                            variant: index,
                            payload: Some(payload),
                        },
                    );
                    nested.emit(node, O::Return { src: result });
                    self.emit(
                        node,
                        O::MakeClosure {
                            dst,
                            function: Box::new(nested.function),
                            captures: vec![],
                        },
                    );
                } else {
                    self.emit(
                        node,
                        O::MakeVariant {
                            dst,
                            ty,
                            variant: index,
                            payload: None,
                        },
                    );
                }
                dst
            }
            HirKind::Binding {
                kind: BindingKind::Native,
                ..
            } => {
                let symbol = self.mir.hir_symbols[node.index()].expect("native declaration");
                let declaration = &self.mir.symbols[symbol.index()];
                let ty = &self.mir.types[self.ty(node)?.index()];
                if ty.constructor != TypeConstructor::Function {
                    return Err(
                        self.error(node, "native value linking requires a function signature")
                    );
                }
                let link = NativeLink {
                    constant: self.function.constants.len(),
                    symbol,
                    module: declaration
                        .module
                        .and_then(|m| self.mir.modules[m.index()].native.as_ref().map(|n| n.id)),
                    name: declaration.name.clone(),
                    arity: ty.arguments.len() - 1,
                    location: self.mir.hir[node.index()].location,
                };
                self.native_links.push(link);
                let value = self.constant(node, Constant::Placeholder);
                self.locals.push((symbol, value));
                value
            }
            HirKind::Int(value) => self.constant(node, Constant::Int(*value)),
            HirKind::Float(value) => self.constant(node, Constant::Float(*value)),
            HirKind::String(value) => self.constant(node, Constant::String(value.clone().into())),
            HirKind::Bytes(value) => self.constant(node, Constant::Bytes(value.clone().into())),
            HirKind::Dict => {
                let ty = self.ty(node)?;
                let supported = match self.mir.types[ty.index()].constructor {
                    TypeConstructor::Dict | TypeConstructor::Record(_) => true,
                    TypeConstructor::Nominal(symbol) => self
                        .mir
                        .type_definitions
                        .iter()
                        .any(|d| d.symbol == symbol && d.operation == TypeOperation::Struct),
                    _ => false,
                };
                if !supported {
                    return Err(self.error(node, "unsupported solved record construction type"));
                }
                if self
                    .mir
                    .properties
                    .iter()
                    .any(|property| property.owner == ty)
                {
                    return Err(self.error(
                        node,
                        "record property execution lowering is not implemented yet",
                    ));
                }
                let mut fields = vec![];
                for field in self.children(node, Role::Field) {
                    let Some(name) = self.mir.hir[field.index()]
                        .children
                        .iter()
                        .find(|e| e.role == Role::Name)
                        .map(|e| e.node)
                    else {
                        return Err(
                            self.error(field, "dictionary spread lowering is not implemented yet")
                        );
                    };
                    let HirKind::Name(name) = &self.mir.hir[name.index()].kind else {
                        unreachable!()
                    };
                    let name = name.clone();
                    let value = self.expression(self.child(field, Role::Value))?;
                    fields.push((name, value));
                }
                let dst = self.register();
                self.emit(node, O::MakeDict { dst, fields });
                dst
            }
            HirKind::Binding {
                kind: BindingKind::Let | BindingKind::Def,
                ..
            } => {
                let value = self.expression(self.child(node, Role::Value))?;
                if let Some(symbol) = self.mir.hir_symbols[node.index()] {
                    self.locals.push((symbol, value));
                }
                value
            }
            HirKind::Block => {
                let scope = self.locals.len();
                for binding in self.children(node, Role::Binding) {
                    self.expression(binding)?;
                }
                let value = self.expression(self.child(node, Role::Result))?;
                self.locals.truncate(scope);
                value
            }
            HirKind::Tuple | HirKind::Array => {
                let ty = &self.mir.types[self.ty(node)?.index()];
                if !matches!(
                    ty.constructor,
                    TypeConstructor::Tuple | TypeConstructor::Array
                ) {
                    return Err(
                        self.error(node, "type-valued syntax requires the type skeleton linker")
                    );
                }
                let items = self
                    .children(node, Role::Item)
                    .into_iter()
                    .map(|n| self.expression(n))
                    .collect::<Result<Vec<_>, _>>()?;
                let dst = self.register();
                self.emit(
                    node,
                    if matches!(self.mir.hir[node.index()].kind, HirKind::Tuple) {
                        O::MakeTuple { dst, items }
                    } else {
                        O::MakeArray { dst, items }
                    },
                );
                dst
            }
            HirKind::Binary(operator) => {
                if matches!(operator, B::And | B::Or) {
                    return Err(self.error(node, "short-circuit lowering is not implemented yet"));
                }
                let left = self.expression(self.child(node, Role::Left))?;
                let right = self.expression(self.child(node, Role::Right))?;
                let dst = self.register();
                let operation = match operator {
                    B::Add => O::Add { dst, left, right },
                    B::Subtract => O::Subtract { dst, left, right },
                    B::Multiply => O::Multiply { dst, left, right },
                    B::Divide => O::Divide { dst, left, right },
                    B::Remainder => O::Remainder { dst, left, right },
                    B::Equal => O::Equal { dst, left, right },
                    B::NotEqual => O::NotEqual { dst, left, right },
                    B::LessThan => O::LessThan { dst, left, right },
                    B::LessThanOrEqual => O::LessThanOrEqual { dst, left, right },
                    B::GreaterThan => O::LessThan {
                        dst,
                        left: right,
                        right: left,
                    },
                    B::GreaterThanOrEqual => O::LessThanOrEqual {
                        dst,
                        left: right,
                        right: left,
                    },
                    _ => {
                        return Err(
                            self.error(node, "binary operation lowering is not implemented yet")
                        );
                    }
                };
                self.emit(node, operation);
                dst
            }
            HirKind::If => {
                let condition = self.expression(self.child(node, Role::Condition))?;
                let dst = self.register();
                let otherwise = self.label();
                let done = self.label();
                self.emit(
                    node,
                    O::JumpIfFalse {
                        condition,
                        target: otherwise,
                    },
                );
                let value = self.expression(self.child(node, Role::Then))?;
                self.emit(node, O::Move { dst, src: value });
                self.emit(node, O::Jump { target: done });
                self.mark(otherwise);
                let value = self.expression(self.child(node, Role::Else))?;
                self.emit(node, O::Move { dst, src: value });
                self.mark(done);
                dst
            }
            HirKind::Closure => {
                let parameters = self.children(node, Role::Parameter);
                let mut nested =
                    Self::new(self.mir, self.graph, format!("closure:{}", node.index()));
                nested.function.parameter_count = parameters.len() as u32;
                for parameter in parameters {
                    let register = nested.register();
                    let symbol =
                        self.mir.hir_symbols[parameter.index()].expect("parameter SymbolId");
                    nested.locals.push((symbol, register));
                }
                // Scope ownership is already resolved. Capture only referenced
                // enclosing bindings, using the stable symbol identity.
                let mut pending = vec![node];
                let mut references = std::collections::BTreeSet::new();
                while let Some(n) = pending.pop() {
                    if let Some(slot) = self.mir.hir[n.index()].resolution
                        && let ResolveState::Bound(symbol) = self.mir.resolve_slots[slot.index()]
                        && self.lookup(symbol).is_some()
                    {
                        references.insert(symbol);
                    }
                    pending.extend(runtime_children(self.mir, n));
                }
                let captures = references
                    .into_iter()
                    .map(|symbol| {
                        let capture = self.lookup(symbol).unwrap();
                        let register = nested.register();
                        nested.locals.push((symbol, register));
                        capture
                    })
                    .collect::<Vec<_>>();
                nested.function.capture_count = captures.len() as u32;
                let result = nested.expression(self.child(node, Role::Body))?;
                if !nested.native_links.is_empty() {
                    return Err(self.error(
                        node,
                        "local native relocation lowering is not implemented yet",
                    ));
                }
                nested.emit(node, O::Return { src: result });
                let dst = self.register();
                self.emit(
                    node,
                    O::MakeClosure {
                        dst,
                        function: Box::new(nested.function),
                        captures,
                    },
                );
                dst
            }
            HirKind::Call => {
                let callee = self.expression(self.child(node, Role::Callee))?;
                let arguments = self
                    .children(node, Role::Argument)
                    .into_iter()
                    .map(|n| self.expression(n))
                    .collect::<Result<Vec<_>, _>>()?;
                let base = self.register();
                self.emit(
                    node,
                    O::Move {
                        dst: base,
                        src: callee,
                    },
                );
                for &argument in &arguments {
                    let dst = self.register();
                    self.emit(node, O::Move { dst, src: argument });
                }
                self.emit(
                    node,
                    O::Call {
                        base,
                        argument_count: arguments.len() as u32,
                    },
                );
                base
            }
            HirKind::TypeAscription => self.expression(self.child(node, Role::Value))?,
            HirKind::TypeApply => self.expression(self.child(node, Role::Callee))?,
            HirKind::Return => {
                let value = self.expression(self.child(node, Role::Value))?;
                self.emit(node, O::Return { src: value });
                value
            }
            _ => {
                return Err(self.error(
                    node,
                    format!(
                        "MIR codegen has no lowering yet for {:?}",
                        self.mir.hir[node.index()].kind
                    ),
                ));
            }
        };
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn execute(artifact: CompiledEntry) -> Result<crate::execution_link::SolvedExecution, String> {
        let linked = crate::execution_link::link_entry(artifact).map_err(|d| format!("{d:?}"))?;
        crate::Vm::new().execute_linked(
            linked,
            crate::Quota::with_fuel(10000),
            crate::DataLimits::default(),
            &mut crate::SourceDatabase::default(),
        )
    }

    #[test]
    fn global_result_is_computed_once_across_repeated_reads_and_calls() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        CALLS.store(0, Ordering::SeqCst);
        let mir = graph(
            "native tick: Fn() -> Int; def cached = tick(); def read: Fn() -> Int = fn() { cached }; export def answer = cached + read();",
            "",
        );
        let mut artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        artifact.bytecode = crate::execution_link::link_with(&artifact, |_| {
            Some(crate::NativeFunction::new("test.tick", 0, |ctx| {
                CALLS.fetch_add(1, Ordering::SeqCst);
                ctx.set_int(ctx.result(), 21)
            }))
        })
        .unwrap();
        artifact.native_links.clear();
        assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42));
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn global_demands_allow_recursive_functions_and_ignore_uncalled_dependencies() {
        for source in [
            "def recurse: Fn(Int) -> Int = fn(n) { if n > 0 { recurse(n - 1) } else { 42 } }; export def answer = recurse(5);",
            "def a: Int = (fn(f) { 42 })(fn(x: Int) { b }); def b: Int = a; export def answer = a;",
            "def unused: Int = 1 / 0; export def answer = if True { 42 } else { unused };",
        ] {
            let mir = graph(source, "");
            let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
            assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42));
        }
        let mir = graph("def a: Int = b; def b: Int = a; export def answer = a;", "");
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        let error = execute(artifact)
            .err()
            .expect("actual value cycle must fail");
        assert!(error.contains("cyclic demand"), "{error}");
    }
    #[test]
    fn execution_graph_uses_sealed_identities_and_one_slot_per_property_chain() {
        use crate::execution_graph::{ExecutionGraph, PropertyKey, Request, Task};
        let source = r#"
            import "./math" { config as first };
            import "./math" { config as second };
            @property(PropertyTarget.Type)
            type Label = struct { value: Int };
            def label: Fn(Type, Option(Label)) -> Label = fn(owner, previous) { { value: first } };
            @label
            @label
            type Item = struct { value: Int };
            export def answer = second;
        "#;
        let mut baseline = None;
        for order in [0, 1, 7] {
            let mir = graph_order(source, "export def config = 42;", order);
            let sealed = mir.seal().unwrap();
            let graph = ExecutionGraph::from_mir(&sealed);
            let symbol = |name: &str| {
                SymbolId(mir.symbols.iter().position(|s| s.name == name).unwrap() as u32)
            };
            assert_eq!(
                graph.global(symbol("first")),
                graph.global(symbol("config"))
            );
            assert_eq!(
                graph.global(symbol("second")),
                graph.global(symbol("config"))
            );
            assert!(graph.global(symbol("Item")).is_none());
            let chain = graph
                .nodes()
                .iter()
                .find_map(|node| match &node.task {
                    Task::Property { key, providers } if providers.len() == 2 => {
                        Some((*key, providers))
                    }
                    _ => None,
                })
                .expect("two decorators reduce into one property node");
            let record = mir
                .properties
                .iter()
                .find(|p| p.providers.len() == 2)
                .unwrap();
            assert_eq!(chain.1.as_ref(), record.providers);
            assert!(
                graph
                    .property(PropertyKey {
                        owner: chain.0.property,
                        ..chain.0
                    })
                    .is_none()
            );
            let property = graph.property(chain.0).unwrap();
            let config = graph.global(symbol("config")).unwrap();
            let mut execution = graph.evaluation();
            assert_eq!(execution.request(property), Ok(Request::Start));
            assert_eq!(execution.request(config), Ok(Request::Start));
            execution.complete(config, 42).unwrap();
            execution.complete(property, 42).unwrap();
            assert_eq!(execution.request(property), Ok(Request::Ready(&42)));
            let dump = format!("{graph:?}");
            if let Some(baseline) = &baseline {
                assert_eq!(&dump, baseline);
            } else {
                baseline = Some(dump);
            }
        }
    }

    fn graph(main: &str, math: &str) -> Mir {
        graph_order(main, math, 0)
    }
    fn graph_order(main: &str, math: &str, order: usize) -> Mir {
        let mut inventory = crate::static_sources::BUILTINS
            .iter()
            .map(|(name, _)| crate::module_resolve::ModuleSpec {
                name: (*name).into(),
                kind: ModuleKind::Source,
                native: crate::static_sources::native_module(name),
                implicit_imports: if *name == "std/prelude" {
                    vec![]
                } else {
                    vec!["std/prelude".into()]
                },
            })
            .collect::<Vec<_>>();
        for name in ["@src/main", "@src/math"] {
            inventory.push(crate::module_resolve::ModuleSpec {
                name: name.into(),
                kind: ModuleKind::Source,
                native: None,
                implicit_imports: vec!["std/prelude".into()],
            });
        }
        if order > 0 {
            inventory.reverse();
            let length = inventory.len();
            inventory.rotate_left(order % length);
        }
        let mut mir =
            crate::module_resolve::resolve(inventory, &["@src/main".into()], |_, name| {
                Ok(if name == "@src/main" {
                    main.into()
                } else if name == "@src/math" {
                    math.into()
                } else {
                    crate::static_sources::BUILTINS
                        .iter()
                        .find(|(n, _)| *n == name)
                        .unwrap()
                        .1
                        .into()
                })
            });
        crate::symbol_resolve::resolve(&mut mir);
        crate::type_resolve::resolve(&mut mir);
        mir
    }
    fn entry(mir: &Mir) -> SymbolId {
        let ModuleTarget::Bound(module) = mir.roots[0] else {
            panic!("root");
        };
        *mir.exports[module.index()]
            .iter()
            .find(|id| mir.symbols[id.index()].name == "answer")
            .unwrap()
    }
    #[test]
    fn sealed_full_build_is_independent_of_inventory_enumeration_order() {
        let main = "import \"./math\" { identity }; \
                    import \"std/array\" { map, fold }; \
                    type Tree = enum { Leaf(Int), Branch((Tree, Tree)) }; \
                    export def answer = fold(map([1, 2, 3], fn(x) { identity(x * 7) }), 0, fn(a, b) { a + b });";
        let math = "export def identity: for(T) Fn(T) -> T = fn(x) { x };";
        let baseline = graph_order(main, math, 0);
        let sealed = baseline.seal().unwrap();
        let expected_image = format!("{:?}", sealed.types());
        let artifact = compile(sealed, entry(&baseline)).unwrap();
        for order in [1, 7, 19] {
            let rebuilt = graph_order(main, math, order);
            let sealed = rebuilt.seal().unwrap();
            assert_eq!(sealed.mir().dump(), baseline.dump());
            assert_eq!(format!("{:?}", sealed.types()), expected_image);
            let rebuilt = compile(sealed, entry(&rebuilt)).unwrap();
            assert_eq!(
                format!("{:?}", rebuilt.bytecode),
                format!("{:?}", artifact.bytecode)
            );
            assert_eq!(
                format!("{:?}", rebuilt.native_links),
                format!("{:?}", artifact.native_links)
            );
        }
    }

    #[test]
    fn seal_rejection_preserves_the_diagnostic_graph() {
        let mut mir = graph("export def answer = missing;", "");
        let before = mir.dump();
        assert!(mir.seal().is_err());
        assert_eq!(mir.dump(), before);
        // Flags alone are not evidence that required slots were normalized.
        mir.diagnostics.clear();
        mir.type_unknowns.clear();
        mir.type_conflicts.clear();
        assert!(mir.seal().is_err());
    }
    #[test]
    fn executes_solved_mir_with_closures_captures_and_branches() {
        let mir = graph(
            "export def answer = (fn(x) { let twice = fn(y) { x + y }; if x > 0 { twice(x) } else { 0 } })(21);",
            "",
        );
        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        let artifact = mir
            .seal()
            .and_then(|sealed| compile(sealed, entry(&mir)))
            .unwrap();
        let result_type = artifact.result_type;
        let result = execute(artifact).unwrap();
        assert_eq!(result.value().as_int(), Some(42));
        assert_eq!(
            mir.types[result_type.index()].constructor,
            TypeConstructor::Int
        );
    }

    #[test]
    fn executes_imported_generic_definitions_using_resolved_ids() {
        let mir = graph(
            "import \"./math\" { identity as choose }; export def answer = choose(42);",
            "export def identity: for(T) Fn(T) -> T = fn(value) { value };",
        );
        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        let before = mir.dump();
        let artifact = mir
            .seal()
            .and_then(|sealed| compile(sealed, entry(&mir)))
            .unwrap();
        assert_eq!(mir.dump(), before);
        assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42));
    }

    #[test]
    fn codegen_rejects_invalid_or_unsupported_mir_and_leaves_runtime_failures_to_vm() {
        for source in [
            "export def answer = missing;",
            "export def answer = match 1 { 1 => 2, _ => 3 };",
        ] {
            let mir = graph(source, "");
            assert!(
                mir.seal()
                    .and_then(|sealed| compile(sealed, entry(&mir)))
                    .is_err()
            );
        }
        let mir = graph("export def answer = 1 / 0;", "");
        let artifact = mir
            .seal()
            .and_then(|sealed| compile(sealed, entry(&mir)))
            .unwrap();
        let error = execute(artifact).err().expect("division must fail");
        assert!(error.contains("division by zero"), "{error}");
    }

    #[test]
    fn links_native_higher_order_calls_after_codegen_without_recompiling() {
        let mir = graph(
            r#"
            import "std/array" { map as transform, fold };
            export def answer = fold(transform([1, 2, 3], fn(x) { x * 7 }), 0, fn(a, b) { a + b });
        "#,
            "",
        );
        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        let artifact = mir
            .seal()
            .and_then(|sealed| compile(sealed, entry(&mir)))
            .unwrap();
        assert_eq!(artifact.native_links.len(), 2);
        assert!(artifact.native_links.iter().all(|l| l.module == Some(5)));
        let bytecode = crate::execution_link::link_builtins(&artifact).unwrap();
        assert!(bytecode.shares_code_with(&artifact.bytecode));
        let types_storage = artifact.types.types.as_ptr();
        let definitions_storage = artifact.types.definitions.as_ptr();
        let result_type = artifact.result_type;
        let linked = crate::execution_link::link_entry(artifact).unwrap();
        drop(mir);
        let result = crate::Vm::new()
            .execute_linked(
                linked,
                crate::Quota::with_fuel(10000),
                crate::DataLimits::default(),
                &mut crate::SourceDatabase::default(),
            )
            .unwrap();
        assert_eq!(result.value().as_int(), Some(42));
        assert_eq!(result.result_type(), result_type);
        assert_eq!(result.types().types.as_ptr(), types_storage);
        assert_eq!(result.types().definitions.as_ptr(), definitions_storage);
        assert_eq!(
            result.types().types[result_type.index()].constructor,
            TypeConstructor::Int
        );
    }

    #[test]
    fn native_link_requires_an_admitted_binding_with_the_declared_arity() {
        let mir = graph(
            "native map: Fn(Int) -> Int; export def answer = map(1);",
            "",
        );
        let artifact = mir
            .seal()
            .and_then(|sealed| compile(sealed, entry(&mir)))
            .unwrap();
        assert!(crate::execution_link::link_builtins(&artifact).is_err());
        let errors = crate::execution_link::link_with(&artifact, |_| {
            Some(crate::NativeFunction::new("wrong", 2, |_| unreachable!()))
        })
        .unwrap_err();
        assert!(errors.iter().any(|d| d.message.contains("arity")));
    }

    #[test]
    fn records_and_nominal_configs_use_existing_vm_storage_and_field_operations() {
        for main in [
            "def config = { evaluate: fn(x) { if True { x + 1 } else { 0 } }, seed: 41 }; export def answer = config.evaluate(config.seed);",
            "import \"./math\" { Config }; def config: Config = { evaluate: fn(x) { x + 1 }, seed: 41 }; export def answer = config.evaluate(config.seed);",
        ] {
            let mir = graph(
                main,
                "export type Config = struct { seed: Int, evaluate: Fn(Int) -> Int };",
            );
            assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
            let before = mir.dump();
            let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
            assert_eq!(mir.dump(), before);
            let linked = crate::execution_link::link_entry(artifact).unwrap();
            let result = crate::Vm::new()
                .execute_linked(
                    linked,
                    crate::Quota::with_fuel(10000),
                    crate::DataLimits::default(),
                    &mut crate::SourceDatabase::default(),
                )
                .unwrap();
            assert_eq!(result.value().as_int(), Some(42));
        }
    }

    #[test]
    fn executes_the_standard_entry_main_wrapper_without_the_old_compiler() {
        let mir = graph(
            r#"
            import "std/entry" { main };
            import "std/value" { Value };
            import "std/array" { length };
            def evaluator = main({ sources: [], envs: [], args: True }, fn(ctx) {
                Value.Int(length(ctx.args) * 21)
            });
            export def answer = evaluator.evaluate({ sources: {}, env: {}, args: ["one", "two"] });
        "#,
            "",
        );
        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        let result_type = artifact.result_type;
        let linked = crate::execution_link::link_entry(artifact).unwrap();
        let result = crate::Vm::new()
            .execute_linked(
                linked,
                crate::Quota::with_fuel(10000),
                crate::DataLimits::default(),
                &mut crate::SourceDatabase::default(),
            )
            .unwrap();
        assert_eq!(result.to_json(result_type).unwrap(), "42");
    }

    #[test]
    fn executes_nominal_variants_with_solved_identity_and_first_class_constructors() {
        let mir = graph(
            "import \"./math\" { Choice as C }; \
             export def answer = (C.Missing, (fn(make) { make(42) })(C.Number));",
            "export type Choice = enum { Missing, Number(Int) };",
        );
        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        let expected = artifact.types.types[artifact.result_type.index()].arguments[0];
        // Bytecode cannot reconstruct missing static type data in an empty VM.
        assert!(crate::Vm::new().execute(&artifact.bytecode, 10000).is_err());
        let linked = crate::execution_link::link_entry(artifact).unwrap();
        drop(mir);
        let result = crate::Vm::new()
            .execute_linked(
                linked,
                crate::Quota::with_fuel(10000),
                crate::DataLimits::default(),
                &mut crate::SourceDatabase::default(),
            )
            .unwrap();
        let missing = result.value().sequence_get(0).unwrap();
        let number = result.value().sequence_get(1).unwrap();
        assert_eq!(missing.solved_type_id(), Some(expected));
        assert_eq!(number.solved_type_id(), Some(expected));
        assert_eq!(number.tagged_parts().unwrap().1.as_int(), Some(42));
        assert_eq!(result.types().variant(expected, 0).unwrap().name, "Missing");
        assert_eq!(result.types().variant(expected, 1).unwrap().name, "Number");
    }

    #[test]
    fn type_image_retains_recursive_and_generic_skeletons_without_mir() {
        let mir = graph(
            "type Pair(T) = struct { first: T, second: T }; \
             type Tree = enum { Leaf(Int), Branch((Tree, Tree)) }; \
             export def answer = 42;",
            "",
        );
        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        let before = mir.dump();
        let artifact = mir
            .seal()
            .and_then(|sealed| compile(sealed, entry(&mir)))
            .unwrap();
        assert_eq!(mir.dump(), before);
        assert_eq!(artifact.types.types.len(), mir.types.len());
        drop(mir);
        let image = &artifact.types;
        let pair = image
            .definitions
            .iter()
            .find(|d| d.name.ends_with("::Pair"))
            .unwrap();
        let parameter = pair.parameters[0];
        for member in &pair.members {
            assert_eq!(
                image.types[member.payload.unwrap().index()].constructor,
                TypeConstructor::Parameter(parameter)
            );
        }
        let tree = image
            .definitions
            .iter()
            .find(|d| d.name.ends_with("::Tree"))
            .unwrap();
        assert!(std::ptr::eq(image.definition(tree.symbol).unwrap(), tree));
        let branch = tree.members.iter().find(|m| m.name == "Branch").unwrap();
        let tuple = &image.types[branch.payload.unwrap().index()];
        assert_eq!(tuple.constructor, TypeConstructor::Tuple);
        assert_eq!(tuple.arguments.len(), 2);
        for &child in &tuple.arguments {
            assert_eq!(
                image.types[child.index()].constructor,
                TypeConstructor::Nominal(tree.symbol)
            );
        }
        assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42));
    }
}
