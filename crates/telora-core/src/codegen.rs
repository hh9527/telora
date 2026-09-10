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
use properties::native_abi;

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
    if target.is_some_and(|symbol| !mir.symbol_generics[symbol.index()].is_empty()) {
        return Err(vec![emitter.error(declaration, "runtime entry requires a concrete generic instance")]);
    }
    let mut globals = if let Some(target) = target {
        reachable_globals(mir, target)
    } else {
        graph
            .nodes()
            .iter()
            .filter_map(|node| match node.task {
                crate::execution_graph::Task::Global { symbol, .. } => Some(symbol),
                _ => None,
            })
            .collect()
    };
    let mut all = globals.into_iter().collect::<std::collections::BTreeSet<_>>();
    for check in mir.construction_checks.iter().filter(|check| check.concrete) {
        for symbol in referenced_globals(mir, check.checker) { all.extend(reachable_globals(mir, symbol)); }
    }
    include_instance_implementations(mir, &mut all);
    globals = all.into_iter().collect();
    let queries_properties = matches!(root, CompilationRoot::Check | CompilationRoot::Tests(_))
        || globals.iter().any(|s| {
            matches!(
                native_abi(mir, *s),
                Some((25, "get_type_prop" | "get_field_prop" | "get_variant_prop" | "evidence")) | Some((13, _)) | Some((7, "parse_with")) | Some((17, "schema_with"))
            )
        });
    if queries_properties {
        let mut all = globals
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        for provider in mir.properties.iter().flat_map(|p| &p.providers) {
            for symbol in referenced_globals(mir, *provider) {
                all.extend(reachable_globals(mir, symbol));
            }
        }
        include_instance_implementations(mir, &mut all);
        globals = all.into_iter().collect();
    }
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
        if !globals.contains(&symbol) { continue; }
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
    if queries_properties {
        for record in &mir.properties {
            emitter.property_thunk(record).map_err(|d| vec![d])?;
        }
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
        for node in graph.nodes().iter().filter(|_| root == CompilationRoot::Check) {
            let demand = match node.task {
                crate::execution_graph::Task::Global { symbol, .. } => {
                    if emitter.lookup(symbol).is_some() || !mir.symbol_generics[symbol.index()].is_empty() {
                        None
                    } else {
                        graph.global(symbol)
                    }
                }
                crate::execution_graph::Task::Instance { instance, .. } => graph.instance(instance),
                crate::execution_graph::Task::Property { key, .. } => graph.property(key),
                crate::execution_graph::Task::ConstructionCheck { owner, site, .. } => graph.construction_check(owner, site),
            };
            if let Some(node) = demand {
                let dst = emitter.register();
                emitter.emit(declaration, O::Demand { dst, node });
            }
        }
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

/// Discover code to compile, including function bodies. This is deliberately
/// not an initialization order: references in uncalled bodies do not demand values.
fn include_instance_implementations(mir: &Mir, globals: &mut std::collections::BTreeSet<SymbolId>) {
    loop {
        let before = globals.len();
        let implementations = mir.generic_instances.iter().filter(|instance| globals.contains(&instance.symbol))
            .flat_map(|instance| instance.implementations.iter().map(|(_, id)| mir.generic_instances[id.index()].symbol))
            .collect::<Vec<_>>();
        for symbol in implementations { globals.extend(reachable_globals(mir, symbol)); }
        if globals.len() == before { break; }
    }
}

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
                    self.mir.reference_instances[node.index()]
                };
                if let Some(value) = instance.and_then(|instance| self.lookup_instance(instance)) {
                    return Ok(value);
                }
                if let Some(instance) = instance
                    && let Some(node_id) = self.graph.instance(instance)
                {
                    let dst = self.register();
                    self.emit(node, O::Demand { dst, node: node_id });
                    return Ok(dst);
                }
                if !self.mir.symbol_generics[symbol.index()].is_empty()
                    && matches!(self.mir.symbols[symbol.index()].kind,
                        SymbolKind::Declaration(BindingKind::Def | BindingKind::Decl | BindingKind::Let | BindingKind::Impl))
                {
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
                let supported = match self.mir.types[ty.index()].constructor {
                    TypeConstructor::Dict | TypeConstructor::Record(_) | TypeConstructor::Unchecked => true,
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
                            self.emit(binding, O::AllocFunc { dst, static_id: None });
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
            HirKind::Closure => {
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
pub(crate) mod tests {
    use super::*;
    #[test]
    fn local_struct_update_chains_keep_their_nominal_type() {
        let mir = graph(r#"import "./math" {Point};
            export def answer = do { let point: Point = {x: 1}; let updated = point <~ {x: 2} <~ {x: 42}; updated.x };"#,
            r#"@check(fn(value) { let warning: Option(()) = warn!(blame!("checked", value.x)); Ok(()) }) type Point = struct {x: Int}; export {Point};"#);
        let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
        let result = execute(artifact).unwrap();
        assert_eq!(result.value().as_int(), Some(42));
    }

    #[test]
    fn json_schema_reports_recoverable_mapping_and_property_errors() {
        for (declaration, target, expected) in [
            ("", "Type", "JSON Schema cannot describe Type metadata"),
            ("", "Bytes", "Type Bytes has no JSON Schema mapping"),
            ("", "Fn(Int) -> Int", "Type Func has no JSON Schema mapping"),
            ("@json.rename_all(json.RenameCase.CamelCase) type Bad = struct {foo_bar: Int, fooBar: Int};", "Bad", "$.fooBar: duplicate external field name"),
            ("@json.untagged type Bad = enum {One, Two};", "Bad", "$: untagged Enum may contain at most one unit variant"),
            ("import \"std/string\" as string; @string.encode_by_display type Bad = struct(Int);", "Bad", "std/string.decode_by_parse and std/string.encode_by_display must be used together"),
        ] {
            let source = format!("import \"std/json\" as json; import \"std/_rt\" as rt; {declaration} export def answer = match rt.with_diagnostics(fn(n: Int) {{ json.schema(({target}).type) }})(0) {{ Err(errors) => errors[0].message, _ => \"unexpected success\" }};");
            let mir = graph(&source, "");
            let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
            let result = execute(artifact).unwrap_or_else(|e| panic!("{source}\n{e}"));
            assert_eq!(result.value().as_str().unwrap().as_str(), expected, "{source}");
        }
    }

    #[test]
    fn json_schema_consumes_solved_layouts_and_lazy_properties() {
        for (declarations, target, expected) in [
            ("", "Int", r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"integer"}"#),
            ("type Id = struct(Int);", "Id", r##"{"$defs":{"Type0":{"type":"integer"}},"$ref":"#/$defs/Type0","$schema":"https://json-schema.org/draft/2020-12/schema"}"##),
            ("@check(fn(value) { fail!(\"schema must not run construction checks\") }) type Id = struct(Int);", "Id", r##"{"$defs":{"Type0":{"type":"integer"}},"$ref":"#/$defs/Type0","$schema":"https://json-schema.org/draft/2020-12/schema"}"##),
            ("type Node = struct {value: Int, next: Option(Node)};", "Node", r##"{"$defs":{"Type0":{"additionalProperties":false,"properties":{"next":{"anyOf":[{"type":"null"},{"$ref":"#/$defs/Type0"}]},"value":{"type":"integer"}},"required":["value"],"type":"object"}},"$ref":"#/$defs/Type0","$schema":"https://json-schema.org/draft/2020-12/schema"}"##),
            ("", "(Int, String)", r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","maxItems":2,"minItems":2,"prefixItems":[{"type":"integer"},{"type":"string"}],"type":"array"}"#),
            ("@json.rename_all(json.RenameCase.CamelCase) type User = struct {user_name: String};", "User", r##"{"$defs":{"Type0":{"additionalProperties":false,"properties":{"userName":{"type":"string"}},"required":["userName"],"type":"object"}},"$ref":"#/$defs/Type0","$schema":"https://json-schema.org/draft/2020-12/schema"}"##),
            ("@json.untagged type Scalar = enum {Text(String), Empty};", "Scalar", r##"{"$defs":{"Type0":{"oneOf":[{"type":"null"},{"type":"string"}]}},"$ref":"#/$defs/Type0","$schema":"https://json-schema.org/draft/2020-12/schema"}"##),
            ("", "Result(Int, String)", r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","oneOf":[{"additionalProperties":false,"properties":{"Err":{"type":"string"}},"required":["Err"],"type":"object"},{"additionalProperties":false,"properties":{"Ok":{"type":"integer"}},"required":["Ok"],"type":"object"}]}"#),
            ("import \"std/string\" as string; @string.decode_by_parse @string.encode_by_display type Text = struct(Int);", "Text", r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","type":"string"}"#),
        ] {
            let source = format!("import \"std/json\" as json; {declarations} def schema = json.schema(({target}).type); def text = json.stringify(schema); def parsed = match json.parse(text) {{ Ok(value) => value, Err(error) => raise!(error) }}; export def answer = (text, schema == parsed);");
            let mir = graph(&source, "");
            let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
            drop(mir);
            let result = execute(artifact).unwrap_or_else(|e| panic!("{source}\n{e}"));
            assert_eq!(result.value().sequence_get(0).unwrap().as_str().unwrap().as_str(), expected, "{source}");
            assert_eq!(result.value().sequence_get(1).unwrap().as_atom().unwrap().as_str(), "True", "{source}");
        }
    }

    #[test]
    fn newtype_facets_and_patterns_consume_static_constructor_selections() {
        for body in [
            "let make = Box; (make(42).0, make(\"text\").0); 42",
            "let make: Fn(Int) -> Id = Id; make(42).0",
            "def make: for(T) Fn(T) -> Box(T) = Box; make@[Int](42).0",
            "let Wrapped(payload) = Wrapped(42); payload",
            "let wrapper: Box(Id) = Box(Id(42)); wrapper.0.0",
        ] {
            let mir = graph(&format!("type Id = struct(Int); type Box(T) = struct(T); type Wrapped = Id; export def answer = do {{ {body} }};"), "");
            let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{body}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
            drop(mir);
            let result = execute(artifact).unwrap_or_else(|e| panic!("{body}\n{e}"));
            assert_eq!(result.value().as_int(), Some(42), "{body}");
        }
        let mut mir = graph("type Id = struct(Int); export def answer = Id(42);", "");
        mir.seal().unwrap();
        let selection = mir.member_selections.iter().position(|selection| matches!(selection, Some(MemberSelection::NewtypeConstructor))).unwrap();
        let TypeState::Known(signature) = mir.ty_slots[selection] else { unreachable!() };
        let wrong_payload = mir.types[signature.index()].arguments[1];
        mir.types[signature.index()].arguments[0] = wrong_payload;
        assert!(mir.seal().is_err(), "a constructor signature must agree with its sealed payload layout");
    }

    #[test]
    fn dyn_projection_uses_ordinary_generic_bindings_and_solved_witnesses() {
        for source in [
            r#"import "std/dyn" as dyn;
                export def answer = match dyn.project@[Int](dyn.pack(Int.type, 42)) { Some(value) => value, None => 0 };"#,
            r#"import "std/dyn" {project as unpack, pack};
                export def answer = match unpack@[Int](pack(Int.type, 42)) { Some(value) => value, None => 0 };"#,
            r#"import "std/dyn" as dyn;
                def unpack: for(T) Fn(Dyn) -> Option(T) = fn(value) { dyn.project@[T](value) };
                export def answer = match unpack@[Int](dyn.pack(Int.type, 42)) { Some(value) => value, None => 0 };"#,
            r#"import "std/dyn" as dyn;
                def unpack: Fn(Dyn) -> Option(Int) = dyn.project;
                export def answer = match unpack(dyn.pack(Int.type, 42)) { Some(value) => value, None => 0 };"#,
            r#"import "std/dyn" as dyn; type A = struct {value: Int}; type B = struct {value: Int};
                def value: A = {value: 1};
                export def answer = if dyn.project@[B](dyn.pack(A.type, value)) == None { 42 } else { 0 };"#,
            r#"import "./math" as user;
                export def answer = user.project@[Int](42);"#,
        ] {
            let mir = graph(source, "export def project: for(T) Fn(T) -> T = fn(value) { value };");
            let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
            drop(mir);
            let result = execute(artifact).unwrap_or_else(|e| panic!("{source}\n{e}"));
            assert_eq!(result.value().as_int(), Some(42), "{source}");
        }
    }

    #[test]
    fn property_target_enum_computes_matches_reflects_and_reduces_all_categories() {
        let mir = graph(r#"
            import "std/type-property" as props;
            import "std/type-desc" as td;
            import "std/dyn" as dyn;
            import PropertyTarget.{Member as Both};
            def choose: Fn(Bool) -> PropertyTarget = fn(flag) { if flag { PropertyTarget.StructType } else { PropertyTarget.EnumType } };
            @property(PropertyTarget.Type) @property(choose(True)) @property(choose(False))
            @property(Both) @property(PropertyTarget.Field) @property(PropertyTarget.Variant)
            type Mark = struct {value: Int};
            def name: Fn(PropertyTarget) -> String = fn(value) { match value {
                PropertyTarget.Type => "type", PropertyTarget.StructType => "struct",
                PropertyTarget.EnumType => "enum", Both => "member",
                PropertyTarget.Field => "field", PropertyTarget.Variant => "variant",
            } };
            export def answer = match props.get_type_prop(Mark.type, PropertyAttr.type) {
                Some(attr) => if attr.bits == 63 && name(choose(False)) == "enum" && name(Both) == "member"
                    && td.kind(PropertyTarget.type) == td.TypeDescKind.Enum
                    && td.variants(PropertyTarget.type)[2].name == "Member"
                    && dyn.get_variant_index(dyn.pack(PropertyTarget.type, PropertyTarget.Variant)) == 5 { 42 } else { 0 },
                None => 0,
            };
        "#, "");
        let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
        let result = execute(artifact).unwrap();
        assert_eq!(result.value().as_int(), Some(42));
    }

    #[test]
    fn existing_record_values_keep_identity_while_fresh_literals_take_context() {
        for body in [
            "let raw = {value: 42}; [raw] != [item] && [item] != [raw] && (raw, 1) != (item, 1)",
            "let raw = [{value: 42}]; [...raw] != [item] && [item] != [...raw] && [...[{value: 42}]] == [item]",
            "let raw = [{value: 42}]; choose(raw, [item]) != [item] && choose_array(raw, item) != [item]",
            "choose_independent([{value: 42}], [item]) != [item] && choose([{value: 42}], [item]) == [item]",
            "let raw = if True { {value: 42} } else { {value: 42} }; [raw] != [item] && [{value: 42}] == [item]",
        ] {
            let mir = graph(&format!("type Item = struct {{value: Int}}; def item: Item = {{value: 42}}; def choose: for(T) Fn(T, T) -> T = fn(left, right) {{left}}; def choose_array: for(T) Fn(Array(T), T) -> Array(T) = fn(left, right) {{left}}; def choose_independent: for(A, B) Fn(A, B) -> A = fn(left, right) {{left}}; export def answer = {{ {body} }};"), "");
            let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{body}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
            let result = execute(artifact).unwrap();
            assert_eq!(result.value().as_atom().unwrap().as_str(), "True", "{body}");
        }
    }

    #[test]
    fn encoded_enum_values_equal_explicit_semantic_value_constructors() {
        let mir = graph("import \"std/codec\" as codec; import \"std/value\" { Value }; type Event = enum { Progress(Int), Finished }; def encoded = codec.encode(Value.type, Event.Progress(47)); def expected = Value.Object({Progress: Value.Int(47)}); export def answer = (encoded == expected, encoded, expected);", "");
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        let result = execute(artifact).unwrap();
        let encoded = result.value().sequence_get(1).unwrap();
        let expected = result.value().sequence_get(2).unwrap();
        assert_eq!(encoded.solved_type_id(), expected.solved_type_id());
        let (encoded_tag, encoded_fields) = encoded.tagged_parts().unwrap();
        let (expected_tag, expected_fields) = expected.tagged_parts().unwrap();
        assert_eq!(encoded_tag.as_atom(), expected_tag.as_atom());
        assert_eq!(encoded_fields.solved_type_id(), expected_fields.solved_type_id());
        let encoded_number = encoded_fields.dict_get("Progress").unwrap();
        let expected_number = expected_fields.dict_get("Progress").unwrap();
        assert_eq!(encoded_number.solved_type_id(), expected_number.solved_type_id());
        assert_eq!(encoded_number.tagged_parts().unwrap().1.runtime(), expected_number.tagged_parts().unwrap().1.runtime());
        assert_eq!(result.value().sequence_get(0).unwrap().as_atom().unwrap().as_str(), "True");
    }

    #[test]
    fn encoded_object_payload_witnesses_cover_nested_and_dictionary_outputs() {
        for (source, expected) in [
            ("{a: 47}", "Value.Object({a: Value.Int(47)})"),
            ("{let value: Dict(Int) = {a: 47}; value}", "Value.Object({a: Value.Int(47)})"),
            ("[{a: 47}]", "Value.Array([Value.Object({a: Value.Int(47)})])"),
            ("{a: {b: 47}}", "Value.Object({a: Value.Object({b: Value.Int(47)})})"),
            ("Event.Finished", "Value.String(\"Finished\")"),
        ] {
            let mir = graph(&format!("import \"std/codec\" as codec; import \"std/value\" {{ Value }}; type Event = enum {{ Finished }}; export def answer = codec.encode(Value.type, {source}) == {expected};"), "");
            let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
            let result = execute(artifact).unwrap();
            assert_eq!(result.value().as_atom().unwrap().as_str(), "True", "{source}");
        }
    }

    #[test]
    fn metadata_comparisons_execute_without_equating_type_witnesses() {
        for source in [
            "export def answer = if Int.type != String.type && Int.type == Int.type { 42 } else { 0 };",
            "type Choice = enum { Selected(Type), Empty }; def chosen = match Choice.Selected(Int.type) { Choice.Selected(value) => value, Choice.Empty => String.type }; export def answer = if chosen == Int.type { 42 } else { 0 };",
            "def matches = fn(value) { value == Int.type }; export def answer = if matches(Int.type) && !matches(String.type) { 42 } else { 0 };",
        ] {
            let mir = graph(source, "");
            let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
            let result = execute(artifact).unwrap();
            assert_eq!(result.value().as_int(), Some(42), "{source}");
        }
    }

    #[test]
    fn nullary_constructor_aliases_have_independent_closed_owners() {
        for source in [
            "import \"./math\" { Empty }; export def answer = (Empty@[Int], Empty@[String]);",
            "import \"./math\" { Message }; export def answer = { import Message.{Empty}; (Empty@[Int], Empty@[String]) };",
            "export def answer = { import Option.{None as Empty}; (Empty@[Int], Empty@[String]) };",
        ] {
            let mir = graph(source, "export type Message(T) = enum { Data(T), Empty }; import Message.{Empty}; export { Empty };");
            let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
            let types = artifact.types.types[artifact.result_type.index()].arguments.clone();
            assert_ne!(types[0], types[1]);
            let nominal = matches!(artifact.types.types[types[0].index()].constructor, TypeConstructor::Nominal(_));
            let result = execute(artifact).unwrap();
            if nominal {
                for (index, ty) in types.into_iter().enumerate() {
                    assert_eq!(result.value().sequence_get(index).unwrap().solved_type_id(), Some(ty));
                }
            } else {
                for index in 0..2 {
                    assert_eq!(result.value().sequence_get(index).unwrap().as_atom().unwrap().as_str(), "None");
                }
            }
        }
    }

    #[test]
    fn generalized_function_aliases_preserve_local_captures() {
        for source in [
            "def identity = fn(value) { value }; def alias = identity; export def answer = (alias(21), alias(True));",
            "export def answer = { let identity = fn(value) { value }; let alias = identity; (alias(21), alias(True)) };",
            "export def answer = { let offset = 1; let identity = fn(value) { if offset == 1 { value } else { value } }; let alias = identity; (alias(21), alias(True)) };",
        ] {
            let mir = graph(source, "");
            let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
            let result = execute(artifact).unwrap();
            assert_eq!(result.value().sequence_get(0).unwrap().as_int(), Some(21));
            assert_eq!(result.value().sequence_get(1).unwrap().runtime().value(), crate::heap::DecodedValue::BuiltinAtom(crate::BuiltinAtom::True));
        }
    }

    #[test]
    fn imported_generic_constructor_aliases_execute_closed_instances() {
        for source in [
            "import \"./math\" { Message }; import Message.{Data}; export def answer = (Data(1), Data@[String](\"text\"));",
            "import \"./math\" { Make }; export def answer = (Make(1), Make@[String](\"text\"));",
            "export def answer = { import Option.{Some as Make}; (Make(1), Make@[String](\"text\")) };",
            "export def answer = (Option.Some@[Int](1), Option.Some@[String](\"text\"));",
            "import \"./math\" { Message }; export def answer = (Message.Data@[Int](1), Message.Data@[String](\"text\"));",
        ] {
            let mir = graph(source, "export type Message(T) = enum { Data(T), Empty }; import Message.{Data as Make}; export { Make };");
            let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
            let items = artifact.types.types[artifact.result_type.index()].arguments.clone();
            assert_ne!(items[0], items[1], "{source}");
            let nominal = matches!(artifact.types.types[items[0].index()].constructor, TypeConstructor::Nominal(_));
            let result = execute(artifact).unwrap();
            let first = result.value().sequence_get(0).unwrap();
            let second = result.value().sequence_get(1).unwrap();
            assert_eq!(first.tagged_parts().unwrap().1.as_int(), Some(1), "{source}");
            assert_eq!(second.tagged_parts().unwrap().1.as_str().unwrap().as_ref(), "text", "{source}");
            if nominal {
                assert_eq!(first.solved_type_id(), Some(items[0]), "{source}");
                assert_eq!(second.solved_type_id(), Some(items[1]), "{source}");
            }
        }
    }

    #[test]
    fn first_and_cached_demands_preserve_initializer_origin_through_function_returns() {
        let mir = graph("import \"./math\" { original }; def echo: Fn(Int) -> Int = fn(value) { value }; export def answer = (echo(original), echo(original), original);", "export def original = -7;");
        let expected = mir.hir.iter().find(|node| matches!(node.kind, HirKind::Unary(crate::ast::UnaryOperator::Negate))).unwrap().location;
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        let result = execute(artifact).unwrap();
        for index in 0..3 {
            let value = result.value().sequence_get(index).unwrap();
            assert_eq!(value.as_int(), Some(-7));
            assert_eq!(value.runtime().loc(), Some(expected));
        }
        let mir = graph("import \"./math\" { Item, original }; def echo: Fn(Item) -> Item = fn(value) { value }; export def answer = (echo(original), original);", "export type Item = struct {value: Int}; export def original: Item = {value: 42};");
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        let result = execute(artifact).unwrap();
        let first = result.value().sequence_get(0).unwrap();
        let cached = result.value().sequence_get(1).unwrap();
        assert!(first.solved_type_id().is_some());
        assert_eq!(first.solved_type_id(), cached.solved_type_id());
        assert_eq!(first.runtime().value(), cached.runtime().value());
        assert_eq!(first.runtime().loc(), cached.runtime().loc());
    }

    #[test]
    fn tail_positions_use_existing_frame_replacement_without_skipping_followup_work() {
        for source in [
            "def count: Fn(Int) -> Int = fn(n) { if n == 0 { 42 } else { count(n - 1) } }; export def answer = count(2000);",
            "export def answer = { decl even: Fn(Int) -> Int; decl odd: Fn(Int) -> Int; def even = fn(n) { if n == 0 { 42 } else { odd(n - 1) } }; def odd = fn(n) { if n == 0 { 0 } else { even(n - 1) } }; even(2000) };",
            "def count: Fn(Int) -> Int = fn(n) { match n { 0 => 42, value => count(value - 1) } }; export def answer = count(2000);",
            "def count: Fn(Int) -> Int = fn(n) { if n == 0 { 42 } else { return count(n - 1); } }; export def answer = count(2000);",
            "def count: for(T) Fn(T, Int) -> T = fn(value, n) { if n == 0 { value } else { count(value, n - 1) } }; export def answer = count(42, 2000);",
            "def count: Fn(Int) -> Int = fn(n) { if n == 0 { 0 } else { count(n - 1) + 1 } }; export def answer = count(42);",
            "import \"std/_rt\" as rt; def count: Fn(Int) -> Int = fn(n) { if n == 0 { 42 } else { count(n - 1) } }; export def answer = match rt.with_diagnostics(count)(2000) { Ok((value, _)) => value, Err(_) => 0 };",
            "import \"std/_rt\" as rt; def count: Fn(Int) -> Int = fn(n) { if n == 0 { fail!(\"caught tail failure\") } else { count(n - 1) } }; export def answer = match rt.with_diagnostics(count)(2000) { Err(errors) => if errors[0].message == \"caught tail failure\" { 42 } else { 0 }, Ok(_) => 0 };",
        ] {
            let mir = graph(source, "");
            let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
            assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42), "{source}");
        }
        for body in ["raw()", "return raw();"] {
            let source = format!("@check(fn(value) {{ Err(blame!(\"must run return check\", value)) }}) type Item = struct {{x: Int}}; def raw: Fn() -> Unchecked(Item) = fn() {{ {{x: 0}} }}; def checked: Fn() -> Item = fn() {{ {body} }}; export def answer = checked();");
            let mir = graph(&source, "");
            let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
            assert!(execute(artifact).err().expect("return check rejects").to_string().contains("must run return check"));
        }
    }

    #[test]
    fn checked_cast_errors_distinguish_scalar_identity_and_nested_path() {
        for source in [
            "export def answer = if \"1\".cast!(Int) == Err(\"value must be Int, got String\") && 1.cast!(Float) == Err(\"value must be Float, got Int\") { 42 } else { 0 };",
            "type A = struct {value: Int}; type B = struct {value: Int}; def a: A = {value: 1}; export def answer = if a.cast!(B) == Err(\"value has a different declared type identity\") { 42 } else { 0 };",
            "type Address = struct {zip: Int}; type User = struct {address: Address}; export def answer = if {address: {zip: \"bad\"}}.cast!(User) == Err(\"value.address.zip must be Int, got String\") { 42 } else { 0 };",
            "type User = struct {id: Int, name: String}; export def answer = match {id: 42, name: \"Ada\"}.cast!(User) { Ok(user) => user.id, Err(_) => 0 };",
        ] {
            let mir = graph(source, "");
            let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
            assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42), "{source}");
        }
    }

    #[test]
    fn metadata_joins_execute_the_selected_original_witness() {
        let mir = graph("def choose = fn(flag: Bool) { if flag { Int.type } else { String.type } }; export def answer = (choose(True), choose(False));", "");
        let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
        let int = TypeId(artifact.types.types.iter().position(|ty| ty.constructor == TypeConstructor::Int).unwrap() as u32);
        let string = TypeId(artifact.types.types.iter().position(|ty| ty.constructor == TypeConstructor::String).unwrap() as u32);
        let result = execute(artifact).unwrap();
        assert_eq!(result.value().sequence_get(0).unwrap().represented_type_id(), Some(int));
        assert_eq!(result.value().sequence_get(1).unwrap().represented_type_id(), Some(string));
    }

    #[test]
    fn nested_callable_results_use_closed_instances() {
        for source in [
            "def invoke = fn(factory) { factory()() }; export def answer = invoke(fn() { fn() { 42 } });",
            "def invoke = fn(factory) { factory()() }; export def answer = if invoke(fn() { fn() { \"text\" } }) == \"text\" { invoke(fn() { fn() { 42 } }) } else { 0 };",
            "def invoke = fn(callback, value) { let saved = callback; saved(value) }; export def answer = invoke(fn(value: Int) { value + 1 }, 41);",
        ] {
            let mir = graph(source, "");
            let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
            let artifact = compile(sealed, entry(&mir)).unwrap();
            assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42));
        }
    }

    #[test]
    fn implicit_schemes_execute_closed_global_and_local_instances() {
        let mir = graph("import \"./math\" { identity }; export def answer = if identity(\"text\") == \"text\" { identity(42) } else { 0 };", "export def identity = fn(value) { value };");
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42));
        for source in [
            "def identity = fn(value) { value }; export def answer = if identity(\"text\") == \"text\" { identity(42) } else { 0 };",
            "export def answer = { let identity = fn(value) { value }; if identity(\"text\") == \"text\" && identity@[Int](3) == 3 { identity(42) } else { 0 } };",
            "export def answer = { def first = fn(value) { second(value) }; def second = fn(value) { value }; if first(\"text\") == \"text\" { first(42) } else { 0 } };",
            "export def answer = { let captured = 42; let keep = fn(value) { captured }; if keep(\"text\") == 42 { keep(True) } else { 0 } };",
            "def outer = fn(value) { let keep = fn(other) { value }; (keep(True), keep(\"text\")) }; export def answer = if outer(\"text\").0 == \"text\" { outer(42).1 } else { 0 };",
            "export def answer = { let identity = fn(value) { value }; let use_it = fn(value: Int) { identity(value) }; if identity(\"text\") == \"text\" { use_it(42) } else { 0 } };",
            "export def answer = { let identity = fn(value) { value }; if identity@[Int] == identity@[Int] { identity(42) } else { 0 } };",
        ] {
            let mir = graph(source, "");
            let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
            let artifact = compile(sealed, entry(&mir)).unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
            assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42), "{source}");
        }
    }

    #[test]
    fn propagation_uses_solved_families_and_nearest_function_boundary() {
        for source in [
            "def step: Fn(Option(Int)) -> Option(String) = fn(value) { value?; Some(\"ok\") }; export def answer = if step(None) == None && step(Some(1)) == Some(\"ok\") { 42 } else { 0 };",
            "def step: Fn(Result(Int, String)) -> Result(Bool, String) = fn(value) { value?; Ok(True) }; export def answer = if step(Err(\"bad\")) == Err(\"bad\") && step(Ok(1)) == Ok(True) { 42 } else { 0 };",
            "def step = fn(value: Option(Int)) { let item = { value? }; Some(item + 1) }; export def answer = if step(None) == None && step(Some(41)) == Some(42) { 42 } else { 0 };",
            "def outer: Fn(Option(Int)) -> Option(Option(Int)) = fn(value) { let inner: Fn(Option(Int)) -> Option(Int) = fn(item) { Some(item?) }; Some(inner(value)) }; export def answer = if outer(None) == Some(None) { 42 } else { 0 };",
            "def step: for(T) Fn(Result(T, String)) -> Result(T, String) = fn(value) { Ok(value?) }; export def answer = if step@[Int](Err(\"bad\")) == Err(\"bad\") { step(Ok(42)).unwrap!() } else { 0 };",
            "def step: Fn(Result(Int, String)) -> Result((), String) = fn(value) { value?; fail!(\"unreachable\") }; export def answer = if step(Err(\"bad\")) == Err(\"bad\") { 42 } else { 0 };",
            "def step = fn(value: Result(Int, String)) { value?; fail!(\"unreachable\") }; def checked: Fn(Result(Int, String)) -> Result((), String) = step; export def answer = if checked(Err(\"bad\")) == Err(\"bad\") { 42 } else { 0 };",
        ] {
            let mir = graph(source, "");
            let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
            let artifact = compile(sealed, entry(&mir)).unwrap();
            assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42), "{source}");
        }
    }

    #[test]
    fn solved_parsers_and_diagnostics_use_closed_types() {
        for source in [
            "import \"std/json\" as json; import \"std/value\" {Value}; export def answer = match json.parse(\"42\") { Ok(Value.Int(n)) => n, _ => 0 };",
            "import \"std/yaml\" as yaml; import \"std/value\" {Value}; export def answer = match yaml.parse(\"42\") { Ok(Value.Int(n)) => n, _ => 0 };",
            "import \"std/toml\" as toml; import \"std/dict\" as dict; import \"std/value\" {Value}; export def answer = match toml.parse(\"n = 42\") { Ok(Value.Object(fields)) => match dict.get(fields, \"n\") { Some(Value.Int(n)) => n, _ => 0 }, _ => 0 };",
            "import \"std/json\" as json; export def answer = match json.parse(\"{\") { Err(_) => 42, Ok(_) => 0 };",
            "import \"std/json\" as json; import \"std/_rt\" as rt; import \"std/array\" as array; export def answer = match rt.with_diagnostics(fn(text: String) { json.parse(text).unwrap!() })(\"{\") { Err(errors) => if array.length(errors) == 1 { 42 } else { 0 }, Ok(_) => 0 };",
            "import \"std/_rt\" as rt; export def answer = match rt.with_diagnostics(fn(n: Int) { fail!(\"boom\") })(1) { Err(errors) => if errors[0].message == \"boom\" { 42 } else { 0 }, _ => 0 };",
        ] {
            let mir = graph(source, "");
            let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}")), entry(&mir)).unwrap_or_else(|d| panic!("{source}\n{d:?}"));
            assert_eq!(execute(artifact).unwrap_or_else(|d| panic!("{source}\n{d}")).value().as_int(), Some(42), "{source}");
        }
    }
    #[test]
    fn sealed_run_policy_configures_and_initializes_in_one_graph() {
        let mir = graph(
            r#"
            import "./math" as policy;
            import "std/entry" as entry;
            import "std/ees" as ees;
            import "std/_rt" as rt;
            def app = entry.run(Int.type, {sources: [], envs: [], args: False}, ees.none,
                fn(ctx) { (42, fn(state, event) { (state, []) }) });
            def main: policy.MainType = {config: app.config, ees: app.ees, start: app.start};
            def configured = policy.config({args: [], ees: {}, mode: rt.EntryMode.Run,
                platform: {os: "linux", arch: "x86_64"}, sources: {}}, main);
            def initialized = configured.1({data: {}, texts: {}, vars: {}, stdin: None}, main);
            def transition = initialized.1(initialized.0, rt.SystemEvent.Initialize);
            export def answer = if transition.0.completed == False { 42 } else { 0 };
            "#,
            include_str!("../modules/std/_entry/run.telora"),
        );
        let sealed = mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{:?}", mir.diagnostics));
        let artifact = compile(sealed, entry(&mir)).unwrap();
        assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42));
    }

    #[test]
    fn sealed_run_policy_emits_json_reply_and_exit() {
        let mir = graph(
            r#"
            import "./math" as policy;
            import "std/entry" as entry;
            import "std/ees" as ees;
            import "std/actor" as actor;
            import "std/value" {Value};
            import "std/_rt" as rt;
            def app = entry.run(Int.type, {sources: [], envs: [], args: False}, ees.none,
                fn(ctx) { (42, fn(state, event) {
                    (state, [actor.Effect.Reply({request_id: "run", value: Value.Int(state)})])
                }) });
            def main: policy.MainType = {config: app.config, ees: app.ees, start: app.start};
            def configured = policy.config({args: [], ees: {}, mode: rt.EntryMode.Run,
                platform: {os: "linux", arch: "x86_64"}, sources: {}}, main);
            def initialized = configured.1({data: {}, texts: {}, vars: {}, stdin: None}, main);
            def transition = initialized.1(initialized.0, rt.SystemEvent.Initialize);
            export def answer = if transition.0.completed {
                match (transition.1[0], transition.1[1]) {
                    (rt.SystemEffect.Output("42"), rt.SystemEffect.Exit(0)) => 42,
                    _ => 0,
                }
            } else { 0 };
            "#,
            include_str!("../modules/std/_entry/run.telora"),
        );
        let sealed = mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{:?}", mir.diagnostics));
        let artifact = compile(sealed, entry(&mir)).unwrap();
        assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42));
    }

    #[test]
    fn solved_json_formatters_read_the_original_value_graph() {
        let source = r#"
            import "std/json" as json;
            import "std/value" {Value};
            def input = Value.Object({a: Value.Array([Value.Int(42), Value.True]), b: Value.Object({})});
            export def answer = (json.stringify(input), json.stringify_pretty(2)(input), json.stringify_pretty(0)(input));
        "#;
        let mir = graph(source, "");
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        let result = execute(artifact).unwrap();
        for (index, expected) in [
            r#"{"a":[42,true],"b":{}}"#,
            "{\n  \"a\": [\n    42,\n    true\n  ],\n  \"b\": {}\n}",
            "{\n\"a\": [\n42,\ntrue\n],\n\"b\": {}\n}",
        ].iter().enumerate() {
            assert_eq!(result.value().sequence_get(index).unwrap().as_str().unwrap().as_str(), *expected);
        }
    }

    #[test]
    fn solved_type_desc_observes_static_bodies_and_applied_members() {
        for source in [
            r#"decl message: Fn(Int) -> String; def message = fn(n) { `value=\{n}` }; export def answer = if message(42) == "value=42" { 42 } else { 0 };"#,
            r#"import "std/type-desc" as td;
                type Box(T) = struct { value: T }; type Tree = enum { Empty, Branch(Array(Tree)) };
                def body = match td.resolve(Box(Int).type) { Ok(value) => value, Err(_) => fail!("resolve") };
                export def answer = if td.kind(Box(Int).type) == td.TypeDescKind.Ref && td.kind(body) == td.TypeDescKind.Struct && td.fields(body)[0].ty == Int.type { 42 } else { 0 };"#,
            r#"import "std/type-desc" as td;
                type Box(T) = struct(T);
                def body = match td.resolve(Box(Int).type) { Ok(value) => value, Err(_) => fail!("resolve") };
                export def answer = if td.kind(body) == td.TypeDescKind.Newtype && td.children(body)[0] == Int.type { 42 } else { 0 };"#,
            r#"import "std/type-desc" as td;
                type Tree(T) = enum { Empty, Branch(Array(Tree(T))), Leaf(T) };
                def body = match td.resolve(Tree(Int).type) { Ok(value) => value, Err(_) => fail!("resolve") };
                def variants = td.variants(body);
                export def answer = if variants[0].name == "Branch" && variants[0].payload == Some(Array(Tree(Int)).type) && variants[1].name == "Empty" && variants[1].payload == None && variants[2].payload == Some(Int.type) && td.kind(body) == td.TypeDescKind.Enum { 42 } else { 0 };"#,
            r#"import "std/type-desc" as td;
                def variants = td.variants(Result(Int, String).type);
                export def answer = if variants[0].name == "Err" && variants[0].payload == Some(String.type) && variants[1].name == "Ok" && variants[1].payload == Some(Int.type) { 42 } else { 0 };"#,
            r#"import "std/type-desc" as td; import "std/array" as array;
                export def answer = if td.kind(Int.type) == td.TypeDescKind.Int && td.variants(Option(Int).type)[1].payload == Some(Int.type) && array.length(td.children((Fn(Int) -> String).type)) == 0 { 42 } else { 0 };"#,
            r#"import "std/type-desc" as td; export def answer = match td.resolve(Int.type) { Err(_) => 42, _ => 0 };"#,
        ] {
            let mir = graph(source, "");
            assert!(mir.diagnostics.is_empty(), "{source}\n{:?}", mir.diagnostics);
            let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
            drop(mir);
            let result = execute(artifact).unwrap_or_else(|e| panic!("{source}\n{e}"));
            assert_eq!(result.value().as_int(), Some(42), "{source}");
        }
    }

    #[test]
    fn solved_prepared_display_uses_type_desc_and_dyn_member_ids() {
        let mir = graph(r#"
            import "std/fmt" as fmt; import "std/type-property" as properties; import "std/dyn" as dyn;
            @fmt.display_by("{host}:{port}") type Endpoint = struct { host: String, port: Int };
            def value: Endpoint = {host: "localhost", port: 8080};
            def property = match properties.get_type_prop(Endpoint.type, fmt.DisplayBy.type) { Some(p) => p, None => fail!("missing display") };
            export def answer = fmt.render(property.display(dyn.pack(Endpoint.type, value)));
        "#, "");
        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        drop(mir);
        let result = execute(artifact).unwrap();
        assert_eq!(result.value().as_str().unwrap().as_str(), "localhost:8080");
    }

    #[test]
    fn solved_dyn_members_consume_applied_layouts() {
        for source in [
            r#"import "std/dyn" as dyn; type Box(T) = struct { z: String, a: T };
                def value: Box(Int) = {a: 42, z: "other"};
                export def answer = take(dyn.project_with(Int.type, dyn.get_field_value(dyn.pack(Box(Int).type, value), 0)));"#,
            r#"import "std/dyn" as dyn; type Box(T) = struct(T);
                def items = get(dyn.tuple_items(dyn.pack(Box(Int).type, Box(42))));
                export def answer = take(dyn.project_with(Int.type, items[0]));"#,
            r#"import "std/dyn" as dyn; type Tree(T) = enum { Empty, Leaf(T), Branch(Array(Tree(T))) };
                def value = dyn.pack(Tree(Int).type, Tree.Leaf(42));
                def child = take(dyn.get_variant_payload(value, 2));
                export def answer = if dyn.get_variant_index(value) == 2 { take(dyn.project_with(Int.type, child)) } else { 0 };"#,
            r#"import "std/dyn" as dyn; type Item = enum { Empty, Full(Int) };
                def value = dyn.pack(Item.type, Item.Empty);
                export def answer = if dyn.get_variant_payload(value, 0) == None && dyn.kind(value) == dyn.ValueKind.Atom { 42 } else { 0 };"#,
            r#"import "std/dyn" as dyn; def value = dyn.pack(Dict(Int).type, {a: 42});
                export def answer = take(dyn.project_with(Int.type, get(dyn.field(value, "a"))));"#,
            r#"import "std/dyn" as dyn; def value = dyn.pack((Int, String).type, (42, "other"));
                export def answer = take(dyn.project_with(Int.type, get(dyn.tuple_items(value))[0]));"#,
            r#"import "std/dyn" as dyn; def value = dyn.pack(Array(Int).type, [42]);
                export def answer = take(dyn.project_with(Int.type, get(dyn.array_items(value))[0]));"#,
            r#"import "std/dyn" as dyn; def value = dyn.pack(Option(Int).type, Some(42));
                export def answer = if get(dyn.tag(value)) == "Some" { take(dyn.project_with(Int.type, take(get(dyn.payload(value))))) } else { 0 };"#,
            r#"import "std/dyn" as dyn; type Box(T) = struct { value: T };
                def value: Box(Int) = {value: 42}; def fields = get(dyn.fields(dyn.pack(Box(Int).type, value)));
                export def answer = if fields[0].0 == "value" { take(dyn.project_with(Int.type, fields[0].1)) } else { 0 };"#,
            r#"import "std/dyn" as dyn; export def answer = match dyn.field(dyn.pack(Int.type, 1), "missing") { Err(_) => 42, _ => 0 };"#,
        ] {
            let source = &format!("def take: for(T) Fn(Option(T)) -> T = fn(value) {{ match value {{ Some(value) => value, None => fail!(\"missing value\") }} }}; def get: for(T, E) Fn(Result(T, E)) -> T = fn(value) {{ match value {{ Ok(value) => value, Err(_) => fail!(\"access failed\") }} }}; {source}");
            let mir = graph(source, "");
            assert!(mir.diagnostics.is_empty(), "{source}\n{:?}", mir.diagnostics);
            let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
            drop(mir);
            let result = execute(artifact).unwrap_or_else(|e| panic!("{source}\n{e}"));
            assert_eq!(result.value().as_int(), Some(42), "{source}");
        }
    }

    #[test]
    fn solved_dyn_and_actor_service_keep_type_witnesses_in_the_vm() {
        for source in [
            "import \"std/dyn\" as dyn; export def answer = match dyn.project_with(Int.type, dyn.pack(Int.type, 42)) { Some(value) => value, None => 0 };",
            "import \"std/dyn\" as dyn; export def answer = match dyn.project_with(String.type, dyn.pack(Int.type, 1)) { Some(_) => 0, None => 42 };",
            "import \"std/dyn\" as dyn; type Alias = Int; export def answer = if dyn.desc(dyn.pack(Alias.type, 1)) == Int.type { 42 } else { 0 };",
            "import \"std/dyn\" as dyn; type Count = struct(Int); export def answer = match dyn.check_int(dyn.pack(Count.type, Count(1))) { Some(_) => 0, None => 42 };",
            "import \"std/dyn\" as dyn; type A = struct(Int); type B = struct(Int); export def answer = match dyn.project_with(B.type, dyn.pack(A.type, A(1))) { Some(_) => 0, None => 42 };",
            "import \"std/dyn\" as dyn; export def answer = match dyn.check_int(dyn.pack(Int.type, 42)) { Some(value) => value, None => 0 };",
            "import \"std/actor\" as actor; import \"std/value\" {Value}; import \"std/dyn\" as dyn; def service = actor.service(Array(Int).type, [42], fn(state, event) { (state, []) }); def transition = service.reduce((service.state, actor.Event.Request({id: \"request\", input: Value.Int(1)}))); export def answer = match dyn.project_with(Array(Int).type, transition.0) { Some(values) => values[0], None => 0 };",
            "import \"std/entry\" as entry; import \"std/ees\" as ees; import \"std/dyn\" as dyn; def app = entry.run(Int.type, { sources: [], envs: [], args: False }, ees.none, fn(ctx) { (42, fn(state, event) { (state, []) }) }); def service = app.start({ sources: {}, env: {}, args: [] }); export def answer = match dyn.project_with(Int.type, service.state) { Some(value) => value, None => 0 };",
        ] {
            let mir = graph(source, "");
            let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{:?}", mir.diagnostics));
            let artifact = compile(sealed, entry(&mir)).unwrap_or_else(|d| panic!("{source}\n{d:?}"));
            let result = execute(artifact).unwrap_or_else(|d| panic!("{source}\n{d}"));
            assert_eq!(result.value().as_int(), Some(42), "{source}");
        }
    }
    #[test]
    fn sequence_spreads_close_element_slots_and_preserve_evaluation_order() {
        for source in [
            "def empty = []; def a = [...empty, 42]; def b = [42, ...empty]; export def answer = if a == b { a[0] } else { 0 };",
            "def value = [...[[]], [42]]; export def answer = value[1][0];",
            "def value = (1, \"ok\"); export def answer = if (...(), ...value, 42, ...()) == (1, \"ok\", 42) { 42 } else { 0 };",
            "type Item = struct {value: Int}; def values: (Int, Item, String) = (...(1, {value: 42}), \"ok\"); export def answer = values.1.value;",
            "type Item = struct {value: Int}; def values: (Item, Int) = (...(...({value: 42},), 3)); export def answer = values.0.value;",
            "def append: for(T) Fn((T, String)) -> (T, String, Int) = fn(value) { (...value, 42) }; export def answer = append((1, \"ok\")).2;",
            "def copy: for(T) Fn(Array(T)) -> Array(T) = fn(value) { [...value] }; export def answer = copy([42])[0];",
        ] {
            let mir = graph(source, "");
            let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
            let artifact = compile(sealed, entry(&mir)).unwrap();
            assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42), "{source}");
        }
        for source in [
            "def stop: Fn() -> Array(Int) = fn() { fail!(\"first operand\") }; export def answer = [...stop(), fail!(\"later operand\")];",
            "def stop: Fn() -> (Int,) = fn() { fail!(\"first operand\") }; export def answer = (...stop(), fail!(\"later operand\"));",
            "export def answer = (...fail!(\"first operand\"), 42);",
        ] {
            let mir = graph(source, "");
            let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
            assert!(execute(artifact).err().expect("eager failure").to_string().contains("first operand"));
        }
    }

    #[test]
    fn record_spreads_keep_winning_types_and_evaluate_overwritten_expressions() {
        for source in [
            "type Full = struct {x: Int, label: String}; type Count = struct {x: Int}; type Wrong = struct {x: String}; def base: Full = {x: 1, label: \"base\"}; def count: Count = {x: 42}; def wrong: Wrong = {x: \"ignored\"}; export def answer = (base <~ {x: \"ignored\", ...count} <~ {...wrong, x: 42}).x;",
            "type Full = struct {x: Int, label: String}; type Count = struct {x: Int}; def count: Count = {x: 42}; def value: Full = {...count, label: \"ok\"}; export def answer = value.x;",
            "type Box(T) = struct {value: T}; def copy: for(T) Fn(Box(T)) -> Box(T) = fn(value) { {...value} }; def value: Box(Int) = {value: 42}; export def answer = copy(value).value;",
            "type Box(T) = struct {value: T}; decl copy: for(T) Fn(Box(T)) -> Box(T); def copy = fn(value) { {...value} }; def number: Box(Int) = {value: 42}; def text: Box(String) = {value: \"ok\"}; export def answer = if copy(text).value == \"ok\" { copy(number).value } else { 0 };",
            "def left: Dict(Int) = {x: 1}; def right: Dict(Int) = {x: 42}; def result = {...left, ...right}; export def answer = result.x;",
            "def result: Dict(Int) = {...{x: 42}}; export def answer = result.x;",
        ] {
            let mir = graph(source, "");
            let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
            let artifact = compile(sealed, entry(&mir)).unwrap();
            assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42), "{source}");
        }
        let source = "type Item = struct {x: Int}; def base: Item = {x: 42}; export def answer = base <~ {x: fail!(\"overwritten failure\"), ...base};";
        let mir = graph(source, "");
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        assert!(execute(artifact).err().expect("eager failure").to_string().contains("overwritten failure"));
    }

    #[test]
    fn struct_updates_preserve_identity_and_contextual_field_types() {
        for source in [
            "type Full = struct {x: Int, label: String}; type Patch = struct {x: Int}; def base: Full = {x: 1, label: \"base\"}; def patch: Patch = {x: 20}; def updated = base <~ patch <~ {x: 42}; export def answer = if base.x == 1 && updated.label == \"base\" { updated.x } else { 0 };",
            "type Child = struct {value: Int}; type Parent = struct {child: Child, items: Array(Int)}; def base: Parent = {child: {value: 1}, items: [1]}; def updated = base <~ {child: {value: 42}, items: []}; export def answer = updated.child.value;",
            "type Box(T) = struct {value: T}; def replace: for(T) Fn(Box(T), T) -> Box(T) = fn(base, value) { base <~ {value: value} }; def base: Box(Int) = {value: 1}; export def answer = replace(base, 42).value;",
            "type Source = struct {x: Int, y: String}; type Target = struct {value: Int}; def source: Source = {x: 42, y: \"x\"}; def base: Target = {value: 1}; export def answer = (base <~ source.{x as value}).value;",
        ] {
            let mir = graph(source, "");
            let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
            let artifact = compile(sealed, entry(&mir)).unwrap();
            assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42), "{source}");
        }
        let source = "@check(fn(value) { if value.x > 0 { Ok(()) } else { Err(blame!(\"update rejected\", value)) } }) type Item = struct {x: Int}; def base: Item = {x: 1}; export def answer = base <~ {x: 0};";
        let mir = graph(source, "");
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        assert!(execute(artifact).err().expect("construction rejection").to_string().contains("update rejected"));
    }

    #[test]
    fn field_projection_uses_solved_nominal_shapes_and_checks() {
        for source in [
            "type Source = struct {x: Int, y: String}; type Target = struct {value: Int}; def source: Source = {x: 42, y: \"x\"}; def selected: Target = source.{x as value}; export def answer = selected.value;",
            "type Source(T) = struct {value: T}; type Target(T) = struct {item: T}; def select: for(T) Fn(Source(T)) -> Target(T) = fn(value) { value.{value as item} }; def source: Source(Int) = {value: 42}; export def answer = select(source).item;",
            "type Source(T) = struct {value: T}; type Target(T) = struct {item: T}; export def answer = do { let source: Source(Int) = {value: 42}; let projected: Target(Int) = source.{value as item}; projected.item };",
            "type Source = struct {x: Int}; type Target = struct {a: Int, b: Int}; def source: Source = {x: 21}; def selected: Target = source.{x as a, x as b}; export def answer = selected.a + selected.b;",
            "type Source = struct {x: Int}; type Empty = struct {}; def source: Source = {x: 42}; def selected: Empty = source.{}; export def answer = if selected == {} { 42 } else { 0 };",
        ] {
            let mir = graph(source, "");
            let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
            let artifact = compile(sealed, entry(&mir)).unwrap();
            assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42), "{source}");
        }
        let source = "type Source = struct {x: Int}; @check(fn(value) { Err(blame!(\"projection rejected\", value)) }) type Target = struct {x: Int}; def source: Source = {x: 42}; export def answer: Target = source.{x};";
        let mir = graph(source, "");
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        assert!(execute(artifact).err().expect("construction rejection").to_string().contains("projection rejected"));
    }

    #[test]
    fn interpolation_resolves_display_calls_before_codegen() {
        for source in [
            r#"export def answer = if `n=\{42}` == "n=42" { 42 } else { 0 };"#,
            r#"import "std/fmt" as fmt; type Item = struct {value: Int}; impl fmt.Display for Item { display: fn(value) { fmt.from_string("item") } }; def value: Item = {value: 1}; export def answer = if `\{value}` == "item" { 42 } else { 0 };"#,
            r#"import "std/fmt" as fmt; def render: for(T: fmt.Display) Fn(T) -> String = fn(value) { `\{value}` }; export def answer = if render(42) == "42" && render("ok") == "ok" { 42 } else { 0 };"#,
            r#"def Display = 0; export def answer = if `\{42}` == "42" { 42 } else { 0 };"#,
        ] {
            let mir = graph(source, "");
            let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
            let artifact = compile(sealed, entry(&mir)).unwrap_or_else(|d| panic!("{source}\n{d:?}"));
            assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42), "{source}");
        }
        let missing = graph(r#"type Item = struct {value: Int}; def value: Item = {value: 1}; export def answer = `\{value}`;"#, "");
        assert!(missing.seal().is_err());
        assert!(missing.bound_requirements.iter().any(|bound| !bound.state.is_proven()));
    }

    #[test]
    fn property_evidence_demands_the_statically_proven_value() {
        let source = r#"
            import "std/type-property" as prop;
            @property(PropertyTarget.Type) type Mark = struct {value: Int};
            def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { {value: 42} };
            @mark type Item = struct {x: Int};
            def read: for(T: Property(Mark)) Fn(TypeOf(T)) -> Int = fn(owner) {
                let get = prop.evidence;
                get(owner, Mark.type).value
            };
            export def answer = read(Item.type);
        "#;
        let mir = graph(source, "");
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        assert_eq!(execute(artifact).unwrap().value().as_int(), Some(42));
        let missing = graph(&source.replace("@mark type Item", "type Item"), "");
        assert!(missing.seal().is_err());
        let failed = graph(&source.replace("{value: 42}", "fail!(\"provider failed\")"), "");
        let artifact = compile(failed.seal().unwrap(), entry(&failed)).unwrap();
        assert!(execute(artifact).err().expect("provider failure").to_string().contains("provider failed"));
    }

    #[test]
    fn block_bottoms_preserve_unit_tails_and_contextual_types() {
        for source in [
            "export def answer = if False { fail!(\"unreachable\"); } else { 42 };",
            "export def answer = if True { 42 } else { let x = fail!(\"unreachable\"); };",
            "export def answer = if True { 42 } else { let x: Int = fail!(\"unreachable\"); };",
            "def early: Fn() -> Int = fn() { 1; return 42; }; export def answer = early();",
            "def stop: Fn() -> Never = fn() { fail!(\"unreachable\") }; export def answer = if True { 42 } else { stop(); };",
            "def copy = fn(x) { let y = x; y }; export def answer = copy(42);",
            "def value: Fn() -> Int = fn() { let x = [42]; x[0] }; export def answer = value();",
            "export def answer = if (do {42;}) == () { 42 } else { 0 };",
        ] {
            let mir = graph(source, "");
            let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
            let artifact = compile(sealed, entry(&mir)).unwrap();
            let result = execute(artifact).unwrap();
            assert_eq!(result.value().as_int(), Some(42), "{source}");
        }
    }

    #[test]
    fn local_recursive_functions_capture_block_slots_and_invocation_values() {
        for source in [
            "export def answer = do { def down: Fn(Int) -> Int = fn(n) { if n == 0 { 42 } else { down(n - 1) } }; down(4) };",
            "export def answer = do { def even: Fn(Int) -> Bool = fn(n) { if n == 0 { True } else { odd(n - 1) } }; def odd: Fn(Int) -> Bool = fn(n) { if n == 0 { False } else { even(n - 1) } }; if even(4) && odd(3) { 42 } else { 0 } };",
            "def make: Fn(Int) -> Fn(Int) -> Int = fn(base) { def walk: Fn(Int) -> Int = fn(n) { if n == 0 { base } else { walk(n - 1) } }; walk }; def first = make(20); def second = make(22); export def answer = first(3) + second(4);",
            "export def answer = do { decl next: Fn(Int) -> Int; def next = fn(n) { if n == 0 { 42 } else { next(n - 1) } }; next(3) };",
        ] {
            let mir = graph(source, "");
            let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}\n{}", mir.dump()));
            let artifact = compile(sealed, entry(&mir)).unwrap_or_else(|d| panic!("{source}\n{d:?}"));
            let result = execute(artifact).unwrap_or_else(|d| panic!("{source}\n{d}"));
            assert_eq!(result.value().as_int(), Some(42), "{source}");
        }
    }

    #[test]
    fn unary_operators_consume_the_solved_operand_family() {
        for source in [
            "export def answer = 43 & 42;",
            "export def answer = 40 | 2;",
            "export def answer = 40 ^ 2;",
            "export def answer = if !False { 42 } else { 0 };",
            "export def answer = !(-43);",
            "export def answer = -(-42);",
            "export def answer = if -1.5 < 0.0 { 42 } else { 0 };",
            "def invert: Fn(Int) -> Int = fn(value) { !value }; export def answer = invert(-43);",
        ] {
            let mir = graph(source, "");
            let sealed = mir.seal().unwrap_or_else(|d| panic!("{source}\n{d:?}"));
            let artifact = compile(sealed, entry(&mir)).unwrap();
            let result = execute(artifact).unwrap();
            assert_eq!(result.value().as_int(), Some(42), "{source}");
        }
        let mir = graph("export def answer = !1.5;", "");
        assert!(mir.seal().is_err());
        assert!(mir.diagnostics.iter().any(|d| d.message == "Bool or Int operand required for !"));
    }

    #[test]
    fn newtypes_and_selected_trait_implementations_execute_from_solved_ids() {
        for source in [
            "type Count = struct(Int); def count = Count(42); export def answer = count.0;",
            "type Box(T) = struct(T); type IntBox = Box(Int); def read: for(T) Fn(Box(T)) -> T = fn(value) { value.0 }; export def answer = read(IntBox(42));",
            "type Inner = struct(Int); type Outer = struct(Inner); export def answer = Outer(Inner(42)).0.0;",
            "type Inner = struct(Array(Int)); type Outer = struct(Inner); def input = [20, 22]; export def answer = match Outer(Inner(input)) { Outer(Inner(items)) => items[0] + items[1] };",
            "type Box(T) = struct(T); type IntBox = Box(Int); export def answer = match IntBox(42) { IntBox(value) => value };",
            "type A = struct(Int); type B = struct(A); export def answer = if B(A(42)) == B(A(42)) { 42 } else { 0 };",
            "trait Name { name: Fn(Self) -> Int }; impl Name for Int { name: fn(value) { value + 1 } }; impl Name for String { name: fn(value) { 42 } }; export def answer = Name.name(\"input\");",
            "trait Name { name: Fn(Self) -> Int }; impl Name for Int { name: fn(value) { base + value } }; def base = 40; def method = Name.name; export def answer = method(2);",
            "trait Count { step: Fn(Self, Int) -> Int }; impl Count for Int { step: fn(value, n) { if n == 0 { value } else { Count.step(value + 1, n - 1) } } }; export def answer = Count.step(0, 42);",
            "@property(PropertyTarget.Type) type Tag = struct(Int); def tag: Fn(Type, Option(Tag)) -> Tag = fn(owner, previous) { Tag(1) }; @tag type Item = struct(Int); trait Name { name: Fn(Self) -> Int }; impl(T: Property(Tag)) Name for T { name: fn(value) { 42 } }; export def answer = Name.name(Item(1));",
        ] {
            let mir = graph(source, "");
            let sealed = mir
                .seal()
                .unwrap_or_else(|d| panic!("{source}\n{d:?}\n{:?}", mir.diagnostics));
            let artifact =
                compile(sealed, entry(&mir)).unwrap_or_else(|d| panic!("{source}\n{d:?}"));
            let result = execute(artifact).unwrap_or_else(|d| panic!("{source}\n{d}"));
            assert_eq!(result.value().as_int(), Some(42), "{source}");
        }
    }
    #[test]
    fn check_root_initializes_all_globals_and_properties_without_calling_functions() {
        for (source, expected) in [
            (
                "def unused = 1 / 0; export def answer = 42;",
                Some("division"),
            ),
            (
                "def unused: Fn() -> Int = fn() { fail!(\"do not call\") }; export def answer = 42;",
                None,
            ),
            (
                "@property(PropertyTarget.Type) type Mark = struct { value: Int }; def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { fail!(\"property root sentinel\") }; @mark type Item = struct { x: Int }; export def answer = 42;",
                Some("property root sentinel"),
            ),
            (
                "def a: Int = b; def b: Int = a; export def answer = 42;",
                Some("cyclic demand"),
            ),
        ] {
            let mut mir = graph(source, "");
            let artifact =
                compile_check(mir.seal().unwrap()).unwrap_or_else(|d| panic!("{source}\n{d:?}"));
            let linked = crate::execution_link::link_entry(artifact).unwrap();
            let diagnostics = crate::Vm::new().check_linked(
                linked,
                crate::Quota::with_fuel(10000),
                crate::DataLimits::default(),
                &mut mir.sources,
            );
            if let Some(message) = expected {
                assert!(
                    diagnostics.iter().any(|d| d.message.contains(message)),
                    "{source}\n{diagnostics:?}"
                );
            } else {
                assert!(diagnostics.is_empty(), "{diagnostics:?}");
            }
        }
    }
    #[test]
    fn solved_patterns_and_native_variants_execute_with_lexical_scopes() {
        for source in [
            "export def answer = match Some((20, 22)) { Some((x, y)) => x + y, None => 0 };",
            "export def answer = match Result(Int, String).Err(\"bad\") { Ok(x) => x, Err(\"bad\") => 42, _ => 0 };",
            "type E = enum { A(Int), B }; export def answer = match E.A(21) { E.B => 0, E.A(x) if x < 0 => 1, E.A(x) => x * 2 };",
            "export def answer = if let Some(x) = Some(42) { x } else { 0 };",
            "def absent: Option(Int) = None; export def answer = if let Some(x) = absent { x } else { 42 };",
            "export def answer = do { let Some(x) = Some(42) else { fail!(\"absent\") }; x };",
            "type Rec = struct { x: Int, y: String }; def v: Rec = { x: 42, y: \"unused\" }; export def answer = match v { { x: n } => n };",
            "import \"std/array\" { fold_control }; type Control = FoldControl(Int, Int); export def answer = match fold_control([20, 22, 99], 0, fn(a, b) { if a == 42 { Control.Break(a) } else { Control.Continue(a + b) } }) { Control.Break(x) => x, Control.Continue(x) => x };",
            "def crash: Fn() -> Bool = fn() { fail!(\"must remain lazy\") }; export def answer = if (False && crash()) || (True || crash()) { 42 } else { 0 };",
            "def f: Fn(Option(Int)) -> Int = fn(v) { let Some(x) = v else { return 42; }; x }; export def answer = f(None);",
            "export def answer = (match Some(21) { Some(x) if False => fn() { 0 }, Some(x) => fn() { x * 2 }, None => fn() { 0 } })();",
            "import \"std/option\" { map, unwrap_or }; export def answer = unwrap_or(map(Some(21), fn(x) { x * 2 }), 0);",
            "import \"std/prelude\" { Some as Present, None as Absent }; export def answer = match Present(42) { Present(x) => x, Absent => 0 };",
            "import \"std/type-property\" { get_type_prop }; @property(PropertyTarget.Type) type Mark = struct { value: Int }; def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { { value: 21 + match previous { Some(p) => p.value, None => 0 } } }; @mark @mark type Item = struct { x: Int }; export def answer = match get_type_prop(Item.type, Mark.type) { Some(p) => p.value, None => 0 };",
        ] {
            let mir = graph(source, "");
            let sealed = mir
                .seal()
                .unwrap_or_else(|d| panic!("{source}\n{d:?}\n{:?}", mir.diagnostics));
            let artifact =
                compile(sealed, entry(&mir)).unwrap_or_else(|d| panic!("{source}\n{d:?}"));
            let result = execute(artifact).unwrap_or_else(|d| panic!("{source}\n{d}"));
            assert_eq!(result.value().as_int(), Some(42), "{source}");
        }
    }
    #[test]
    fn record_pattern_fields_are_checked_before_codegen() {
        let mir = graph(
            "type Rec = struct { x: Int }; def v: Rec = { x: 1 }; export def answer = match v { { missing: n } => n };",
            "",
        );
        assert!(mir.seal().is_err());
        assert!(
            mir.diagnostics
                .iter()
                .any(|d| d.message.contains("missing")),
            "{:?}",
            mir.diagnostics
        );
    }
    #[test]
    fn member_properties_receive_solved_skeleton_contexts() {
        let mir = graph(
            r#"
            import "std/type-property" { get_field_prop, get_variant_prop, FieldPropertyCtx, VariantPropertyCtx };
            @property(PropertyTarget.Field) type FieldMark = struct { context: FieldPropertyCtx };
            @property(PropertyTarget.Variant) type VariantMark = struct { context: VariantPropertyCtx };
            def field: Fn(FieldPropertyCtx, Option(FieldMark)) -> FieldMark = fn(ctx, previous) { { context: ctx } };
            def variant: Fn(VariantPropertyCtx, Option(VariantMark)) -> VariantMark = fn(ctx, previous) { { context: ctx } };
            type Item = struct { @field first: Int, @field second: String };
            type Choice = enum { @variant Empty, @variant Value(Int) };
            export def answer = (get_field_prop(Item.type, 1, FieldMark.type),
                get_variant_prop(Choice.type, 0, VariantMark.type),
                get_variant_prop(Choice.type, 1, VariantMark.type), Item.type, Choice.type, String.type, Int.type);
        "#,
            "",
        );
        let result = execute(compile(mir.seal().unwrap(), entry(&mir)).unwrap()).unwrap();
        for (index, name, position, owner_index) in
            [(0, "second", 1, 3), (1, "Empty", 0, 4), (2, "Value", 1, 4)]
        {
            let (_, mark) = result
                .value()
                .sequence_get(index)
                .unwrap()
                .tagged_parts()
                .unwrap();
            let context = mark.dict_get("context").unwrap();
            assert_eq!(
                context.dict_get("name").unwrap().as_str().unwrap().as_str(),
                name
            );
            assert_eq!(context.dict_get("index").unwrap().as_int(), Some(position));
            assert_eq!(
                context.dict_get("owner").unwrap().represented_type_id(),
                result
                    .value()
                    .sequence_get(owner_index)
                    .unwrap()
                    .represented_type_id()
            );
            if index == 0 {
                assert_eq!(
                    context.dict_get("ty").unwrap().represented_type_id(),
                    result
                        .value()
                        .sequence_get(5)
                        .unwrap()
                        .represented_type_id()
                );
            } else if index == 1 {
                assert_eq!(
                    context
                        .dict_get("payload")
                        .unwrap()
                        .as_atom()
                        .unwrap()
                        .as_str(),
                    "None"
                );
            } else {
                let (_, payload) = context.dict_get("payload").unwrap().tagged_parts().unwrap();
                assert_eq!(
                    payload.represented_type_id(),
                    result
                        .value()
                        .sequence_get(6)
                        .unwrap()
                        .represented_type_id()
                );
            }
        }
    }

    #[test]
    fn property_queries_reduce_once_and_share_lazy_global_dependencies() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        CALLS.store(0, Ordering::SeqCst);
        let mir = graph(
            r#"
            import "std/type-property" { get_type_prop as query };
            native tick: Fn() -> Int;
            @property(PropertyTarget.Type)
            @property(PropertyTarget.Field)
            type Mark = struct { value: Int };
            def config = tick();
            def make: Fn(Int) -> Fn(Type, Option(Mark)) -> Mark = fn(n) {
                fn(owner, previous) { let counted = tick(); { value: config + n } }
            };
            @make(1)
            @make(2)
            type Item = struct { value: Int };
            export def answer = (query(Item.type, Mark.type), (fn(read) { read(Item.type, Mark.type) })(query),
                query(Int.type, Mark.type), query(Mark.type, PropertyAttr.type));
        "#,
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
        let result = execute(artifact).unwrap();
        for index in [0, 1] {
            let (tag, payload) = result
                .value()
                .sequence_get(index)
                .unwrap()
                .tagged_parts()
                .unwrap();
            assert_eq!(tag.as_atom().unwrap().as_str(), "Some");
            assert_eq!(payload.dict_get("value").unwrap().as_int(), Some(23));
        }
        assert_eq!(
            result
                .value()
                .sequence_get(2)
                .unwrap()
                .as_atom()
                .unwrap()
                .as_str(),
            "None"
        );
        let (_, attr) = result
            .value()
            .sequence_get(3)
            .unwrap()
            .tagged_parts()
            .unwrap();
        assert_eq!(attr.dict_get("bits").unwrap().as_int(), Some(17));
        assert!(attr.solved_type_id().is_some());
        assert_eq!(CALLS.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn property_absence_is_lazy_and_actual_property_global_cycles_fail() {
        for (query, expected) in [
            ("query(Int.type, Mark.type)", None),
            ("query(Item.type, Mark.type)", Some("provider failed")),
        ] {
            let source = format!(
                r#"
                import "std/type-property" {{ get_type_prop as query }};
                @property(PropertyTarget.Type) type Mark = struct {{ value: Int }};
                def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) {{ fail!("provider failed") }};
                @mark type Item = struct {{ value: Int }};
                export def answer = {query};
            "#
            );
            let mir = graph(&source, "");
            let result = execute(compile(mir.seal().unwrap(), entry(&mir)).unwrap());
            if let Some(expected) = expected {
                assert!(result.err().unwrap().contains(expected));
            } else {
                assert_eq!(result.unwrap().value().as_atom().unwrap().as_str(), "None");
            }
        }
        let mir = graph(
            r#"
            import "std/type-property" { get_type_prop as query };
            @property(PropertyTarget.Type) type Mark = struct { value: Int };
            def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { let dependency = a; { value: 1 } };
            @mark type Item = struct { value: Int };
            def a: Option(Mark) = query(Item.type, Mark.type);
            export def answer = a;
        "#,
            "",
        );
        let error = execute(compile(mir.seal().unwrap(), entry(&mir)).unwrap())
            .err()
            .unwrap();
        assert!(
            error.contains("cyclic demand") && error.contains("property(") && error.contains("::a"),
            "{error}"
        );
    }
    #[test]
    fn type_metadata_retains_solved_ids_without_executing_property_providers() {
        let mir = graph(
            r#"
            @property(PropertyTarget.Type)
            type Mark = struct { value: Int };
            def mark: Fn(Type, Option(Mark)) -> Mark = fn(owner, previous) { fail!("must remain lazy") };
            @mark
            type Item = struct { next: Array(Item) };
            type Alias = Item;
            export def answer = (Int.type, Array(Int).type, Item.type, Alias.type);
        "#,
            "",
        );
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        let types = &artifact.types;
        let expected = types.types[artifact.result_type.index()]
            .arguments
            .iter()
            .map(|id| types.types[id.index()].arguments[0])
            .collect::<Vec<_>>();
        let result = execute(artifact).unwrap();
        for (index, id) in expected.iter().enumerate() {
            let metadata = result.value().sequence_get(index).unwrap();
            assert_eq!(metadata.kind(), crate::ValueKind::Type);
            assert_eq!(metadata.represented_type_id(), Some(*id));
            assert!(metadata.as_int().is_none());
        }
        assert_eq!(expected[2], expected[3]);
        let mir = graph(
            "type Alias = Int; export def answer = if Int.type == Alias.type { 42 } else { 0 };",
            "",
        );
        assert_eq!(
            execute(compile(mir.seal().unwrap(), entry(&mir)).unwrap())
                .unwrap()
                .value()
                .as_int(),
            Some(42)
        );
    }

    #[test]
    fn generic_metadata_uses_closed_mir_instances_through_calls_and_recursion() {
        let mir = graph(r#"
            def metadata: for(T) Fn(T) -> TypeOf(Array(T)) = fn(value) { Array(T).type };
            def apply: for(A, B) Fn(Fn(A) -> B, A) -> B = fn(f, value) { f(value) };
            def repeated: for(T) Fn(T, Int) -> TypeOf(Array(T)) = fn(value, n) {
                if n > 0 { repeated(value, n - 1) } else { metadata(value) }
            };
            export def answer = (metadata(42), apply(metadata, "ok"), repeated(True, 3));
        "#, "");
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        let expected = artifact.types.types[artifact.result_type.index()].arguments.iter()
            .map(|ty| artifact.types.types[ty.index()].arguments[0]).collect::<Vec<_>>();
        assert!(artifact.graph.nodes().iter().any(|node| matches!(node.task, crate::execution_graph::Task::Instance { .. })));
        // Neither HIR nor the solver survives into runtime execution.
        drop(mir);
        let result = execute(artifact).unwrap();
        for (index, ty) in expected.into_iter().enumerate() {
            assert_eq!(result.value().sequence_get(index).unwrap().represented_type_id(), Some(ty));
        }
    }

    #[test]
    fn generic_nominal_constructors_consume_instance_type_ids() {
        let mir = graph(r#"
            type Wrapped(T) = struct(T);
            def make: for(T) Fn(T) -> Wrapped(T) = fn(value) { Wrapped(T)(value) };
            export def answer = (make(42), make("ok"));
        "#, "");
        let artifact = compile(mir.seal().unwrap_or_else(|d| panic!("{d:?}\n{}", mir.dump())), entry(&mir)).unwrap();
        let expected = artifact.types.types[artifact.result_type.index()].arguments.clone();
        drop(mir);
        let result = execute(artifact).unwrap();
        for (index, ty) in expected.into_iter().enumerate() {
            assert_eq!(result.value().sequence_get(index).unwrap().solved_type_id(), Some(ty));
        }
    }
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

    pub(crate) fn graph(main: &str, math: &str) -> Mir {
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
    pub(crate) fn entry(mir: &Mir) -> SymbolId {
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
    fn codegen_rejects_invalid_mir_and_leaves_runtime_failures_to_vm() {
        for source in [
            "export def answer = missing;",
            "export def answer: Int = \"wrong\";",
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
    fn native_instances_receive_their_solved_signature_without_inspecting_arguments() {
        let mir = graph(r#"
            native signature: for(T) Fn(T) -> Type;
            def forward: for(U) Fn(U) -> Type = fn(value) { signature(value) };
            export def answer = (forward(42), forward("ok"));
        "#, "");
        let mut artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        artifact.bytecode = crate::execution_link::link_with(&artifact, |_| {
            Some(crate::NativeFunction::new("test.signature", 1, |ctx| {
                // Read only compiler-provided metadata; deliberately never
                // inspect the user argument to determine its type.
                assert!(ctx.solved_signature()?.is_some());
                ctx.copy(ctx.result(), ctx.upvalue(0)?)
            }))
        }).unwrap();
        artifact.native_links.clear();
        drop(mir);
        let result = execute(artifact).unwrap();
        for (index, expected) in [TypeConstructor::Int, TypeConstructor::String].into_iter().enumerate() {
            let id = result.value().sequence_get(index).unwrap().represented_type_id().unwrap();
            let signature = &result.types().types[id.index()];
            assert_eq!(signature.constructor, TypeConstructor::Function);
            assert_eq!(result.types().types[signature.arguments[0].index()].constructor, expected);
        }
    }

    #[test]
    fn tuple_literals_complete_candidates_with_static_element_targets() {
        let definitions = r#"
            import "std/_rt" as rt;
            @check(fn(value) { if value.x > 0 { Ok(()) } else { Err(blame!("positive tuple item", value.x)) } }) type Point = struct {x: Int};
            def candidate: Unchecked(Point) = {x: 42};
        "#;
        for body in [
            "export def answer = do { let pair: (Point, Int) = (candidate, 0); pair.0.x + pair.1 };",
            "def accept: Fn((Point, Int)) -> Int = fn(pair) { pair.0.x }; export def answer = accept((candidate, 0));",
            "def pair: Fn(Unchecked(Point)) -> (Point, Int) = fn(value) { (value, 0) }; export def answer = pair(candidate).0.x;",
            "export def answer = do { let nested: ((Point, Int), String) = ((candidate, 0), \"ok\"); nested.0.0.x };",
            r#"export def answer = match rt.with_diagnostics(fn(x: Int) { let value: Unchecked(Point) = {x: x}; let pair: (Int, Point) = (0, value); pair })(0) { Err(errors) => if errors[0].message == "positive tuple item" { 42 } else { 0 }, _ => 0 };"#,
        ] {
            let mir = graph(&format!("{definitions}{body}"), "");
            assert!(mir.diagnostics.is_empty(), "{body}\n{:?}", mir.diagnostics);
            let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
            drop(mir);
            let result = execute(artifact).unwrap_or_else(|e| panic!("{body}\n{e}"));
            assert_eq!(result.value().as_int(), Some(42), "{body}");
        }
    }

    #[test]
    fn branch_completion_keeps_unselected_candidates_unchecked() {
        let definitions = r#"
            import "std/_rt" as rt;
            @check(fn(value) { if value.x > 0 { Ok(()) } else { Err(blame!("positive branch", value.x)) } }) type Point = struct {x: Int};
            def good: Point = {x: 42};
            def bad: Unchecked(Point) = {x: 0};
        "#;
        for body in [
            "export def answer = (if True { good } else { bad }).x;",
            "export def answer = (if False { bad } else { good }).x;",
            "export def answer = (match True { True => good, False => bad }).x;",
            "export def answer = (match False { True => bad, False => good }).x;",
            r#"export def answer = match rt.with_diagnostics(fn(flag: Bool) { if flag { bad } else { good } })(True) { Err(errors) => if errors[0].message == "positive branch" { 42 } else { 0 }, _ => 0 };"#,
            r#"export def answer = match rt.with_diagnostics(fn(flag: Bool) { match flag { True => bad, False => good } })(True) { Err(errors) => if errors[0].message == "positive branch" { 42 } else { 0 }, _ => 0 };"#,
            "export def answer = do { let value: Point = if True { good } else { bad }; value.x + bad.x };",
            r#"@check(fn(value) { if value.valid { Ok(()) } else { Err(blame!("invalid generic branch", value.item)) } }) type Box(T) = struct {item: T, valid: Bool};
                def choose: for(T) Fn(Bool, Unchecked(Box(T)), Box(T)) -> Box(T) = fn(flag, candidate, good) { if flag { candidate } else { good } };
                export def answer = match rt.with_diagnostics(fn(flag: Bool) { let candidate: Unchecked(Box(Int)) = {item: 0, valid: False}; let good: Box(Int) = {item: 42, valid: True}; choose(flag, candidate, good) })(True) { Err(errors) => if errors[0].message == "invalid generic branch" { 42 } else { 0 }, _ => 0 };"#,
        ] {
            let mir = graph(&format!("{definitions}{body}"), "");
            assert!(mir.diagnostics.is_empty(), "{body}\n{:?}", mir.diagnostics);
            let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
            drop(mir);
            let result = execute(artifact).unwrap_or_else(|e| panic!("{body}\n{e}"));
            assert_eq!(result.value().as_int(), Some(42), "{body}");
        }
    }

    #[test]
    fn unchecked_metadata_observes_shared_skeleton_without_checks() {
        for body in [
            r#"def body = match td.resolve(Unchecked(Box(Int)).type) { Ok(value) => value, _ => fail!("resolve") };
                def checked_body = match td.resolve(Box(Int).type) { Ok(value) => value, _ => fail!("resolve") };
                export def answer = if td.kind(Unchecked(Box(Int)).type) == td.TypeDescKind.Ref && body == checked_body && td.kind(body) == td.TypeDescKind.Struct && td.fields(body)[1].ty == Int.type { 42 } else { 0 };"#,
            r#"export def answer = if td.fields(Unchecked(Box(String)).type)[1].ty == String.type && td.fields(Unchecked(Box(Int)).type)[0].ty == Array(Box(Int)).type { 42 } else { 0 };"#,
            r#"export def answer = do { let candidate: Unchecked(Box(Int)) = {value: 42, children: []}; let packed = dyn.pack(Unchecked(Box(Int)).type, candidate); match dyn.project_with(Int.type, dyn.get_field_value(packed, 1)) { Some(value) => value, _ => 0 } };"#,
        ] {
            let mir = graph(&format!(r#"
                import "std/type-desc" as td; import "std/dyn" as dyn;
                @check(fn(value) {{ fail!("metadata must not complete a candidate") }})
                type Box(T) = struct {{value: T, children: Array(Box(T))}};
                {body}
            "#), "");
            assert!(mir.diagnostics.is_empty(), "{body}\n{:?}", mir.diagnostics);
            let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
            drop(mir);
            let result = execute(artifact).unwrap_or_else(|e| panic!("{body}\n{e}"));
            assert_eq!(result.value().as_int(), Some(42), "{body}");
        }
    }

    #[test]
    fn unchecked_values_complete_at_explicit_mir_boundaries() {
        let definitions = r#"
            import "std/dyn" as dyn; import "std/_rt" as rt;
            @check(fn(value) { if value.x > 0 { Ok(()) } else { Err(blame!("positive x", value.x)) } }) type Point = struct {x: Int};
            type Box(T) = struct {value: T};
        "#;
        for body in [
            r#"export def answer = do { let candidate: Unchecked(Point) = {x: 0}; candidate.x + 42 };"#,
            r#"export def answer = do { let candidate: Unchecked(Point) = {x: 42}; let checked: Point = candidate; checked.x };"#,
            r#"def accept: Fn(Point) -> Int = fn(point) { point.x }; export def answer = do { let candidate: Unchecked(Point) = {x: 42}; accept(candidate) };"#,
            r#"type Container = struct {point: Point}; export def answer = do { let candidate: Unchecked(Point) = {x: 42}; let value: Container = {point: candidate}; value.point.x };"#,
            r#"export def answer = match rt.with_diagnostics(fn(x: Int) { let candidate: Unchecked(Point) = {x: x}; let values: Array(Point) = [candidate]; values })(0) { Err(errors) => if errors[0].message == "positive x" { 42 } else { 0 }, _ => 0 };"#,
            r#"export def answer = do { let candidate: Unchecked(Unchecked(Point)) = {x: 42}; if Unchecked(Unchecked(Point)).type == Unchecked(Point).type { candidate.x } else { 0 } };"#,
            r#"export def answer = do { let candidate: Unchecked(Point) = {x: 42}; match candidate.cast!(Point) { Ok(value) => value.x, _ => 0 } };"#,
            r#"export def answer = do { let candidate: Unchecked(Point) = {x: 42}; let packed = dyn.pack(Unchecked(Point).type, candidate); match dyn.project_with(Point.type, packed) { None => 42, _ => 0 } };"#,
            r#"def finish: for(T) Fn(Unchecked(Box(T))) -> Box(T) = fn(candidate) { candidate }; export def answer = do { let candidate: Unchecked(Box(Int)) = {value: 42}; finish(candidate).value };"#,
            r#"def finish: Fn(Unchecked(Point)) -> Point = fn(candidate) { candidate };
                export def answer = match rt.with_diagnostics(fn(x: Int) { let candidate: Unchecked(Point) = {x: x}; finish(candidate) })(0) { Err(errors) => if errors[0].message == "positive x" { 42 } else { 0 }, _ => 0 };"#,
            r#"@check(fn(value) { if value.valid { Ok(()) } else { Err(blame!("invalid box", value.value)) } }) type CheckedBox(T) = struct {value: T, valid: Bool};
                def finish: for(T) Fn(Unchecked(CheckedBox(T))) -> CheckedBox(T) = fn(candidate) { candidate };
                export def answer = match rt.with_diagnostics(fn(x: Int) { let candidate: Unchecked(CheckedBox(Int)) = {value: x, valid: False}; finish(candidate) })(0) { Err(errors) => if errors[0].message == "invalid box" { 42 } else { 0 }, _ => 0 };"#,
            r#"export def answer = match rt.with_diagnostics(fn(x: Int) { let candidate: Unchecked(Point) = {x: x}; let checked: Point = candidate; checked })(0) { Err(errors) => if errors[0].message == "positive x" { 42 } else { 0 }, _ => 0 };"#,
        ] {
            let mir = graph(&format!("{definitions}{body}"), "");
            assert!(mir.diagnostics.is_empty(), "{body}\n{:?}", mir.diagnostics);
            let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
            drop(mir);
            let result = execute(artifact).unwrap_or_else(|e| panic!("{body}\n{e}"));
            assert_eq!(result.value().as_int(), Some(42), "{body}");
        }
    }

    #[test]
    fn checked_casts_use_closed_ids_and_validate_before_construction() {
        let definitions = r#"
            import "std/_rt" as rt;
            @check(fn(value) { if value.x > 0 { Ok(()) } else { Err(blame!("positive x required", value.x)) } }) type Point = struct {x: Int};
            type Container = struct {point: Point};
            type Other = struct {x: Int};
        "#;
        for body in [
            r#"export def answer = match {x: 42}.cast!(Point) { Ok(point) => point.x, _ => 0 };"#,
            r#"export def answer = match {point: {x: 42}}.cast!(Container) { Ok(value) => value.point.x, _ => 0 };"#,
            r#"export def answer = match [{x: 42}].cast!(Array(Point)) { Ok(value) => value[0].x, _ => 0 };"#,
            r#"export def answer = match Some({x: 42}).cast!(Option(Point)) { Ok(Some(value)) => value.x, _ => 0 };"#,
            r#"def raw: Result(Int, String) = Ok(42); export def answer = match raw.cast!(Result(Int, Bool)) { Ok(Ok(value)) => value, _ => 0 };"#,
            r#"@check(fn(value) { if value > 0 { Ok(()) } else { Err(blame!("positive payload", value)) } }) type Count = struct(Int);
                export def answer = match (42,).cast!(Count) { Ok(Count(value)) => value, _ => 0 };"#,
            r#"@check(fn(value) { fail!("checker execution failure") }) type Broken = struct {x: Int};
                export def answer = match rt.with_diagnostics(fn(x: Int) { {x: x}.cast!(Broken) })(0) { Err(errors) => if errors[0].message == "checker execution failure" { 42 } else { 0 }, _ => 0 };"#,
            r#"export def answer = match {x: "wrong"}.cast!(Point) { Err(message) => if message == "value.x must be Int, got String" { 42 } else { 0 }, _ => 0 };"#,
            r#"export def answer = match "42".cast!(Int) { Err(_) => 42, _ => 0 };"#,
            r#"export def answer = match 42.cast!(Float) { Err(_) => 42, _ => 0 };"#,
            r#"export def answer = do { let value: Other = {x: 42}; match value.cast!(Point) { Err(_) => 42, _ => 0 } };"#,
            r#"export def answer = match rt.with_diagnostics(fn(x: Int) { {point: {x: x}}.cast!(Container) })(0) { Err(errors) => if errors[0].message == "positive x required" { 42 } else { 0 }, _ => 0 };"#,
            r#"@check(fn(value) { fail!("must not check mismatching graph") }) type Deferred = struct {x: Int};
                type Pair = struct {a: Deferred, b: Int};
                export def answer = match {a: {x: 1}, b: "bad"}.cast!(Pair) { Err(_) => 42, _ => 0 };"#,
            r#"def cast: for(T) Fn(T) -> Result(Point, String) = fn(value) { value.cast!(Point) };
                export def answer = match cast({x: 42}) { Ok(value) => value.x, _ => 0 };"#,
        ] {
            let mir = graph(&format!("{definitions}{body}"), "");
            assert!(mir.diagnostics.is_empty(), "{body}\n{:?}", mir.diagnostics);
            let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
            drop(mir);
            let result = execute(artifact).unwrap_or_else(|e| panic!("{body}\n{e}"));
            assert_eq!(result.value().as_int(), Some(42), "{body}");
        }
    }

    #[test]
    fn generic_construction_checks_consume_static_body_instances() {
        let definitions = r#"
            import "std/json" as json;
            import "std/_rt" as rt;
            def identity: for(T) Fn(T) -> T = fn(value) { value };
            @check(fn(value) { let copied = identity(value.item); if value.valid { Ok(()) } else { Err(blame!("invalid item", copied)) } })
            type Item(T) = struct { item: T, valid: Bool };
            @check(fn(value) { let copied = identity(value); Ok(()) }) type Wrapped(T) = struct(T);
            type Choice(T) = enum { @check(fn(value) { let copied = identity(value); Ok(()) }) Some(T), Empty };
            type Envelope(T) = struct { child: Item(T) };
        "#;
        for body in [
            r#"export def answer = do { let a: Item(Int) = { item: 40, valid: True }; let b: Item(String) = { item: "ok", valid: True }; a.item + 2 };"#,
            r#"def make: for(T) Fn(T) -> Item(T) = fn(value) { { item: value, valid: True } }; export def answer = make(42).item;"#,
            r#"export def answer = match rt.with_diagnostics(fn(value: Int) { let item: Item(Int) = { item: value, valid: False }; item })(0) { Err(errors) => if errors[0].message == "invalid item" { 42 } else { 0 }, _ => 0 };"#,
            r#"type IntWrapped = Wrapped(Int); export def answer = match IntWrapped(42) { IntWrapped(value) => value };"#,
            r#"type IntChoice = Choice(Int); export def answer = match IntChoice.Some(42) { IntChoice.Some(value) => value, _ => 0 };"#,
            r#"export def answer = match json.decode(Envelope(Int).type, "{\"child\":{\"item\":42,\"valid\":true}}") { Ok(value) => value.child.item, _ => 0 };"#,
            r#"export def answer = match json.decode(Envelope(String).type, "{\"child\":{\"item\":\"bad\",\"valid\":false}}") { Err(_) => 42, _ => 0 };"#,
        ] {
            let mir = graph(&format!("{definitions}{body}"), "");
            assert!(mir.diagnostics.is_empty(), "{body}\n{}", mir.dump());
            let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
            drop(mir);
            let result = execute(artifact).unwrap_or_else(|e| panic!("{body}\n{e}"));
            assert_eq!(result.value().as_int(), Some(42), "{body}");
        }
    }

    #[test]
    fn solved_codec_construction_checks_reject_values_and_preserve_trial_semantics() {
        let definitions = r#"
            import "std/json" as json; import "std/string" as string; import "std/regex" as regex;
            import "std/_rt" as rt;
            @check(fn(value) { if value > 0 { Ok(()) } else { Err(blame!("positive count", value)) } }) type Count = struct(Int);
            @string.decode_by_parse @string.encode_by_display
            @regex.parse_by(regex.compile(r"^(?P<value>\d+)$"))
            @check(fn(item) { if item.value > 0 { Ok(()) } else { Err(blame!("positive value", item.value)) } }) type Item = struct {value: Int};
            @json.untagged type Choice = enum { @check(fn(value) { if value > 0 { Ok(()) } else { Err(blame!("positive variant", value)) } }) Checked(Int), Plain(Int) };
            @json.untagged type TextChoice = enum { Parsed(Item), Plain(String) };
        "#;
        for body in [
            r#"export def answer = match json.decode(Count.type, "0") { Err(_) => 42, _ => 0 };"#,
            r#"export def answer = match json.decode(Count.type, "42") { Ok(Count(value)) => value, _ => 0 };"#,
            r#"export def answer = match json.decode(Choice.type, "0") { Ok(Choice.Plain(_)) => 42, _ => 0 };"#,
            r#"export def answer = match json.decode(Choice.type, "1") { Err(_) => 42, _ => 0 };"#,
            r#"export def answer = match json.decode(Item.type, "\"0\"") { Err(_) => 42, _ => 0 };"#,
            r#"export def answer = match json.decode(TextChoice.type, "\"0\"") { Ok(TextChoice.Plain(_)) => 42, _ => 0 };"#,
            r#"export def answer = match rt.with_diagnostics(fn(text: String) { string.parse(Item.type, text) })("0") { Err(errors) => if errors[0].message == "positive value" { 42 } else { 0 }, _ => 0 };"#,
            r#"@check(fn(value) { fail!("checker execution failed") }) type Broken = struct(Int);
                @json.untagged type BrokenChoice = enum { Plain(Int), Broken(Broken) };
                export def answer = match rt.with_diagnostics(fn(text: String) { json.decode(BrokenChoice.type, text) })("1") { Err(errors) => if errors[0].message == "checker execution failed" { 42 } else { 0 }, _ => 0 };"#,
            r#"@check(fn(value) { if value.number > 0 { Ok(()) } else { Err(blame!("positive record", value.number)) } }) type Record = struct {number: Int};
                export def answer = match json.decode(Record.type, "{\"number\":0}") { Err(_) => 42, _ => 0 };"#,
        ] {
            let mir = graph(&format!("{definitions}{body}"), "");
            assert!(mir.diagnostics.is_empty(), "{body}\n{:?}", mir.diagnostics);
            let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
            drop(mir);
            let result = execute(artifact).unwrap_or_else(|e| panic!("{body}\n{e}"));
            assert_eq!(result.value().as_int(), Some(42), "{body}");
        }
    }

    #[test]
    fn construction_checks_execute_at_solved_constructor_boundaries() {
        for source in [
            r#"def minimum = 1; def validate = fn(value) { if value >= minimum { Ok(()) } else { Err(blame!("minimum", value)) } };
                @check(validate) type Item = struct(Int); export def answer = match Item(42) { Item(value) => value };"#,
            r#"@check(fn(value) { if value > 0 { Ok(()) } else { Err(blame!("positive", value)) } }) type Item = struct(Int);
                export def answer = match Item(42) { Item(value) => value };"#,
            r#"@check(fn(value) { if value.number > 0 { Ok(()) } else { Err(blame!("positive", value.number)) } }) type Item = struct {number: Int};
                def value: Item = {number: 42}; export def answer = value.number;"#,
            r#"type Item = enum { @check(fn(value) { if value > 0 { Ok(()) } else { Err(blame!("positive", value)) } }) Full(Int), Empty };
                export def answer = match Item.Full(42) { Item.Full(value) => value, _ => 0 };"#,
            r#"import "std/_rt" as rt; import "std/array" as array;
                @check(fn(value) { if value > 0 { Ok(()) } else { Err(blame!("positive", value)) } }) type Item = struct(Int);
                export def answer = match rt.with_diagnostics(fn(n: Int) { Item(n) })(0) { Err(errors) => if array.length(errors) == 1 && errors[0].message == "positive" { 42 } else { 0 }, _ => 0 };"#,
            r#"import "std/_rt" as rt; import "std/array" as array;
                @check(fn(value) { if value.number > 0 { Ok(()) } else { Err(blame!("positive", value.number)) } }) type Item = struct {number: Int};
                def attempt = rt.with_diagnostics(fn(n: Int) { let value: Item = {number: n}; value });
                export def answer = match (attempt(0), attempt(0), attempt(42)) { (Err(first), Err(second), Ok((value, _))) => if array.length(first) == 1 && array.length(second) == 1 { value.number } else { 0 }, _ => 0 };"#,
            r#"import "std/_rt" as rt; type Item = enum { @check(fn(value) { Err(blame!("rejected", value)) }) Full(Int), Empty };
                export def answer = match rt.with_diagnostics(fn(n: Int) { Item.Full(n) })(1) { Err(_) => 42, _ => 0 };"#,
        ] {
            let mir = graph(source, "");
            assert!(mir.diagnostics.is_empty(), "{source}\n{:?}", mir.diagnostics);
            let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
            drop(mir);
            let result = execute(artifact).unwrap_or_else(|e| panic!("{source}\n{e}"));
            assert_eq!(result.value().as_int(), Some(42), "{source}");
        }
    }

    #[test]
    fn solved_codec_text_decode_roundtrips_and_composes_with_untagged_trials() {
        let definitions = r#"
            import "std/codec" as codec; import "std/json" as json;
            import "std/regex" as regex; import "std/string" as string; import "std/fmt" as fmt;
            @string.decode_by_parse @string.encode_by_display @fmt.display_by("{host}:{port}")
            @regex.parse_by(regex.compile(r"^(?P<host>[^:]+):(?P<port>\d+)$"))
            type Endpoint = struct { host: String, port: Int };
            @string.decode_by_parse @string.encode_by_display @fmt.display_by("{name}@{endpoint}")
            @regex.parse_by(regex.compile(r"^(?P<name>\w+)@(?P<endpoint>.+)$"))
            type Service = struct { name: String, endpoint: Endpoint };
            @json.untagged type Choice = enum { Parsed(Endpoint), Text(String) };
        "#;
        for body in [
            r#"def value = json.decode(Service.type, "\"api@local:42\"").unwrap!(); export def answer = if codec.encode(codec.Value.type, value) == codec.Value.String("api@local:42") { value.endpoint.port } else { 0 };"#,
            r#"export def answer = match json.decode(Choice.type, "\"not-an-endpoint\"") { Ok(Choice.Text(value)) => if value == "not-an-endpoint" { 42 } else { 0 }, _ => 0 };"#,
            r#"export def answer = match json.decode(Choice.type, "\"local:42\"") { Err(_) => 42, _ => 0 };"#,
            r#"export def answer = match json.decode(Endpoint.type, "42") { Err(_) => 42, _ => 0 };"#,
        ] {
            let mir = graph(&format!("{definitions}{body}"), "");
            assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
            let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
            drop(mir);
            let result = execute(artifact).unwrap_or_else(|e| panic!("{body}\n{e}"));
            assert_eq!(result.value().as_int(), Some(42), "{body}");
        }
    }

    #[test]
    fn solved_regex_prepare_checks_capture_contracts_without_type_reconstruction() {
        for (field, pattern, expected) in [
            ("value: Int", "^(?P<other>.*)$", "captures must match struct fields"),
            ("value: Option(Int)", "^(?P<value>.*)$", "capture \"value\" is required"),
            ("value: Int", "^(?P<value>.*)?$", "capture \"value\" is optional"),
            ("value: Array(Int)", "^(?P<value>.*)$", "not string-parsable"),
        ] {
            let mir = graph(&format!(r#"import "std/string" as string; import "std/regex" as regex;
                @regex.parse_by(regex.compile(r"{pattern}")) type Item = struct {{ {field} }};
                export def answer = string.parse(Item.type, "42");"#), "");
            assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
            let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
            let error = execute(artifact).err().expect("invalid capture contract");
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn solved_string_parse_uses_capture_ranges_and_lazy_nested_properties() {
        for source in [
            r#"import "std/string" as string; export def answer = match string.parse(Int.type, "42") { Ok(value) => value, Err(_) => 0 };"#,
            r#"import "std/string" as string; import "std/regex" as regex;
                @regex.parse_by(regex.compile(r"^(?P<host>[^:]+):(?P<port>\d+)$"))
                type Endpoint = struct { host: String, port: Int };
                @regex.parse_by(regex.compile(r"^(?P<name>\w+)@(?P<endpoint>.+)$"))
                type Service = struct { name: String, endpoint: Endpoint };
                export def answer = match string.parse(Service.type, "api@local:42") { Ok(value) => if value.name == "api" && value.endpoint.host == "local" { value.endpoint.port } else { 0 }, Err(_) => 0 };"#,
            r#"import "std/string" as string; import "std/regex" as regex;
                @regex.parse_by(regex.compile(r"^(?P<value>\d+)(?:/(?P<note>\w+))?$"))
                type Item = struct { value: Int, note: Option(String) };
                export def answer = match string.parse(Item.type, "42") { Ok(value) => if value.note == None { value.value } else { 0 }, Err(_) => 0 };"#,
            r#"import "std/string" as string; export def answer = match string.parse(Int.type, "bad") { Err(error) => if error.value == "bad" { 42 } else { 0 }, _ => 0 };"#,
            r#"import "std/string" as string; export def answer = match string.parse(Float.type, "NaN") { Err(_) => 42, _ => 0 };"#,
        ] {
            let mir = graph(source, "");
            assert!(mir.diagnostics.is_empty(), "{source}\n{:?}", mir.diagnostics);
            let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
            drop(mir);
            let result = execute(artifact).unwrap_or_else(|e| panic!("{source}\n{e}"));
            assert_eq!(result.value().as_int(), Some(42), "{source}");
        }
    }

    #[test]
    fn solved_codec_text_encode_calls_prepared_display_for_nested_values() {
        let mir = graph(r#"
            import "std/codec" as codec; import "std/json" as json;
            import "std/string" as string; import "std/fmt" as fmt;
            @string.decode_by_parse @string.encode_by_display
            @fmt.display_by("{host}:{port}")
            type Endpoint = struct { host: String, port: Int };
            @string.decode_by_parse @string.encode_by_display
            @fmt.display_by("{name}@{endpoint}")
            type Service = struct { name: String, endpoint: Endpoint };
            def endpoint: Endpoint = {host: "localhost", port: 8080};
            def service: Service = {name: "api", endpoint};
            export def answer = json.stringify(codec.encode(codec.Value.type, { endpoints: [endpoint, endpoint], service }));
        "#, "");
        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        drop(mir);
        let result = execute(artifact).unwrap();
        assert_eq!(result.value().as_str().unwrap().as_str(), r#"{"endpoints":["localhost:8080","localhost:8080"],"service":"api@localhost:8080"}"#);
    }

    #[test]
    fn solved_codec_text_encode_rejects_incomplete_bridge_contracts() {
        for (decorators, expected) in [
            ("@string.encode_by_display", "must be used together"),
            ("@string.decode_by_parse", "must be used together"),
            ("@string.decode_by_parse @string.encode_by_display", "requires a DisplayBy"),
        ] {
            let mir = graph(&format!(r#"import "std/codec" as codec; import "std/string" as string;
                {decorators} type Item = struct {{ value: Int }};
                def item: Item = {{value: 42}}; export def answer = codec.encode(codec.Value.type, item);"#), "");
            let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
            let error = execute(artifact).err().expect("invalid text bridge");
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn solved_codec_encode_consumes_layouts_and_lazy_untagged_properties() {
        let mir = graph(r#"
            import "std/codec" as codec;
            import "std/json" as json;
            import "std/value" {ScalarValue};
            type Box(T) = struct { value: T };
            def boxed: Box(Int) = { value: 42 };
            export def answer = json.stringify(codec.encode(codec.Value.type, {
                boxed, bindings: [ScalarValue.Int(42), ScalarValue.String("ok"), ScalarValue.None],
                sql: "SELECT 1", flags: [True, False],
            }));
        "#, "");
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        drop(mir);
        let result = execute(artifact).unwrap();
        assert_eq!(result.value().as_str().unwrap().as_str(),
            r#"{"bindings":[42,"ok",null],"boxed":{"value":42},"flags":[true,false],"sql":"SELECT 1"}"#);
    }

    #[test]
    fn solved_codec_encode_uses_rename_options_and_recursive_layouts() {
        let mir = graph(r#"
            import "std/codec" as codec;
            import "std/json" as json;
            type Tree = enum { Leaf(Int), Branch(Array(Tree)), Empty };
            @json.rename_all(json.RenameCase.CamelCase)
            type Model = struct { some_value: Option(Int), tree: Tree };
            def model: Model = { some_value: Some(42), tree: Tree.Branch([Tree.Leaf(1), Tree.Empty]) };
            export def answer = json.stringify(codec.encode(codec.Value.type, model));
        "#, "");
        let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
        let result = execute(artifact).unwrap();
        assert_eq!(result.value().as_str().unwrap().as_str(), r#"{"someValue":42,"tree":{"Branch":[{"Leaf":1},"Empty"]}}"#);
    }

    #[test]
    fn solved_codec_decode_untagged_requires_one_match_and_preserves_nested_work() {
        for source in [
            r#"import "std/json" as json;
                @json.untagged type Inner = enum { Text(String), Flag(Bool) };
                @json.untagged type Outer = enum { Inner(Inner), Number(Int) };
                export def answer = match json.decode(Outer.type, "42").unwrap!() { Outer.Number(n) => n, _ => 0 };"#,
            r#"import "std/json" as json;
                @json.untagged type Item = enum { Empty, Missing };
                export def answer = match json.decode(Item.type, "null") { Err(_) => 42, _ => 0 };"#,
            r#"import "std/json" as json;
                @json.untagged type Item = enum { Text(String), Number(Int), Empty };
                export def answer = match json.decode(Item.type, "42").unwrap!() { Item.Number(n) => n, _ => 0 };"#,
            r#"import "std/json" as json;
                @json.untagged type Item = enum { Text(String), Number(Int), Empty };
                export def answer = match json.decode(Item.type, "null").unwrap!() { Item.Empty => 42, _ => 0 };"#,
            r#"import "std/json" as json;
                @json.untagged type Item = enum { First(Int), Second(Int) };
                export def answer = match json.decode(Item.type, "42") { Err(_) => 42, _ => 0 };"#,
            r#"import "std/json" as json;
                @json.untagged type Item = enum { Text(String), Number(Int) };
                export def answer = match json.decode(Item.type, "true") { Err(_) => 42, _ => 0 };"#,
            r#"import "std/json" as json;
                @json.rename_all(json.RenameCase.CamelCase) type Named = struct { some_value: Int };
                @json.untagged type Item = enum { Wrong((Int, String)), Pair((Int, Int)), Named(Named) };
                def items = json.decode(Array(Item).type, "[[1,2],{\"someValue\":39}]").unwrap!();
                export def answer = match items[0] { Item.Pair(pair) => match items[1] { Item.Named(named) => pair.0 + pair.1 + named.some_value, _ => 0 }, _ => 0 };"#,
        ] {
            let mir = graph(source, "");
            assert!(mir.diagnostics.is_empty(), "{source}\n{:?}", mir.diagnostics);
            let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
            drop(mir);
            let result = execute(artifact).unwrap_or_else(|e| panic!("{source}\n{e}"));
            assert_eq!(result.value().as_int(), Some(42), "{source}");
        }
    }

    #[test]
    fn solved_codec_decode_uses_generic_recursive_layouts_and_reports_mismatches() {
        for source in [
            r#"import "std/json" as json;
                @json.rename_all(json.RenameCase.CamelCase)
                type Choice = enum { SomeValue(Int), NoValue };
                export def answer = match json.decode(Choice.type, "{\"someValue\":42}").unwrap!() { Choice.SomeValue(value) => value, _ => 0 };"#,
            r#"import "std/json" as json;
                @json.rename_all(json.RenameCase.CamelCase)
                type Model = struct { some_value: Int, optional_note: Option(String) };
                export def answer = json.decode(Model.type, "{\"someValue\":42}").unwrap!().some_value;"#,
            r#"import "std/json" as json;
                type Box(T) = struct { value: T, note: Option(String) };
                type Tree = enum { Leaf(Box(Int)), Branch(Array(Tree)), Empty };
                def decoded = json.decode(Tree.type, "{\"Branch\":[{\"Leaf\":{\"value\":42}},\"Empty\"]}").unwrap!();
                export def answer = match decoded { Tree.Branch(items) => match items[0] { Tree.Leaf(boxed) => if boxed.note == None { boxed.value } else { 0 }, _ => 0 }, _ => 0 };"#,
            r#"import "std/json" as json; import "std/_rt" as rt; import "std/array" as array;
                type Box = struct { value: Int };
                export def answer = match rt.with_diagnostics(fn(text: String) { json.decode(Box.type, text).unwrap!() })("{\"value\":\"bad\"}") {
                    Err(errors) => if array.length(errors) == 1 { 42 } else { 0 }, _ => 0
                };"#,
            r#"import "std/json" as json; type Box = struct { value: Int };
                export def answer = match json.decode(Box.type, "{\"value\":42,\"extra\":0}") { Err(_) => 42, _ => 0 };"#,
        ] {
            let mir = graph(source, "");
            let artifact = compile(mir.seal().unwrap(), entry(&mir)).unwrap();
            drop(mir);
            let result = execute(artifact).unwrap_or_else(|e| panic!("{source}\n{e}"));
            assert_eq!(result.value().as_int(), Some(42), "{source}");
        }
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
