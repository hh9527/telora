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

#[path = "codegen/newtypes.rs"]
mod newtypes;
#[path = "codegen/interpreters.rs"]
mod interpreters;
#[path = "codegen/patterns.rs"]
mod patterns;
#[path = "codegen/local-instances.rs"]
mod local_instances;
#[path = "codegen/properties.rs"]
mod properties;
#[path = "codegen/run.rs"]
mod run;
pub use run::{RunCalls, RunContract, RunMode, compile_run};
pub(crate) use run::RunHostTypes;

pub struct CompiledEntry {
    pub graph: ExecutionGraph,
    pub root: CompilationRoot,
    pub result_type: TypeId,
    pub bytecode: BytecodeFunction,
    pub native_links: Vec<NativeLink>,
    pub types: crate::type_image::TypeImage,
    pub eval_call: Option<EvalCall>,
    pub run_calls: Option<RunCalls>,
    pub data_links: Vec<DataLink>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompilationRoot {
    Export(SymbolId),
    Check,
    Tests(ModuleId),
}

pub struct CompiledTests {
    pub bootstrap: CompiledEntry,
    pub plan: crate::test_plan::TestPlan,
}

pub fn compile_tests(sealed: SealedMir<'_>, module: ModuleId) -> Result<CompiledTests, Vec<Diagnostic>> {
    let plan = crate::test_plan::TestPlan::from_mir(&sealed, module)?;
    let bootstrap = compile_root(sealed, CompilationRoot::Tests(module))?;
    Ok(CompiledTests { bootstrap, plan })
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
    pub signature: TypeId,
    pub location: crate::source::Location,
}

pub fn compile(sealed: SealedMir<'_>, entry: SymbolId) -> Result<CompiledEntry, Vec<Diagnostic>> {
    compile_root(sealed, CompilationRoot::Export(entry))
}

pub fn compile_check(sealed: SealedMir<'_>) -> Result<CompiledEntry, Vec<Diagnostic>> {
    compile_root(sealed, CompilationRoot::Check)
}

fn compile_root(
    sealed: SealedMir<'_>,
    root: CompilationRoot,
) -> Result<CompiledEntry, Vec<Diagnostic>> {
    let graph = ExecutionGraph::from_mir(&sealed);
    let (mir, types) = sealed.into_parts();
    let (target, declaration, name) = if let CompilationRoot::Export(entry) = root {
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
        (Some(target), declaration, symbol.name.clone())
    } else {
        let module = if let CompilationRoot::Tests(module) = root { module } else {
            let ModuleTarget::Bound(module) = mir.roots[0] else { unreachable!("sealed root") };
            module
        };
        let (ModuleState::Source { body, .. } | ModuleState::Data { body }) =
            mir.modules[module.index()].state
        else {
            unreachable!("sealed module")
        };
        (None, body, if matches!(root, CompilationRoot::Tests(_)) { "<test bootstrap>" } else { "<session check>" }.into())
    };
    let mut emitter = Emitter::new(mir, &graph, name);
    if target.is_some_and(|symbol| !mir.symbol_generics[symbol.index()].is_empty() && mir.function_families[symbol.index()].is_none()) {
        return Err(vec![emitter.error(declaration, "runtime entry requires a concrete generic instance")]);
    }
    // Emit the admitted graph in its stable order. Entry selection controls
    // demand, not which solved definitions have executable tasks installed.
    let globals = graph
        .nodes()
        .iter()
        .filter_map(|node| match node.task {
            crate::execution_graph::Task::Global { symbol, .. } => Some(symbol),
            _ => None,
        })
        .collect::<Vec<_>>();
    // Native ABI values and injected data already exist before initialization.
    for &global in &globals {
        if !mir.symbol_generics[global.index()].is_empty() {
            continue;
        }
        if !matches!(
            mir.hir[mir.symbols[global.index()].declarations.last().expect("global declaration").index()].kind,
            HirKind::Binding { kind: BindingKind::Native | BindingKind::Decl, .. }
        ) {
            continue;
        }
        let declaration = *mir.symbols[global.index()]
            .declarations
            .last()
            .expect("global declaration");
        emitter.expression(declaration).map_err(|d| vec![d])?;
    }
    for &global in &globals {
        if let Some(family) = &mir.function_families[global.index()] {
            let declaration = *mir.symbols[global.index()].declarations.last().expect("family declaration");
            let mut thunk = Emitter::new(mir, &graph, format!("family:{}", global.index()));
            let result = thunk.register();
            match family {
                FunctionFamily::Alias(target) => {
                    let node = graph.global(*target).ok_or_else(|| vec![thunk.error(declaration, "family alias has no global execution node")])?;
                    thunk.emit(declaration, O::Demand { dst: result, node });
                }
                FunctionFamily::Variants { identity, instances } => {
                    let identity = if let Some(source) = identity {
                        let value = thunk.register();
                        let node = graph.global(*source).ok_or_else(|| vec![thunk.error(declaration, "restricted family has no identity source")])?;
                        thunk.emit(declaration, O::Demand { dst: value, node });
                        Some(value)
                    } else { None };
                    let mut variants = vec![];
                    for (arguments, instance) in instances {
                        let value = thunk.register();
                        let node = graph.instance(*instance).ok_or_else(|| vec![thunk.error(declaration, "family instance has no execution node")])?;
                        thunk.emit(declaration, O::Demand { dst: value, node });
                        variants.push((arguments.clone(), value));
                    }
                    thunk.emit(declaration, O::MakeFunctionFamily { dst: result, identity, variants });
                }
            }
            thunk.emit(declaration, O::Return { src: result });
            let function = emitter.register();
            emitter.emit(declaration, O::MakeClosure { dst: function, function: Box::new(thunk.function), captures: vec![] });
            emitter.emit(declaration, O::InstallTask { node: graph.global(global).expect("family node"), src: function });
            continue;
        }
        if matches!(
            mir.hir[mir.symbols[global.index()].declarations.last().expect("global declaration").index()].kind,
            HirKind::Binding { kind: BindingKind::Native | BindingKind::Decl, .. }
        ) {
            continue;
        }
        if !mir.symbol_generics[global.index()].is_empty() {
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
    for task in graph.nodes() {
        let crate::execution_graph::Task::Instance { instance, symbol, declaration } = task.task else { continue; };
        let mut thunk = Emitter::new(mir, &graph, task.label.clone());
        thunk.instance = Some(instance);
        if mir.symbols[symbol.index()].kind == SymbolKind::Declaration(BindingKind::Native) {
            let locals = emitter.locals.len();
            emitter.instance = Some(instance);
            let value = emitter.expression(declaration).map_err(|d| vec![d])?;
            emitter.instance = None;
            emitter.locals.truncate(locals);
            let capture = thunk.register();
            thunk.function.capture_count = 1;
            thunk.emit(declaration, O::Return { src: capture });
            let dst = emitter.register();
            emitter.emit(declaration, O::MakeClosure { dst, function: Box::new(thunk.function), captures: vec![value] });
            emitter.emit(declaration, O::InstallTask { node: graph.instance(instance).expect("native instance task"), src: dst });
            continue;
        }
        let mut captures = vec![];
        for reference in referenced_globals(mir, declaration) {
            if let Some(value) = emitter.lookup(reference) {
                let register = thunk.register();
                thunk.locals.push((reference, register));
                captures.push(value);
            }
        }
        thunk.function.capture_count = captures.len() as u32;
        let result = thunk.expression(declaration).map_err(|d| vec![d])?;
        thunk.emit(declaration, O::Return { src: result });
        let dst = emitter.register();
        emitter.emit(declaration, O::MakeClosure { dst, function: Box::new(thunk.function), captures });
        emitter.emit(declaration, O::InstallTask { node: graph.instance(instance).expect("instance task"), src: dst });
    }
    for record in mir.properties.iter().filter(|record| record.concrete) {
        emitter.property_thunk(record).map_err(|d| vec![d])?;
    }
    for check in mir.construction_checks.iter().filter(|check| check.concrete) {
        let mut thunk = Emitter::new(mir, &graph, format!("check:{}", check.checker.index()));
        thunk.instance = check.instance;
        let mut captures = vec![];
        for symbol in referenced_globals(mir, check.checker) {
            if let Some(value) = emitter.lookup(symbol) { let register = thunk.register(); thunk.locals.push((symbol, register)); captures.push(value); }
        }
        thunk.function.capture_count = captures.len() as u32;
        let value = thunk.expression(check.checker).map_err(|d| vec![d])?;
        thunk.emit(check.checker, O::Return { src: value });
        let dst = emitter.register();
        emitter.emit(check.checker, O::MakeClosure { dst, function: Box::new(thunk.function), captures });
        emitter.emit(check.checker, O::InstallTask { node: graph.construction_check(check.owner, check.site).expect("check task"), src: dst });
    }
    for &node in graph.initializers() {
        let dst = emitter.register();
        emitter.emit(declaration, O::Demand { dst, node });
    }
    let (result, result_type) = if let Some(target) = target {
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
        (result, emitter.ty(declaration).map_err(|d| vec![d])?)
    } else {
        let unit = mir
            .types
            .iter()
            .position(|ty| ty.constructor == TypeConstructor::Tuple && ty.arguments.is_empty())
            .ok_or_else(|| vec![emitter.error(declaration, "session root requires solved Unit")])?;
        let dst = emitter.register();
        emitter.emit(declaration, O::MakeTuple { dst, items: vec![] });
        (dst, TypeId(unit as u32))
    };
    emitter.emit(declaration, O::Return { src: result });
    let bytecode = lir::assemble(emitter.function).map_err(|e| {
        vec![Diagnostic::error(
            e.message,
            mir.hir[declaration.index()].location,
        )]
    })?;
    Ok(CompiledEntry {
        root,
        result_type,
        bytecode,
        native_links: emitter.native_links,
        types,
        eval_call: None,
        run_calls: None,
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
                Some(
                    MemberSelection::EnumVariant { .. }
                        | MemberSelection::NewtypeConstructor
                        | MemberSelection::TraitMember { .. }
                        | MemberSelection::Boolean(_)
                )
            ) && !matches!(
                mir.hir[node.index()].kind,
                HirKind::Binding {
                    kind: BindingKind::Native | BindingKind::Decl,
                    ..
                } | HirKind::TypeMetadata
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

fn referenced_globals(mir: &Mir, root: HirId) -> Vec<SymbolId> {
    let mut seen = std::collections::BTreeSet::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        if let Some(MemberSelection::TraitMember {
            implementation: Some(symbol),
            ..
        }) = mir.member_selections[node.index()]
        {
            seen.insert(symbol);
        }
        if let Some(slot) = mir.hir[node.index()].resolution
            && let ResolveState::Bound(target) = mir.resolve_slots[slot.index()]
        {
            let symbol = &mir.symbols[target.index()];
            if let Some(module) = symbol.module
                && symbol.scope.is_some()
                && symbol.scope == mir.module_scopes[module.index()]
                && matches!(
                    symbol.kind,
                    SymbolKind::Declaration(
                        BindingKind::Let
                            | BindingKind::Def
                            | BindingKind::Native
                            | BindingKind::Decl
                    )
                )
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
    instance: Option<GenericInstanceId>,
    function: Function,
    locals: Vec<(SymbolId, R)>,
    local_instances: Vec<(GenericInstanceId, R)>,
    return_boundary: Option<HirId>,
    next_label: u32,
    native_links: Vec<NativeLink>,
    data_links: Vec<DataLink>,
}

impl<'a> Emitter<'a> {
    fn new(mir: &'a Mir, graph: &'a ExecutionGraph, name: String) -> Self {
        Self {
            mir,
            graph,
            instance: None,
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
            local_instances: vec![],
            return_boundary: None,
            next_label: 0,
            native_links: vec![],
            data_links: vec![],
        }
    }
    fn error(&self, node: HirId, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(message, self.mir.hir[node.index()].location)
    }
    fn ty(&self, node: HirId) -> Result<TypeId, Diagnostic> {
        if let Some(instance) = self.instance {
            return self.mir.generic_instances[instance.index()].ty(node)
                .ok_or_else(|| self.error(node, "instance has no closed type for this node"));
        }
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
    fn construction_check(&mut self, node: HirId, owner: TypeId, site: PropertySite, value: R) {
        let Some(task) = self.graph.construction_check(owner, site) else { return };
        let callee = self.register();
        self.emit(node, O::Demand { dst: callee, node: task });
        let base = self.register();
        self.emit(node, O::Move { dst: base, src: callee });
        let argument = self.register();
        self.emit(node, O::Move { dst: argument, src: value });
        self.emit(node, O::Call { base, argument_count: 1 });
        let ok = self.constant(node, Constant::Atom(crate::Atom::builtin(crate::BuiltinAtom::Ok)));
        let success = self.register();
        self.emit(node, O::TaggedTagEquals { dst: success, value: base, tag: ok });
        let rejected = self.label();
        let done = self.label();
        self.emit(node, O::JumpIfFalse { condition: success, target: rejected });
        self.emit(node, O::Jump { target: done });
        self.mark(rejected);
        let blame = self.register();
        self.emit(node, O::GetTaggedPayload { dst: blame, value: base });
        self.emit(node, O::Raise { action: crate::ast::BlameAction::Raise, dst: blame, message: blame, subjects: vec![] });
        self.mark(done);
    }
    fn lookup(&self, symbol: SymbolId) -> Option<R> {
        self.locals
            .iter()
            .rev()
            .find(|(s, _)| *s == symbol)
            .map(|(_, r)| *r)
    }

    fn expression(&mut self, node: HirId) -> Result<R, Diagnostic> {
        self.expression_mode(node, false)
    }
    fn expression_mode(&mut self, node: HirId, tail: bool) -> Result<R, Diagnostic> {
        let tail = tail && self.mir.value_adjustments[node.index()].is_none();
        let value = self.expression_unadjusted(node, tail)?;
        self.adjust_value(node, value)
    }
    fn adjust_value(&mut self, node: HirId, value: R) -> Result<R, Diagnostic> {
        if let Some(slot) = self.mir.value_adjustments[node.index()] {
            let target = if let Some(instance) = self.instance {
                self.mir.generic_instances[instance.index()].adjustment(node)
                    .ok_or_else(|| self.error(node, "instance has no closed construction adjustment"))?
            } else {
                match self.mir.ty_slots[slot.index()] {
                    TypeState::Known(ty) => ty,
                    _ => return Err(self.error(node, "construction adjustment has no closed target")),
                }
            };
            self.construction_check(node, target, PropertySite::Type, value);
            let dst = self.register();
            self.emit(node, O::StampType { dst, src: value, ty: target });
            return Ok(dst);
        }
        Ok(value)
    }
    fn expression_unadjusted(&mut self, node: HirId, tail: bool) -> Result<R, Diagnostic> {
        self.ty(node)?;
        if let Some(owner) = self.newtype_owner(node)? {
            return self.newtype_constructor(node, owner);
        }
        if let Some(slot) = self.mir.hir[node.index()].resolution {
            if let ResolveState::Bound(symbol) = self.mir.resolve_slots[slot.index()] {
                let instance = if let Some(instance) = self.instance {
                    self.mir.generic_instances[instance.index()].reference(node)
                } else {
                    self.mir.generic_references[node.index()].and_then(GenericReference::instance)
                };
                if let Some(value) = instance.and_then(|instance| self.lookup_instance(instance)) {
                    return Ok(value);
                }
                if let Some(instance) = instance
                    && let Some(node_id) = self.graph.instance(instance)
                {
                    let dst = self.register();
                    let selected = &self.mir.generic_instances[instance.index()];
                    if self.mir.function_families[selected.symbol.index()].is_some() {
                        let family = self.register();
                        let arguments = self.mir.symbol_generics[selected.symbol.index()].iter().map(|parameter|
                            selected.arguments.iter().find(|(p, _)| p == parameter).expect("closed instance parameter").1).collect();
                        self.emit(node, O::Demand { dst: family, node: self.graph.global(selected.symbol).expect("family execution node") });
                        self.emit(node, O::SpecializeFunction { dst, family, arguments });
                    } else {
                        self.emit(node, O::Demand { dst, node: node_id });
                    }
                    return Ok(dst);
                }
                if matches!(self.mir.generic_references[node.index()], Some(GenericReference::Scheme { .. } | GenericReference::Quantified { .. })) {
                    if let Some(value) = self.lookup(symbol) { return Ok(value); }
                    let node_id = self.graph.global(symbol).ok_or_else(|| self.error(node, "quantified function has no family execution node"))?;
                    let dst = self.register();
                    self.emit(node, O::Demand { dst, node: node_id });
                    return Ok(dst);
                }
                if matches!(self.mir.generic_references[node.index()], Some(GenericReference::Instance(_))) {
                    return Err(self.error(node, "generic reference has no executable MIR instance"));
                }
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
            HirKind::Debug { message, expression } => {
                let value = self.expression(self.child(node, Role::Value))?;
                let location = self.mir.hir[node.index()].location;
                let source = self.mir.sources.get(location.source);
                self.emit(node, O::Debug {
                    value,
                    module: source.name.to_string(),
                    line: u32::try_from(source.position(location.start).line).unwrap_or(u32::MAX),
                    name: expression.clone(),
                    message: message.clone(),
                });
                value
            }
            HirKind::Index => {
                let receiver = self.child(node, Role::Receiver);
                if self.mir.types[self.ty(receiver)?.index()].constructor != TypeConstructor::Array
                {
                    return Err(self.error(node, "index lowering requires a solved Array"));
                }
                let array = self.expression(receiver)?;
                let index = self.expression(self.child(node, Role::Index))?;
                let dst = self.register();
                self.emit(node, O::GetArray { dst, array, index });
                dst
            }
            HirKind::TupleProjection(index) => {
                let index = *index;
                let tuple = self.expression(self.child(node, Role::Receiver))?;
                let dst = self.register();
                self.emit(node, O::ProjectTuple { dst, tuple, index });
                dst
            }
            HirKind::Field
                if matches!(
                    self.mir.member_selections[node.index()],
                    Some(MemberSelection::TraitMember { .. })
                ) =>
            {
                let specialized = self.instance.and_then(|id| self.mir.generic_instances[id.index()].implementation(node));
                let slot = if let Some(instance) = specialized.or(self.mir.implementation_instances[node.index()]) {
                    self.graph.instance(instance)
                } else if let Some(MemberSelection::TraitMember { implementation: Some(symbol), .. }) = self.mir.member_selections[node.index()]
                    && self.mir.symbol_generics[symbol.index()].is_empty() {
                    self.graph.global(symbol)
                } else {
                    None
                }.ok_or_else(|| {
                    self.error(node, "selected implementation has no execution slot")
                })?;
                let dict = self.register();
                self.emit(
                    node,
                    O::Demand {
                        dst: dict,
                        node: slot,
                    },
                );
                let name = self.child(node, Role::Name);
                let HirKind::Name(name) = &self.mir.hir[name.index()].kind else {
                    unreachable!()
                };
                let field = name.clone();
                let dst = self.register();
                self.emit(node, O::GetField { dst, dict, field });
                dst
            }
            HirKind::Match | HirKind::IfLet | HirKind::LetElse => self.pattern_branch(node, tail)?,
            HirKind::Panic => {
                let message = self.expression(self.child(node, Role::Value))?;
                self.emit(node, O::Panic { message });
                message
            }
            HirKind::Raise(action) => {
                let action = *action;
                let message = self.expression(self.child(node, Role::Value))?;
                let subjects = self
                    .children(node, Role::Subject)
                    .into_iter()
                    .map(|n| self.expression(n))
                    .collect::<Result<Vec<_>, _>>()?;
                let dst = self.register();
                self.emit(
                    node,
                    O::Raise {
                        action,
                        dst,
                        message,
                        subjects,
                    },
                );
                dst
            }
            HirKind::TypeMetadata => {
                let ty = &self.mir.types[self.ty(node)?.index()];
                if ty.constructor != TypeConstructor::TypeOf || ty.arguments.len() != 1 {
                    return Err(self.error(node, "type metadata has no solved represented type"));
                }
                let represented = ty.arguments[0];
                let mut pending = vec![represented];
                while let Some(id) = pending.pop() {
                    let ty = &self.mir.types[id.index()];
                    if matches!(ty.constructor, TypeConstructor::Parameter(_)) {
                        return Err(
                            self.error(node, "generic metadata requires a compiled type witness")
                        );
                    }
                    pending.extend(ty.arguments.iter().copied());
                }
                self.constant(node, Constant::SolvedType(represented))
            }
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
                    Some(MemberSelection::RecordField | MemberSelection::DictField)
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
                let mut pending = if crate::type_image::builtin_variant(
                    &self.mir.types[owner.index()].constructor,
                    index,
                )
                .is_some()
                {
                    vec![]
                } else {
                    vec![owner]
                };
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
                    nested.construction_check(node, owner, PropertySite::Variant(index), payload);
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
                if let Some(value) = self.property_native(node, symbol)? {
                    return Ok(value);
                }
                let declaration = &self.mir.symbols[symbol.index()];
                let ty = &self.mir.types[self.ty(node)?.index()];
                let link = NativeLink {
                    constant: self.function.constants.len(),
                    symbol,
                    module: declaration
                        .module
                        .and_then(|m| self.mir.modules[m.index()].native.as_ref().map(|n| n.id)),
                    name: declaration.name.clone(),
                    arity: ty.arguments.len() - 1,
                    signature: self.ty(node)?,
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
            HirKind::FieldProjection => {
                let ty = self.ty(node)?;
                let dict = self.expression(self.child(node, Role::Receiver))?;
                let mut fields = vec![];
                for (source, target) in self.children(node, Role::Name).into_iter().zip(self.children(node, Role::Target)) {
                    let HirKind::Name(source_name) = &self.mir.hir[source.index()].kind else { unreachable!() };
                    let HirKind::Name(target_name) = &self.mir.hir[target.index()].kind else { unreachable!() };
                    let field = source_name.clone();
                    let name = target_name.clone();
                    let dst = self.register();
                    self.emit(source, O::GetField { dst, dict, field });
                    fields.push((name, dst));
                }
                let dst = self.register();
                self.emit(node, O::MakeDict { dst, fields });
                self.construction_check(node, ty, PropertySite::Type, dst);
                self.emit(node, O::StampType { dst, src: dst, ty });
                dst
            }
            HirKind::Dict => {
                let ty = self.ty(node)?;
                let mut fields = vec![];
                let mut dicts = vec![];
                for field in self.children(node, Role::Field) {
                    let Some(name) = self.mir.hir[field.index()]
                        .children
                        .iter()
                        .find(|e| e.role == Role::Name)
                        .map(|e| e.node)
                    else {
                        if !fields.is_empty() {
                            let dst = self.register();
                            self.emit(node, O::MakeDict { dst, fields: std::mem::take(&mut fields) });
                            dicts.push(dst);
                        }
                        let spread = self.child(field, Role::Value);
                        let value = self.expression(self.child(spread, Role::Operand))?;
                        dicts.push(value);
                        continue;
                    };
                    let HirKind::Name(name) = &self.mir.hir[name.index()].kind else {
                        unreachable!()
                    };
                    let name = name.clone();
                    let value = self.expression(self.child(field, Role::Value))?;
                    fields.push((name, value));
                }
                let dst = self.register();
                if dicts.is_empty() {
                    self.emit(node, O::MakeDict { dst, fields });
                } else {
                    if !fields.is_empty() {
                        let part = self.register();
                        self.emit(node, O::MakeDict { dst: part, fields });
                        dicts.push(part);
                    }
                    self.emit(node, O::MergeDicts { dst, dicts });
                }
                self.construction_check(node, ty, PropertySite::Type, dst);
                self.emit(node, O::StampType { dst, src: dst, ty });
                dst
            }
            HirKind::Binding {
                kind: BindingKind::Let | BindingKind::Def | BindingKind::Impl,
                ..
            } => {
                if let Some(symbol) = self.mir.hir_symbols[node.index()]
                    && !self.mir.symbol_generics[symbol.index()].is_empty()
                    && self.graph.global(symbol).is_none() {
                    return self.local_instance_binding(node, symbol);
                }
                let value = self.expression(self.child(node, Role::Value))?;
                if let Some(symbol) = self.mir.hir_symbols[node.index()] {
                    if matches!(self.mir.hir[node.index()].kind, HirKind::Binding { kind: BindingKind::Def, .. })
                        && let Some(target) = self.lookup(symbol)
                    {
                        self.emit(node, O::SealFunc { target, source: value });
                        return Ok(target);
                    }
                    self.locals.push((symbol, value));
                }
                value
            }
            HirKind::Binding { kind: BindingKind::Decl, .. } => {
                let symbol = self.mir.hir_symbols[node.index()].expect("declaration SymbolId");
                self.lookup(symbol).ok_or_else(|| self.error(node, "local declaration has no function slot"))?
            }
            HirKind::Block => {
                let scope = self.locals.len();
                let instance_scope = self.local_instances.len();
                let bindings = self.children(node, Role::Binding);
                self.allocate_local_instances(node, &bindings);
                // Stable symbols and closed types identify the block-wide
                // function slots. Closures capture these handles before their
                // bodies are installed, supporting self and mutual recursion.
                for &binding in &bindings {
                    if matches!(self.mir.hir[binding.index()].kind, HirKind::Binding { kind: BindingKind::Def | BindingKind::Decl, .. })
                        && self.mir.types[self.ty(binding)?.index()].constructor == TypeConstructor::Function
                    {
                        let symbol = self.mir.hir_symbols[binding.index()].expect("function SymbolId");
                        if !self.mir.symbol_generics[symbol.index()].is_empty() { continue; }
                        if self.lookup(symbol).is_none() {
                            let dst = self.register();
                            self.emit(binding, O::AllocFunc { dst });
                            self.locals.push((symbol, dst));
                        }
                    }
                }
                for binding in bindings {
                    self.expression(binding)?;
                }
                let value = self.expression_mode(self.child(node, Role::Result), tail)?;
                self.locals.truncate(scope);
                self.local_instances.truncate(instance_scope);
                value
            }
            HirKind::InterpolatedString => {
                let parts = self.children(node, Role::Part).into_iter().map(|part| self.expression(part)).collect::<Result<Vec<_>, _>>()?;
                let dst = self.register();
                self.emit(node, O::InterpolateString { dst, parts });
                dst
            }
            HirKind::Tuple | HirKind::Array => {
                let ty = &self.mir.types[self.ty(node)?.index()];
                if !matches!(
                    ty.constructor,
                    TypeConstructor::Tuple | TypeConstructor::Array | TypeConstructor::Never
                ) {
                    return Err(
                        self.error(node, "type-valued syntax requires the type skeleton linker")
                    );
                }
                let tuple = matches!(self.mir.hir[node.index()].kind, HirKind::Tuple);
                let mut items = vec![];
                let mut parts = vec![];
                for item in self.children(node, Role::Item) {
                    if matches!(self.mir.hir[item.index()].kind, HirKind::Spread) {
                        if !items.is_empty() {
                            let dst = self.register();
                            let values = std::mem::take(&mut items);
                            self.emit(node, if tuple { O::MakeTuple { dst, items: values } } else { O::MakeArray { dst, items: values } });
                            parts.push(dst);
                        }
                        parts.push(self.expression(self.child(item, Role::Operand))?);
                    } else { items.push(self.expression(item)?); }
                }
                let dst = self.register();
                if parts.is_empty() {
                    self.emit(node, if tuple { O::MakeTuple { dst, items } } else { O::MakeArray { dst, items } });
                } else {
                    if !items.is_empty() {
                        let part = self.register();
                        self.emit(node, if tuple { O::MakeTuple { dst: part, items } } else { O::MakeArray { dst: part, items } });
                        parts.push(part);
                    }
                    self.emit(node, if tuple { O::ConcatTuples { dst, tuples: parts } } else { O::ConcatArrays { dst, arrays: parts } });
                }
                dst
            }
            HirKind::Unary(operator) => {
                use crate::ast::UnaryOperator;
                let operator = *operator;
                let operand = self.child(node, Role::Operand);
                let src = self.expression(operand)?;
                let dst = self.register();
                let instruction = match operator {
                    UnaryOperator::Negate => O::Negate { dst, src },
                    UnaryOperator::LogicalNot => O::LogicalNot { dst, src },
                    UnaryOperator::BitNot => O::BitNot { dst, src },
                    UnaryOperator::Not => match self.mir.types[self.ty(operand)?.index()].constructor {
                        TypeConstructor::Bool => O::LogicalNot { dst, src },
                        TypeConstructor::Int => O::BitNot { dst, src },
                        TypeConstructor::Never => return Ok(src),
                        _ => return Err(self.error(node, "! requires a solved Bool or Int operand")),
                    },
                };
                self.emit(node, instruction);
                dst
            }
            HirKind::Binary(B::StructUpdate) => {
                let ty = self.ty(node)?;
                let left = self.expression(self.child(node, Role::Left))?;
                let right = self.expression(self.child(node, Role::Right))?;
                let dst = self.register();
                self.emit(node, O::StructUpdate { dst, left, right });
                self.construction_check(node, ty, PropertySite::Type, dst);
                self.emit(node, O::StampType { dst, src: dst, ty });
                dst
            }
            HirKind::Binary(operator) => {
                if matches!(operator, B::And | B::Or) {
                    let is_and = *operator == B::And;
                    let left = self.expression(self.child(node, Role::Left))?;
                    let dst = self.register();
                    self.emit(node, O::Move { dst, src: left });
                    let rhs = self.label();
                    let done = self.label();
                    self.emit(
                        node,
                        O::JumpIfFalse {
                            condition: left,
                            target: if is_and { done } else { rhs },
                        },
                    );
                    if !is_and {
                        self.emit(node, O::Jump { target: done });
                    }
                    self.mark(rhs);
                    let right = self.expression(self.child(node, Role::Right))?;
                    self.emit(node, O::Move { dst, src: right });
                    self.mark(done);
                    return Ok(dst);
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
                    B::BitAnd => O::BitAnd { dst, left, right },
                    B::BitOr => O::BitOr { dst, left, right },
                    B::BitXor => O::BitXor { dst, left, right },
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
                let value = self.expression_mode(self.child(node, Role::Then), tail)?;
                self.emit(node, O::Move { dst, src: value });
                self.emit(node, O::Jump { target: done });
                self.mark(otherwise);
                let value = self.expression_mode(self.child(node, Role::Else), tail)?;
                self.emit(node, O::Move { dst, src: value });
                self.mark(done);
                dst
            }
            HirKind::Interpreter => self.interpreter(node)?,
            HirKind::Closure => {
                if matches!(self.mir.types[self.ty(node)?.index()].constructor, TypeConstructor::Quantified(_)) {
                    let dst = self.register();
                    self.emit(node, O::MakeFunctionFamily { dst, identity: None, variants: vec![] });
                    return Ok(dst);
                }
                let parameters = self.children(node, Role::Parameter);
                let mut nested =
                    Self::new(self.mir, self.graph, format!("closure:{}", node.index()));
                nested.instance = self.instance;
                let boundary = self.child(node, Role::ReturnType);
                nested.return_boundary = Some(boundary);
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
                let mut captures = references
                    .into_iter()
                    .map(|symbol| {
                        let capture = self.lookup(symbol).unwrap();
                        let register = nested.register();
                        nested.locals.push((symbol, register));
                        capture
                    })
                    .collect::<Vec<_>>();
                for instance in self.referenced_instances(node) {
                    if let Some(capture) = self.lookup_instance(instance) {
                        let register = nested.register();
                        nested.local_instances.push((instance, register));
                        captures.push(capture);
                    }
                }
                nested.function.capture_count = captures.len() as u32;
                let result = nested.expression_mode(self.child(node, Role::Body), self.mir.value_adjustments[boundary.index()].is_none())?;
                let result = nested.adjust_value(boundary, result)?;
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
                if let Some(value) = self.property_call(node)? {
                    return Ok(value);
                }
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
                    if tail { O::TailCall { base, argument_count: arguments.len() as u32 } }
                    else { O::Call { base, argument_count: arguments.len() as u32 } },
                );
                base
            }
            HirKind::CheckedCast => {
                let value = self.child(node, Role::Value);
                let source = self.ty(value)?;
                let target = self.mir.types[self.ty(node)?.index()].arguments[0];
                let src = self.expression(value)?;
                let dst = self.register();
                self.emit(node, O::CheckedCast { dst, src, source, target });
                dst
            }
            HirKind::TypeAscription => self.expression_mode(self.child(node, Role::Value), tail)?,
            HirKind::TypeApply => self.expression(self.child(node, Role::Callee))?,
            HirKind::Propagate => {
                let operand = self.child(node, Role::Operand);
                let tag = match self.mir.types[self.ty(operand)?.index()].constructor {
                    TypeConstructor::Option => "Some",
                    TypeConstructor::Result => "Ok",
                    _ => return Err(self.error(node, "propagation operand has no solved family")),
                };
                let value = self.expression(operand)?;
                let tag = self.constant(node, Constant::Atom(crate::Atom::named(tag)));
                let condition = self.register();
                self.emit(node, O::TaggedTagEquals { dst: condition, value, tag });
                let failure = self.label();
                let done = self.label();
                self.emit(node, O::JumpIfFalse { condition, target: failure });
                let dst = self.register();
                self.emit(node, O::GetTaggedPayload { dst, value });
                self.emit(node, O::Jump { target: done });
                self.mark(failure);
                self.emit(node, O::Return { src: value });
                self.mark(done);
                dst
            }
            HirKind::Return => {
                let tail = self.return_boundary.is_some_and(|boundary| self.mir.value_adjustments[boundary.index()].is_none());
                let mut value = self.expression_mode(self.child(node, Role::Value), tail)?;
                if let Some(boundary) = self.return_boundary { value = self.adjust_value(boundary, value)?; }
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
#[path = "codegen/tests.rs"]
pub(crate) mod tests;
