use super::*;
use cranelift_module::FuncId;

pub(super) struct Functions {
    pub registered: BTreeMap<HirId, FuncId>,
    pub pending: Vec<HirId>,
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
    pub fn declare(&mut self, mir: &Mir, node: HirId, module: &mut JITModule) -> Result<FuncId> {
        if let Some(&function) = self.registered.get(&node) {
            return Ok(function);
        }
        if !matches!(mir.hir[node.index()].kind, HirKind::Closure)
            || mir.types[known(mir, node)?.index()].constructor != TypeConstructor::Function
        {
            return Err("native direct call requires a monomorphic closure".into());
        }
        let function = module
            .declare_function(
                &format!("telora_fn_{}", node.index()),
                Linkage::Local,
                &self.signature,
            )
            .map_err(|e| e.to_string())?;
        self.registered.insert(node, function);
        self.pending.push(node);
        Ok(function)
    }
}
pub(super) fn shape(
    graph: &Mir,
    layouts: &Layouts,
    root: HirId,
) -> Result<(Vec<HirId>, HirId, TypeKey, Vec<TypeKey>)> {
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
    let output = TypeKey::try_from(known(graph, body)?)?;
    let arguments = parameters
        .iter()
        .map(|&p| TypeKey::try_from(known(graph, p)?))
        .collect::<Result<Vec<_>>>()?;

    layouts.words(output)?;
    for &argument in &arguments {
        layouts.words(argument)?;
    }
    Ok((parameters, body, output, arguments))
}
pub(super) fn emit(
    graph: &Mir,
    layouts: &Layouts,
    root: HirId,
    helper: FuncId,
    module: &mut JITModule,
    functions: &mut Functions,
) -> Result<()> {
    let (parameters, body, output, arguments) = shape(graph, layouts, root)?;
    let output_words = layouts.words(output)?;
    let function = functions.registered[&root];
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
        };
        let result = lower.expression(body, 0)?;
        if result.len() != output_words {
            return Err("native result shape mismatch".into());
        }
        for (i, value) in result.into_iter().enumerate() {
            lower.builder.ins().store(
                MemFlagsData::new(),
                value,
                out,
                i32::try_from(i.checked_mul(8).ok_or("native result overflow")?)
                    .map_err(|_| "native result overflow")?,
            );
        }
        let success = lower.builder.ins().iconst(types::I32, 0);
        lower.builder.ins().return_(&[success]);
        let config = lower.module.target_config();
        lower.builder.finalize(config);
    }
    module
        .define_function(function, &mut ctx)
        .map_err(|e| e.to_string())?;

    Ok(())
}
