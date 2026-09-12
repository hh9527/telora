use super::*;
use cranelift_module::FuncId;
use telora_core::mir::GenericInstanceId;

pub(super) fn is_native(kind: &HirKind) -> bool {
    matches!(
        kind,
        HirKind::Binding {
            kind: telora_core::ast::BindingKind::Native,
            ..
        }
    )
}

pub(super) fn constructor(graph: &Mir, node: HirId) -> Option<MemberSelection> {
    if let Some(selection @ (MemberSelection::EnumVariant { .. } | MemberSelection::NewtypeConstructor)) = graph.member_selections[node.index()] {
        return Some(selection);
    }
    if matches!(graph.hir[node.index()].kind, HirKind::TypeApply) {
        return constructor(graph, child(graph, node, Role::Callee).ok()?);
    }
    let slot = graph.hir[node.index()].resolution?;
    let ResolveState::Bound(symbol) = graph.resolve_slots[slot.index()] else {
        return None;
    };
    graph.symbols[symbol.index()]
        .declarations
        .iter()
        .find_map(|&declaration| {
            let value = child(graph, declaration, Role::Value).ok()?;
            match graph.member_selections[value.index()] {
                Some(selection @ (MemberSelection::EnumVariant { .. } | MemberSelection::NewtypeConstructor)) => Some(selection),
                _ => None,
            }
        })
}

pub(super) fn is_callable(graph: &Mir, key: Key) -> bool {
    !key.initializer
        && (matches!(graph.hir[key.node.index()].kind, HirKind::Closure | HirKind::Interpreter)
            || is_native(&graph.hir[key.node.index()].kind)
            || constructor(graph, key.node).is_some())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Key {
    pub node: HirId,
    pub instance: Option<GenericInstanceId>,
    pub initializer: bool,
    pub configured_factory: bool,
}
impl Key {
    pub fn ty(self, mir: &Mir, node: HirId) -> Result<telora_core::mir::TypeId> {
        if matches!(mir.hir[node.index()].kind, HirKind::Closure)
            && let Some(slot) = mir.value_adjustments[node.index()] {
            return match self.instance {
                Some(id) => mir.generic_instances[id.index()].adjustment(node)
                    .ok_or_else(|| "native instance has no sealed callable adjustment".into()),
                None => match mir.ty_slots[slot.index()] {
                    TypeState::Known(ty) => Ok(ty),
                    _ => Err("native callable adjustment is not closed".into()),
                },
            };
        }
        if self.configured_factory && node == self.node {
            let factory = Self { configured_factory: false, ..self }.ty(mir, node)?;
            return mir.types[factory.index()]
                .arguments
                .last()
                .copied()
                .ok_or_else(|| "native factory has no configured function type".into());
        }
        match self.instance {
            Some(id) => mir.generic_instances[id.index()].ty(node).ok_or_else(|| {
                format!(
                    "native instance {} has no solved type for HIR {}",
                    id.index(),
                    node.index()
                )
            }),
            None => known(mir, node),
        }
    }
}
impl From<HirId> for Key {
    fn from(node: HirId) -> Self {
        Self {
            node,
            instance: None,
            initializer: false,
            configured_factory: false,
        }
    }
}

pub(super) struct Functions {
    pub registered: BTreeMap<Key, FuncId>,
    pub pending: Vec<Key>,
    pub captures: BTreeMap<Key, Vec<(SymbolId, TypeKey)>>,
    pub instance_captures: BTreeMap<Key, Vec<(GenericInstanceId, TypeKey)>>,
    pub self_bindings: BTreeMap<HirId, SymbolId>,
    pub globals: BTreeMap<SymbolId, (u32, Key, TypeKey)>,
    pub instances: BTreeMap<GenericInstanceId, (u32, Key, TypeKey)>,
    pub properties: BTreeMap<usize, (u32, FuncId)>,
    pub checks: BTreeMap<usize, (u32, Key, TypeKey)>,
    dispatchers: BTreeMap<TypeKey, FuncId>,
    pub(super) signature: ir::Signature,
}
impl Functions {
    pub fn new(signature: ir::Signature, graph: &Mir) -> Self {
        let mut parents = vec![None; graph.hir.len()];
        let mut closures = BTreeMap::new();
        for (index, node) in graph.hir.iter().enumerate() {
            for edge in &node.children { parents[edge.node.index()] = Some(index); }
            if let Some(symbol) = graph.hir_symbols[index]
                && matches!(node.kind, HirKind::Binding { .. })
                && let Some(mut value) = node.children.iter().find(|edge| edge.role == Role::Value).map(|edge| edge.node) {
                // A block result or type ascription preserves the closure's
                // identity. Its lexical captures still come from the actual
                // closure creation site, including bindings inside the block.
                loop {
                    let role = match graph.hir[value.index()].kind {
                        HirKind::Closure => { closures.insert(symbol, value); break; }
                        HirKind::Block => Role::Result,
                        HirKind::TypeAscription => Role::Value,
                        _ => break,
                    };
                    let Ok(next) = child(graph, value, role) else { break; };
                    value = next;
                }
            }
        }
        let mut self_bindings = BTreeMap::new();
        for (index, node) in graph.hir.iter().enumerate() {
            if let Some(slot) = node.resolution
                && let ResolveState::Bound(symbol) = graph.resolve_slots[slot.index()]
                && let Some(&closure) = closures.get(&symbol) {
                let mut parent = parents[index];
                while let Some(node) = parent {
                    if node == closure.index() { self_bindings.insert(closure, symbol); break; }
                    parent = parents[node];
                }
            }
        }
        Self {
            self_bindings,
            registered: BTreeMap::new(),
            pending: vec![],
            captures: BTreeMap::new(),
            instance_captures: BTreeMap::new(),
            globals: BTreeMap::new(),
            instances: BTreeMap::new(),
            properties: BTreeMap::new(),
            checks: BTreeMap::new(),
            dispatchers: BTreeMap::new(),
            signature,
        }
    }
    pub fn global(
        &mut self,
        graph: &Mir,
        symbol: SymbolId,
        module: &mut JITModule,
    ) -> Result<(u32, FuncId, TypeKey)> {
        if let Some(&(slot, key, ty)) = self.globals.get(&symbol) {
            return Ok((slot, self.registered[&key], ty));
        }
        let declaration = graph.symbols[symbol.index()]
            .declarations
            .iter()
            .rev()
            .copied()
            .find(|&node| matches!(graph.hir[node.index()].kind, HirKind::Binding { .. }))
            .ok_or("native global has no value declaration")?;
        let ty = TypeKey::try_from(known(graph, declaration)?)?;
        let mut key = Key::from(declaration);
        key.initializer = is_native(&graph.hir[declaration.index()].kind);
        let slot = u32::try_from(self.globals.len() + self.instances.len() + self.properties.len() + self.checks.len())
            .map_err(|_| "native global slot overflow")?;
        let function = if let Some(&function) = self.registered.get(&key) {
            function
        } else {
            let function = module
                .declare_function(
                    &format!("telora_global_{}", symbol.index()),
                    Linkage::Local,
                    &self.signature,
                )
                .map_err(|e| e.to_string())?;
            self.registered.insert(key, function);
            self.pending.push(key);
            function
        };
        self.globals.insert(symbol, (slot, key, ty));
        Ok((slot, function, ty))
    }
    pub fn instance(
        &mut self,
        graph: &Mir,
        instance: GenericInstanceId,
        module: &mut JITModule,
    ) -> Result<(u32, FuncId, TypeKey)> {
        if let Some(&(slot, key, ty)) = self.instances.get(&instance) {
            return Ok((slot, self.registered[&key], ty));
        }
        let selected = &graph.generic_instances[instance.index()];
        if !selected.concrete {
            return Err("native initialization needs a concrete generic instance".into());
        }
        let node = *graph.symbols[selected.symbol.index()]
            .declarations
            .last()
            .ok_or("native instance has no declaration")?;
        let key = Key {
            node,
            instance: Some(instance),
            initializer: true,
            configured_factory: false,
        };
        let ty = TypeKey::try_from(key.ty(graph, node)?)?;
        let slot = u32::try_from(self.globals.len() + self.instances.len() + self.properties.len() + self.checks.len())
            .map_err(|_| "native instance slot overflow")?;
        let function = module
            .declare_function(
                &format!("telora_instance_init_{}", instance.index()),
                Linkage::Local,
                &self.signature,
            )
            .map_err(|e| e.to_string())?;
        self.registered.insert(key, function);
        self.pending.push(key);
        self.instances.insert(instance, (slot, key, ty));
        Ok((slot, function, ty))
    }
    pub fn check(&mut self, graph: &Mir, index: usize, module: &mut JITModule) -> Result<(u32, FuncId, TypeKey)> {
        if let Some(&(slot, key, ty)) = self.checks.get(&index) { return Ok((slot, self.registered[&key], ty)); }
        let check = &graph.construction_checks[index];
        if !check.concrete { return Err("construction check is not concrete".into()); }
        let key = Key { node: check.checker, instance: check.instance, initializer: true, configured_factory: false };
        let ty = TypeKey::try_from(check.signature)?;
        if key.ty(graph, key.node)? != check.signature { return Err("checker signature contradicts sealed instance".into()); }
        let slot = u32::try_from(self.globals.len() + self.instances.len() + self.properties.len() + self.checks.len()).map_err(|_| "native check slot overflow")?;
        let function = if let Some(&function) = self.registered.get(&key) { function } else {
            let function = module.declare_function(&format!("telora_check_init_{index}"), Linkage::Local, &self.signature).map_err(|e| e.to_string())?;
            self.registered.insert(key, function);
            self.pending.push(key);
            function
        };
        self.checks.insert(index, (slot, key, ty));
        Ok((slot, function, ty))
    }
    pub fn dispatcher(&mut self, ty: TypeKey, module: &mut JITModule) -> Result<FuncId> {
        if let Some(&id) = self.dispatchers.get(&ty) {
            return Ok(id);
        }
        let id = module
            .declare_function(
                &format!("telora_dispatch_{}", ty.raw()),
                Linkage::Local,
                &self.signature,
            )
            .map_err(|e| e.to_string())?;
        self.dispatchers.insert(ty, id);
        Ok(id)
    }
    pub fn declare(&mut self, mir: &Mir, key: Key, module: &mut JITModule) -> Result<FuncId> {
        if let Some(&function) = self.registered.get(&key) {
            return Ok(function);
        }
        let node = key.node;
        if !is_callable(mir, key)
            || mir.types[key.ty(mir, node)?.index()].constructor != TypeConstructor::Function
        {
            return Err("native direct call requires a monomorphic closure".into());
        }
        let function = module
            .declare_function(
                &format!(
                    "telora_fn_{}_{:?}_{}",
                    node.index(),
                    key.instance.map(|i| i.index()),
                    key.configured_factory,
                ),
                Linkage::Local,
                &self.signature,
            )
            .map_err(|e| e.to_string())?;
        self.registered.insert(key, function);
        self.pending.push(key);
        Ok(function)
    }
}

pub(super) fn emit_dispatchers(
    graph: &Mir,
    layouts: &Layouts,
    root: HirId,
    helper: FuncId,
    module: &mut JITModule,
    functions: &mut Functions,
) -> Result<()> {
    for (ty, dispatcher) in functions.dispatchers.clone() {
        let candidates = functions
            .registered
            .iter()
            .filter_map(|(key, &id)| {
                if !is_callable(graph, *key) {
                    return None;
                }
                Some(key.ty(graph, key.node).map(|actual| (actual, id)))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut ctx = module.make_context();
        ctx.func.signature = functions.signature.clone();
        let object_helper = module.declare_func_in_func(helper, &mut ctx.func);
        let mut fbctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let mut args = builder.block_params(entry).to_vec();
        // Resolve a stable lexical slot to its installed body at invocation.
        // Captured values and equality continue to use the original identity.
        let mut resolver = Lower {
            local_instances: BTreeMap::new(), guarded: false, frame_charge: None,
            mir: graph, layouts, builder, locals: BTreeMap::new(), module,
            context: args[0], object_helper, functions, function_key: root.into(),
            return_pointer: args[2], return_type: ty,
        };
        let zero = resolver.builder.ins().iconst(types::I64, 0);
        let resolved = resolver.object(root, helpers::RESOLVE_FUNCTION, ty, args[3], zero)
            .map_err(|e| format!("native function slot resolution: {e:?}"))?;
        args[3] = resolver.stack_words(&resolved).map_err(|e| format!("native resolved closure: {e:?}"))?;
        let mut builder = resolver.builder;
        let packed = builder
            .ins()
            .load(types::I64, MemFlagsData::new(), args[3], 16);
        let id = builder.ins().ireduce(types::I32, packed);
        for (actual, candidate) in candidates {
            if TypeKey::try_from(actual)? != ty {
                continue;
            }
            let yes = builder.create_block();
            let no = builder.create_block();
            let selected = builder.ins().icmp_imm_s(
                cranelift_codegen::ir::condcodes::IntCC::Equal,
                id,
                i64::from(candidate.as_u32()),
            );
            builder.ins().brif(selected, yes, &[], no, &[]);
            builder.switch_to_block(yes);
            builder.seal_block(yes);
            let callee = module.declare_func_in_func(candidate, builder.func);
            let call = builder.ins().call(callee, &args);
            let status = builder.inst_results(call)[0];
            builder.ins().return_(&[status]);
            builder.switch_to_block(no);
            builder.seal_block(no);
        }
        let mut lower = Lower {
            local_instances: BTreeMap::new(),
            guarded: false, frame_charge: None,
            mir: graph,
            layouts,
            builder,
            locals: BTreeMap::new(),
            module,
            context: args[0],
            object_helper,
            functions,
            function_key: root.into(),
            return_pointer: args[2],
            return_type: ty,
        };
        lower
            .report_failure(
                root,
                "native function ID does not match the closed call signature",
            )
            .map_err(|e| format!("native dispatcher: {e:?}"))?;
        let config = lower.module.target_config();
        lower.seal_frame_charge();
        lower.builder.finalize(config);
        module
            .define_function(dispatcher, &mut ctx)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
pub(super) fn shape(
    graph: &Mir,
    layouts: &Layouts,
    key: Key,
) -> Result<(Vec<HirId>, HirId, TypeKey, Vec<TypeKey>)> {
    let root = key.node;
    let root_node = graph
        .hir
        .get(root.index())
        .ok_or("native HIR outside graph")?;
    let native = is_native(&root_node.kind) && !key.initializer;
    let interpreter = matches!(root_node.kind, HirKind::Interpreter) && !key.initializer;
    let constructor = constructor(graph, root).is_some() && !key.initializer;
    if native && graph.types[key.ty(graph, root)?.index()].constructor != TypeConstructor::Function
    {
        return Err("native ABI requires a closed function instance".into());
    }
    let is_function = (matches!(root_node.kind, HirKind::Closure) && !key.initializer) || native || constructor || interpreter;
    let parameters = if is_function {
        root_node
            .children
            .iter()
            .filter(|e| e.role == Role::Parameter)
            .map(|e| e.node)
            .collect::<Vec<_>>()
    } else {
        vec![]
    };
    let body = if is_function && !native && !constructor && !interpreter {
        child(graph, root, Role::Body)?
    } else {
        root
    };
    let output = TypeKey::try_from(if is_function {
        *graph.types[key.ty(graph, root)?.index()]
            .arguments
            .last()
            .ok_or("native function has no solved return type")?
    } else {
        key.ty(graph, body)?
    })?;
    let arguments = if native || constructor || interpreter {
        let signature = &graph.types[key.ty(graph, root)?.index()];
        signature.arguments[..signature.arguments.len() - 1]
            .iter()
            .copied()
            .map(TypeKey::try_from)
            .collect::<Result<Vec<_>>>()?
    } else {
        parameters
            .iter()
            .map(|&p| TypeKey::try_from(key.ty(graph, p)?))
            .collect::<Result<Vec<_>>>()?
    };

    if !layouts.is_never(output)? {
        layouts.words(output).map_err(|error| format!("{error}; output of {key:?} at {:?}: {:?}", graph.hir[key.node.index()].location, graph.types[output.index()]))?;
    }
    for &argument in &arguments {
        if layouts.is_never(argument)? { continue; }
        layouts.words(argument).map_err(|error| format!("{error}; parameter of {key:?} at {:?}: {:?}", graph.hir[key.node.index()].location, graph.types[argument.index()]))?;
    }
    Ok((parameters, body, output, arguments))
}
pub(super) fn emit(
    graph: &Mir,
    layouts: &Layouts,
    key: Key,
    helper: FuncId,
    module: &mut JITModule,
    functions: &mut Functions,
) -> Result<()> {
    let (parameters, body, output, arguments) = shape(graph, layouts, key)?;
    let uncallable = arguments.iter().try_fold(false, |found, &ty| layouts.is_never(ty).map(|never| found || never))?;

    let function = functions.registered[&key];
    let mut ctx = module.make_context();
    ctx.func.signature = functions.signature.clone();
    let object_helper = module.declare_func_in_func(helper, &mut ctx.func);
    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let args = builder.block_params(entry)[1];
        let context = builder.block_params(entry)[0];
        let out = builder.block_params(entry)[2];
        let environment = builder.block_params(entry)[3];
        let mut locals = BTreeMap::new();
        let mut offset: usize = 0;
        for (&parameter, &ty) in parameters.iter().zip(&arguments) {
            if uncallable { break; }
            let symbol = graph.hir_symbols[parameter.index()]
                .ok_or("native parameter missing resolved identity")?;
            let words = layouts.words(ty)?;
            let mut values = Vec::with_capacity(words);
            for _ in 0..words {
                values.push(builder.ins().load(
                    types::I64,
                    MemFlagsData::new(),
                    args,
                    i32::try_from(offset).map_err(|_| "native argument offset overflow")?,
                ));
                offset = offset
                    .checked_add(8)
                    .ok_or("native argument offset overflow")?;
            }
            locals.insert(symbol, values);
        }
        let mut lower = Lower {
            local_instances: BTreeMap::new(),
            guarded: false, frame_charge: None,
            mir: graph,
            layouts: &layouts,
            builder,
            locals,
            module,
            context,
            object_helper,
            functions,
            function_key: key,
            return_pointer: out,
            return_type: output,
        };
        lower.enter_call(key.node);
        // Charge once per generated function/initializer invocation. Straight-line
        // expression lowering must not turn fuel into an instruction cost model.
        lower.charge_fuel(key.node);
        for (index, (symbol, ty)) in lower
            .functions
            .captures
            .get(&key)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .enumerate()
        {
            let index = lower.builder.ins().iconst(types::I64, index as i64);
            let value = lower
                .object(key.node, helpers::CAPTURE, ty, environment, index)
                .map_err(|e| format!("native capture load: {e:?}"))?;
            lower.locals.insert(symbol, value);
        }
        lower.load_instance_captures(key, environment).map_err(|e| format!("native instance capture: {e:?}"))?;
        if !key.initializer && let Some(&symbol) = lower.functions.self_bindings.get(&key.node)
            && !lower.locals.contains_key(&symbol)
            && !key.instance.is_some_and(|id| lower.local_instances.contains_key(&id)) {
            let ty = TypeKey::try_from(key.ty(graph, key.node)?)?;
            let id = lower.builder.ins().iconst(types::I64, i64::from(function.as_u32()));
            let value = lower.object(key.node, helpers::SELF_CLOSURE, ty, environment, id)
                .map_err(|e| format!("native recursive closure: {e:?}"))?;
            lower.locals.insert(symbol, value.clone());
            if let Some(instance) = key.instance
                && graph.generic_instances[instance.index()].symbol == symbol {
                lower.local_instances.insert(instance, value);
            }
        }
        let outcome = if uncallable {
            lower.report_failure(key.node, "function with uninhabited parameter was called")
                .and_then(|()| Err(EmitError::Diverged))
        } else if let Some(constructor) = constructor(graph, key.node).filter(|_| !key.initializer)
        {
            (|| {
                if arguments.len() != 1 {
                    return Err("native constructor signature needs one payload".into());
                }
                let mut payload = Vec::new();
                for index in 0..layouts.words(arguments[0])? {
                    payload.push(
                        lower.builder.ins().load(
                            types::I64,
                            MemFlagsData::new(),
                            args,
                            i32::try_from(index * 8)
                                .map_err(|_| "native constructor argument overflow")?,
                        ),
                    );
                }
                let result = match constructor {
                    MemberSelection::EnumVariant { index } => lower.enum_constructor(key.node, output, index, &payload)?,
                    MemberSelection::NewtypeConstructor => {
                        lower.construction_check(key.node, output, telora_core::mir::PropertySite::Type, &payload)?;
                        let data = lower.stack_words(&payload)?;
                        let count = lower.builder.ins().iconst(types::I64, 1);
                        lower.object(key.node, helpers::AGGREGATE, output, data, count)?
                    }
                    _ => unreachable!(),
                };
                lower.write_return(&result)
            })()
        } else if matches!(graph.hir[key.node.index()].kind, HirKind::Interpreter) && !key.initializer {
            lower.interpreter_adapter(&arguments, args, environment)
        } else if is_native(&graph.hir[key.node.index()].kind) && !key.initializer {
            lower.native_adapter(key.node, &arguments, args, environment)
        } else {
            match lower.expression(body, 0) {
                Ok(result) => lower.return_value(body, &result),
                Err(error) => Err(error),
            }
        };
        match outcome {
            Ok(()) | Err(EmitError::Diverged) => {}
            Err(EmitError::Message(message)) => return Err(message),
        }
        let config = lower.module.target_config();
        lower.seal_frame_charge();
        lower.builder.finalize(config);
    }
    module
        .define_function(function, &mut ctx)
        .map_err(|e| e.to_string())?;

    Ok(())
}
