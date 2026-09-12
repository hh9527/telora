use super::*;
use cranelift_module::FuncId;
use telora_core::mir::GenericInstanceId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Key {
    pub node: HirId,
    pub instance: Option<GenericInstanceId>,
}
impl Key {
    pub fn ty(self, mir: &Mir, node: HirId) -> Result<telora_core::mir::TypeId> {
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
        }
    }
}

pub(super) struct Functions {
    pub registered: BTreeMap<Key, FuncId>,
    pub pending: Vec<Key>,
    signature: ir::Signature,
}
impl Functions {
    pub fn new(signature: ir::Signature) -> Self {
        Self {
            registered: BTreeMap::new(),
            pending: vec![],
            signature,
        }
    }
    pub fn declare(&mut self, mir: &Mir, key: Key, module: &mut JITModule) -> Result<FuncId> {
        if let Some(&function) = self.registered.get(&key) {
            return Ok(function);
        }
        let node = key.node;
        if !matches!(mir.hir[node.index()].kind, HirKind::Closure)
            || mir.types[key.ty(mir, node)?.index()].constructor != TypeConstructor::Function
        {
            return Err("native direct call requires a monomorphic closure".into());
        }
        let function = module
            .declare_function(
                &format!(
                    "telora_fn_{}_{:?}",
                    node.index(),
                    key.instance.map(|i| i.index())
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
    let is_function = matches!(root_node.kind, HirKind::Closure);
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
    let body = if is_function {
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
    let arguments = parameters
        .iter()
        .map(|&p| TypeKey::try_from(key.ty(graph, p)?))
        .collect::<Result<Vec<_>>>()?;

    if !layouts.is_never(output)? {
        layouts.words(output)?;
    }
    for &argument in &arguments {
        layouts.words(argument)?;
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
        let mut locals = BTreeMap::new();
        let mut offset: usize = 0;
        for (&parameter, &ty) in parameters.iter().zip(&arguments) {
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
        let outcome = match lower.expression(body, 0) {
            Ok(result) => lower.return_value(&result),
            Err(error) => Err(error),
        };
        match outcome {
            Ok(()) | Err(EmitError::Diverged) => {}
            Err(EmitError::Message(message)) => return Err(message),
        }
        let config = lower.module.target_config();
        lower.builder.finalize(config);
    }
    module
        .define_function(function, &mut ctx)
        .map_err(|e| e.to_string())?;

    Ok(())
}
