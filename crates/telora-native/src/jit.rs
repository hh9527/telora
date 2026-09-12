//! First mechanical SealedMir JIT slice. Unsupported nodes fail during lowering.
use crate::abi::{CallContext, Layouts, Origin, Result, TypeKey, Value};
use cranelift_codegen::ir::{self, AbiParam, InstBuilder, MemFlagsData, types};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module};
use std::collections::BTreeMap;
use telora_core::mir::{
    HirId, HirKind, MemberSelection, Mir, ResolveState, Role, SealedMir, SymbolId, TypeConstructor,
    TypeState,
};

type Entry = unsafe extern "C" fn(*mut CallContext, *const u64, *mut u64) -> u32;

/// Owns executable memory. No raw entry address escapes this owner.
pub struct Compiled {
    _memory: CodeMemory,
    entry: Entry,
    layouts: Layouts,
    arguments: Vec<TypeKey>,
    output: TypeKey,
}
/// Also frees allocations if compilation exits early with an error.
struct CodeMemory(Option<JITModule>);
impl std::ops::Deref for CodeMemory {
    type Target = JITModule;
    fn deref(&self) -> &JITModule {
        self.0.as_ref().unwrap()
    }
}
impl std::ops::DerefMut for CodeMemory {
    fn deref_mut(&mut self) -> &mut JITModule {
        self.0.as_mut().unwrap()
    }
}
impl Drop for CodeMemory {
    fn drop(&mut self) {
        if let Some(module) = self.0.take() {
            // SAFETY: entry never escapes, calls borrow self, and no code is active at drop.
            unsafe {
                module.free_memory();
            }
        }
    }
}
impl Compiled {
    pub fn arguments(&self) -> &[TypeKey] {
        &self.arguments
    }
    pub fn output(&self) -> TypeKey {
        self.output
    }
    pub fn layouts(&self) -> &Layouts {
        &self.layouts
    }
    pub fn call(&self, context: &mut CallContext, values: &[Value]) -> Result<Value> {
        if values.len() != self.arguments.len() {
            return Err("native argument count mismatch".into());
        }
        let mut args = Vec::new();
        for (value, &ty) in values.iter().zip(&self.arguments) {
            if value.type_key() != ty || value.words().len() != self.layouts.words(ty)? {
                return Err("native argument type/width mismatch".into());
            }
            args.extend_from_slice(value.words());
        }
        let words = self.layouts.words(self.output)?;
        let mut result = vec![0; words].into_boxed_slice();
        // SAFETY: only our verified signature is transmuted; buffers have checked
        // widths, stay alive and do not move during execution. Generated code may
        // neither retain pointers nor unwind across this ABI.
        let status = unsafe { (self.entry)(context, args.as_ptr(), result.as_mut_ptr()) };
        match status {
            0 => Value::from_result(result, self.output, words),
            1 => Err("native execution failed".into()),
            _ => Err("invalid native return status".into()),
        }
    }
}

fn known(mir: &Mir, node: HirId) -> Result<telora_core::mir::TypeId> {
    match mir.ty_slots.get(node.index()) {
        Some(TypeState::Known(ty)) => Ok(*ty),
        _ => Err(format!(
            "native requires a solved type at HIR {}",
            node.index()
        )),
    }
}
fn child(mir: &Mir, node: HirId, role: Role) -> Result<HirId> {
    mir.hir[node.index()]
        .children
        .iter()
        .find(|e| e.role == role)
        .map(|e| e.node)
        .ok_or_else(|| format!("native missing {role:?} at HIR {}", node.index()))
}

/// Compile one closed expression or a non-capturing monomorphic function.
/// This does not initialize a module or silently skip its top-level effects.
pub fn compile(mir: &SealedMir<'_>, root: HirId) -> Result<Compiled> {
    let layouts = Layouts::from_mir(mir)?;
    let graph = mir.mir();
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
    let output_words = layouts.words(output)?;
    let arguments = parameters
        .iter()
        .map(|&p| TypeKey::try_from(known(graph, p)?))
        .collect::<Result<Vec<_>>>()?;
    let builder =
        JITBuilder::new(cranelift_module::default_libcall_names()).map_err(|e| e.to_string())?;
    let mut module = CodeMemory(Some(JITModule::new(builder)));
    let mut ctx = module.make_context();
    let pointer = module.target_config().pointer_type();
    ctx.func.signature.params = vec![AbiParam::new(pointer); 3];
    ctx.func.signature.returns.push(AbiParam::new(types::I32));
    let function = module
        .declare_function("telora_entry", Linkage::Export, &ctx.func.signature)
        .map_err(|e| e.to_string())?;
    let mut fbctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut fbctx);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let args = builder.block_params(entry)[1];
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
        lower.builder.finalize(module.target_config());
    }
    module
        .define_function(function, &mut ctx)
        .map_err(|e| e.to_string())?;
    module.clear_context(&mut ctx);
    module.finalize_definitions().map_err(|e| e.to_string())?;
    let address = module.get_finalized_function(function);
    // SAFETY: target default C ABI, exactly three pointers and a u32 status.
    let entry = unsafe { std::mem::transmute::<*const u8, Entry>(address) };
    Ok(Compiled {
        _memory: module,
        entry,
        layouts,
        arguments,
        output,
    })
}

struct Lower<'a, 'b> {
    mir: &'a Mir,
    layouts: &'a Layouts,
    builder: FunctionBuilder<'b>,
    locals: BTreeMap<SymbolId, Vec<ir::Value>>,
}
impl Lower<'_, '_> {
    fn expression(&mut self, node: HirId, depth: usize) -> Result<Vec<ir::Value>> {
        if depth > 512 {
            return Err("native expression nesting limit".into());
        }
        let ty = known(self.mir, node)?;
        let key = TypeKey::try_from(ty)?;
        let syntax = &self.mir.hir[node.index()];
        let data = match &syntax.kind {
            HirKind::Int(bits) => Some(vec![*bits as u64]),
            HirKind::Float(bits) => Some(vec![bits.to_bits()]),
            HirKind::Tuple if syntax.children.is_empty() => Some(vec![]),
            _ if matches!(
                self.mir.member_selections.get(node.index()),
                Some(Some(MemberSelection::Boolean(_)))
            ) =>
            {
                let Some(Some(MemberSelection::Boolean(value))) =
                    self.mir.member_selections.get(node.index())
                else {
                    unreachable!()
                };
                Some(vec![u64::from(*value)])
            }
            _ => None,
        };
        if let Some(data) = data {
            let value = self
                .layouts
                .value(key, Origin::from_loc(Some(syntax.location)), &data)?;
            return Ok(value
                .words()
                .iter()
                .map(|&w| self.builder.ins().iconst(types::I64, w as i64))
                .collect());
        }
        match syntax.kind {
            HirKind::Variable(_) => {
                let slot = syntax
                    .resolution
                    .ok_or("native variable missing resolution")?;
                let ResolveState::Bound(symbol) = self.mir.resolve_slots[slot.index()] else {
                    return Err("native reference is not bound".into());
                };
                self.locals.get(&symbol).cloned().ok_or_else(|| {
                    format!("native unsupported capture/export at {:?}", syntax.location)
                })
            }
            HirKind::TypeAscription => {
                self.expression(child(self.mir, node, Role::Value)?, depth + 1)
            }
            HirKind::Block => {
                // Do not discard bindings or effects to get a successful result.
                if syntax.children.iter().any(|e| e.role != Role::Result) {
                    return Err(format!(
                        "native unsupported block statements at {:?}",
                        syntax.location
                    ));
                }
                self.expression(child(self.mir, node, Role::Result)?, depth + 1)
            }
            HirKind::If => {
                let condition_node = child(self.mir, node, Role::Condition)?;
                let condition_ty = known(self.mir, condition_node)?;
                if self.mir.types[condition_ty.index()].constructor != TypeConstructor::Bool {
                    return Err("native if condition is not solved Bool".into());
                }
                let condition = self.expression(condition_node, depth + 1)?;
                let yes = self.builder.create_block();
                let no = self.builder.create_block();
                let merge = self.builder.create_block();
                for _ in 0..self.layouts.words(key)? {
                    self.builder.append_block_param(merge, types::I64);
                }
                self.builder.ins().brif(condition[2], yes, &[], no, &[]);
                self.builder.switch_to_block(yes);
                self.builder.seal_block(yes);
                let a = self.expression(child(self.mir, node, Role::Then)?, depth + 1)?;
                let a = a.into_iter().map(ir::BlockArg::from).collect::<Vec<_>>();
                self.builder.ins().jump(merge, &a);
                self.builder.switch_to_block(no);
                self.builder.seal_block(no);
                let b = self.expression(child(self.mir, node, Role::Else)?, depth + 1)?;
                let b = b.into_iter().map(ir::BlockArg::from).collect::<Vec<_>>();
                self.builder.ins().jump(merge, &b);
                self.builder.switch_to_block(merge);
                self.builder.seal_block(merge);
                Ok(self.builder.block_params(merge).to_vec())
            }
            _ => Err(format!(
                "native unsupported {:?} at {:?}",
                syntax.kind, syntax.location
            )),
        }
    }
}
#[cfg(test)]
mod tests;
