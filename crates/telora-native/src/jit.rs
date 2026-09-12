//! First mechanical SealedMir JIT slice. Unsupported nodes fail during lowering.
use crate::abi::{CallContext, Layouts, Origin, Result, TypeKey, Value};
use crate::runtime::helpers;
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
            if let Ok(runtime) = context.runtime() {
                runtime.check_argument(value)?;
            } else if value.arena != 0 {
                return Err("native heap argument requires its runtime".into());
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
            0 => {
                let mut value = Value::from_result(result, self.output, words)?;
                value.arena = context.runtime().map_or(0, |r| r.identity());
                Ok(value)
            }
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
    let mut builder =
        JITBuilder::new(cranelift_module::default_libcall_names()).map_err(|e| e.to_string())?;
    builder.symbol("telora_native_object", helpers::object as *const u8);
    let mut module = CodeMemory(Some(JITModule::new(builder)));
    let mut ctx = module.make_context();
    let pointer = module.target_config().pointer_type();
    ctx.func.signature.params = vec![AbiParam::new(pointer); 3];
    ctx.func.signature.returns.push(AbiParam::new(types::I32));
    let function = module
        .declare_function("telora_entry", Linkage::Export, &ctx.func.signature)
        .map_err(|e| e.to_string())?;
    let mut helper_signature = module.make_signature();
    helper_signature.params = [
        pointer,
        types::I32,
        types::I32,
        types::I64,
        types::I32,
        pointer,
        types::I64,
        pointer,
    ]
    .into_iter()
    .map(AbiParam::new)
    .collect();
    helper_signature.returns.push(AbiParam::new(types::I32));
    let helper = module
        .declare_function("telora_native_object", Linkage::Import, &helper_signature)
        .map_err(|e| e.to_string())?;
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
            module: &mut module,
            context,
            object_helper,
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
    module: &'a mut JITModule,
    context: ir::Value,
    object_helper: ir::FuncRef,
}
impl Lower<'_, '_> {
    fn stack_words(&mut self, values: &[ir::Value]) -> Result<ir::Value> {
        let bytes = u32::try_from(
            values
                .len()
                .checked_mul(8)
                .ok_or("native stack size overflow")?
                .max(8),
        )
        .map_err(|_| "native stack size overflow")?;
        if bytes > i32::MAX as u32 {
            return Err("native stack offset overflow".into());
        }
        let slot = self.builder.create_sized_stack_slot(ir::StackSlotData::new(
            ir::StackSlotKind::ExplicitSlot,
            bytes,
            3,
        ));
        for (i, &value) in values.iter().enumerate() {
            self.builder.ins().stack_store(
                self.module.target_config().pointer_type(),
                value,
                slot,
                (i * 8) as i32,
            );
        }
        Ok(self
            .builder
            .ins()
            .stack_addr(self.module.target_config().pointer_type(), slot, 0))
    }
    fn object(
        &mut self,
        node: HirId,
        operation: u32,
        ty: TypeKey,
        data: ir::Value,
        count: ir::Value,
    ) -> Result<Vec<ir::Value>> {
        let width = self.layouts.words(ty)?;
        let zero = self.builder.ins().iconst(types::I64, 0);
        let out = self.stack_words(&vec![zero; width])?;
        let origin = Origin::from_loc(Some(self.mir.hir[node.index()].location)).words();
        let operation = self.builder.ins().iconst(types::I32, operation as i64);
        let ty = self.builder.ins().iconst(types::I32, ty.raw() as i64);
        let loc0 = self.builder.ins().iconst(
            types::I64,
            (u64::from(origin[0]) | (u64::from(origin[1]) << 32)) as i64,
        );
        let end = self.builder.ins().iconst(types::I32, origin[2] as i64);
        let call = self.builder.ins().call(
            self.object_helper,
            &[self.context, operation, ty, loc0, end, data, count, out],
        );
        let status = self.builder.inst_results(call)[0];
        let failed = self.builder.create_block();
        let success = self.builder.create_block();
        self.builder.ins().brif(status, failed, &[], success, &[]);
        self.builder.switch_to_block(failed);
        self.builder.seal_block(failed);
        self.builder.ins().return_(&[status]);
        self.builder.switch_to_block(success);
        self.builder.seal_block(success);
        Ok((0..width)
            .map(|i| {
                self.builder
                    .ins()
                    .load(types::I64, MemFlagsData::new(), out, (i * 8) as i32)
            })
            .collect())
    }
    fn string(&mut self, node: HirId, ty: TypeKey, text: &str) -> Result<Vec<ir::Value>> {
        let id = self
            .module
            .declare_anonymous_data(false, false)
            .map_err(|e| e.to_string())?;
        let mut data = cranelift_module::DataDescription::new();
        // Empty strings still get a valid non-null address.
        data.define(if text.is_empty() {
            vec![0].into_boxed_slice()
        } else {
            text.as_bytes().into()
        });
        self.module
            .define_data(id, &data)
            .map_err(|e| e.to_string())?;
        let global = self.module.declare_data_in_func(id, self.builder.func);
        let pointer = self
            .builder
            .ins()
            .symbol_value(self.module.target_config().pointer_type(), global);
        let length = self.builder.ins().iconst(types::I64, text.len() as i64);
        self.object(node, helpers::STRING, ty, pointer, length)
    }
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
            HirKind::String(ref text) => self.string(node, key, text),
            HirKind::Array | HirKind::Tuple => {
                if self
                    .mir
                    .construction_checks
                    .iter()
                    .any(|c| c.owner == ty && c.concrete)
                {
                    return Err("native construction checks are not yet linked".into());
                }
                let items = syntax
                    .children
                    .iter()
                    .filter(|e| e.role == Role::Item)
                    .map(|e| e.node)
                    .collect::<Vec<_>>();
                let mut values = Vec::new();
                for &item in &items {
                    values.extend(self.expression(item, depth + 1)?);
                }
                let data = self.stack_words(&values)?;
                let count = self.builder.ins().iconst(types::I64, items.len() as i64);
                let operation = if matches!(syntax.kind, HirKind::Array) {
                    helpers::ARRAY
                } else {
                    helpers::AGGREGATE
                };
                self.object(node, operation, key, data, count)
            }
            HirKind::Dict => {
                if self
                    .mir
                    .construction_checks
                    .iter()
                    .any(|c| c.owner == ty && c.concrete)
                {
                    return Err("native construction checks are not yet linked".into());
                }
                let dictionary = self.mir.types[ty.index()].constructor == TypeConstructor::Dict;
                let mut fields = Vec::new();
                for edge in &syntax.children {
                    if edge.role != Role::Field {
                        return Err("native unsupported record child".into());
                    }
                    let field = edge.node;
                    if self.mir.hir[field.index()]
                        .children
                        .iter()
                        .any(|e| e.role == Role::Decorator)
                    {
                        return Err("native field property construction is not yet linked".into());
                    }
                    let name = child(self.mir, field, Role::Name)?;
                    let HirKind::Name(name) = &self.mir.hir[name.index()].kind else {
                        return Err("native field name missing".into());
                    };
                    let value = self.expression(child(self.mir, field, Role::Value)?, depth + 1)?;
                    fields.push((field, name.clone(), value));
                }
                let count = fields.len();
                let mut words = Vec::new();
                if dictionary {
                    let string = self
                        .mir
                        .types
                        .iter()
                        .position(|t| t.constructor == TypeConstructor::String)
                        .ok_or("sealed image has no String")?;
                    let string = self.layouts.type_at(string)?;
                    for (field, name, value) in fields {
                        words.extend(self.string(field, string, &name)?);
                        words.extend(value);
                    }
                } else {
                    let names = self.layouts.field_names[key.index()].clone();
                    if names.len() != fields.len() {
                        return Err("native record skeleton mismatch".into());
                    }
                    for name in names {
                        let index = fields
                            .iter()
                            .position(|(_, n, _)| *n == name)
                            .ok_or("native record field missing")?;
                        words.extend(std::mem::take(&mut fields[index].2));
                    }
                }
                let data = self.stack_words(&words)?;
                let count = self.builder.ins().iconst(types::I64, count as i64);
                self.object(
                    node,
                    if dictionary {
                        helpers::DICT
                    } else {
                        helpers::AGGREGATE
                    },
                    key,
                    data,
                    count,
                )
            }
            HirKind::TupleProjection(index) => {
                let receiver =
                    self.expression(child(self.mir, node, Role::Receiver)?, depth + 1)?;
                let data = self.stack_words(&receiver)?;
                let index = self.builder.ins().iconst(types::I64, index as i64);
                self.object(node, helpers::FIELD, key, data, index)
            }
            HirKind::Field => {
                let receiver_node = child(self.mir, node, Role::Receiver)?;
                let receiver_ty = TypeKey::try_from(known(self.mir, receiver_node)?)?;
                let name = child(self.mir, node, Role::Name)?;
                let HirKind::Name(name) = &self.mir.hir[name.index()].kind else {
                    return Err("native field name missing".into());
                };
                let index = self.layouts.field_names[receiver_ty.index()]
                    .iter()
                    .position(|n| n == name)
                    .ok_or_else(|| format!("native unsupported field access {name}"))?;
                let receiver = self.expression(receiver_node, depth + 1)?;
                let data = self.stack_words(&receiver)?;
                let index = self.builder.ins().iconst(types::I64, index as i64);
                self.object(node, helpers::FIELD, key, data, index)
            }
            HirKind::Index => {
                let receiver_node = child(self.mir, node, Role::Receiver)?;
                if self.mir.types[known(self.mir, receiver_node)?.index()].constructor
                    != TypeConstructor::Array
                {
                    return Err("native index currently requires solved Array".into());
                }
                let receiver = self.expression(receiver_node, depth + 1)?;
                let index_node = child(self.mir, node, Role::Index)?;
                if self.mir.types[known(self.mir, index_node)?.index()].constructor
                    != TypeConstructor::Int
                {
                    return Err("native index currently requires solved Int".into());
                }
                let index = self.expression(index_node, depth + 1)?;
                let data = self.stack_words(&receiver)?;
                self.object(node, helpers::INDEX, key, data, index[2])
            }
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
