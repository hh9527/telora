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

#[derive(Debug)]
enum EmitError {
    Message(String),
    Diverged,
}
impl From<String> for EmitError {
    fn from(value: String) -> Self {
        Self::Message(value)
    }
}
impl From<&str> for EmitError {
    fn from(value: &str) -> Self {
        Self::Message(value.into())
    }
}
type EmitResult<T> = std::result::Result<T, EmitError>;

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
        let never = self.layouts.is_never(self.output)?;
        let words = if never {
            0
        } else {
            self.layouts.words(self.output)?
        };
        let mut result = vec![0; words].into_boxed_slice();
        // SAFETY: only our verified signature is transmuted; buffers have checked
        // widths, stay alive and do not move during execution. Generated code may
        // neither retain pointers nor unwind across this ABI.
        let status = unsafe { (self.entry)(context, args.as_ptr(), result.as_mut_ptr()) };
        match status {
            0 => {
                if never {
                    return Err("native Never function returned successfully".into());
                }
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
    let (_, _, output, arguments) = functions::shape(graph, &layouts, root.into())?;
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
    let mut functions = functions::Functions::new(ctx.func.signature.clone());
    functions.registered.insert(root.into(), function);
    functions.pending.push(root.into());
    let mut next = 0;
    while next < functions.pending.len() {
        let node = functions.pending[next];
        next += 1;
        functions::emit(graph, &layouts, node, helper, &mut module, &mut functions)?;
    }
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
    functions: &'a mut functions::Functions,
    function_key: functions::Key,
    return_pointer: ir::Value,
    return_type: TypeKey,
}
impl Lower<'_, '_> {
    fn return_value(&mut self, result: &[ir::Value]) -> EmitResult<()> {
        if self.layouts.is_never(self.return_type)? {
            return Err("native Never body produced a value".into());
        }
        if result.len() != self.layouts.words(self.return_type)? {
            return Err("native return width mismatch".into());
        }
        for (i, &value) in result.iter().enumerate() {
            self.builder.ins().store(
                MemFlagsData::new(),
                value,
                self.return_pointer,
                i32::try_from(i.checked_mul(8).ok_or("native return overflow")?)
                    .map_err(|_| "native return overflow")?,
            );
        }
        let status = self.builder.ins().iconst(types::I32, 0);
        self.builder.ins().return_(&[status]);
        Ok(())
    }
    fn report_failure(&mut self, node: HirId, message: &str) -> EmitResult<()> {
        let (data, count) = self.literal_bytes(message)?;
        let operation = self.builder.ins().iconst(types::I32, helpers::FAIL as i64);
        let ty = self.builder.ins().iconst(types::I32, 0);
        let origin = Origin::from_loc(Some(self.mir.hir[node.index()].location)).words();
        let loc0 = self.builder.ins().iconst(
            types::I64,
            (u64::from(origin[0]) | (u64::from(origin[1]) << 32)) as i64,
        );
        let end = self.builder.ins().iconst(types::I32, origin[2] as i64);
        let zero = self.builder.ins().iconst(types::I64, 0);
        let out = self.stack_words(&[zero])?;
        let call = self.builder.ins().call(
            self.object_helper,
            &[self.context, operation, ty, loc0, end, data, count, out],
        );
        let status = self.builder.inst_results(call)[0];
        self.builder.ins().return_(&[status]);
        Ok(())
    }
    fn selected_member(&self, node: HirId) -> Option<MemberSelection> {
        if let Some(selected) = self.mir.member_selections[node.index()] {
            return Some(selected);
        }
        let slot = self.mir.hir[node.index()].resolution?;
        let ResolveState::Bound(symbol) = self.mir.resolve_slots[slot.index()] else {
            return None;
        };
        self.mir.symbols[symbol.index()]
            .declarations
            .iter()
            .find_map(|&declaration| {
                let value = child(self.mir, declaration, Role::Value).ok()?;
                self.mir.member_selections[value.index()]
            })
    }
    fn enum_constructor(
        &mut self,
        node: HirId,
        ty: TypeKey,
        index: u32,
        payload: &[ir::Value],
    ) -> EmitResult<Vec<ir::Value>> {
        let expected = self.layouts.variant_payloads[ty.index()]
            .get(index as usize)
            .ok_or("native enum has no solved variant")?;
        let width = expected
            .map(|t| self.layouts.words(t))
            .transpose()?
            .unwrap_or(0);
        if payload.len() != width {
            return Err("native enum payload width does not match solved variant".into());
        }
        if self
            .mir
            .construction_checks
            .iter()
            .any(|c| c.owner.index() == ty.index() && c.concrete)
        {
            return Err("native enum construction checks are not yet linked".into());
        }
        let data = self.stack_words(payload)?;
        let tag = self.builder.ins().iconst(types::I64, index as i64);
        self.object(node, helpers::ENUM, ty, data, tag)
    }
    fn ty(&self, node: HirId) -> EmitResult<telora_core::mir::TypeId> {
        self.function_key.ty(self.mir, node).map_err(Into::into)
    }
    fn instance_reference(&self, mut node: HirId) -> Option<telora_core::mir::GenericInstanceId> {
        loop {
            let selected = match self.function_key.instance {
                Some(id) => self.mir.generic_instances[id.index()].reference(node),
                None => self.mir.generic_references[node.index()].and_then(|r| r.instance()),
            };
            if selected.is_some() {
                return selected;
            }
            // Explicit type application wraps the resolved callee reference.
            // Consume that reference's existing instance; do not substitute here.
            node = match self.mir.hir[node.index()].kind {
                HirKind::TypeApply => child(self.mir, node, Role::Callee).ok()?,
                HirKind::TypeAscription => child(self.mir, node, Role::Value).ok()?,
                _ => return None,
            };
        }
    }
    fn callable(&self, node: HirId, depth: usize) -> EmitResult<HirId> {
        if depth > 512 {
            return Err("native callable alias cycle".into());
        }
        match self.mir.hir[node.index()].kind {
            HirKind::Closure => Ok(node),
            HirKind::TypeApply => self.callable(child(self.mir, node, Role::Callee)?, depth + 1),
            HirKind::TypeAscription => {
                self.callable(child(self.mir, node, Role::Value)?, depth + 1)
            }
            HirKind::Variable(_) => {
                let slot = self.mir.hir[node.index()]
                    .resolution
                    .ok_or("native callable missing resolve slot")?;
                let ResolveState::Bound(symbol) = self.mir.resolve_slots[slot.index()] else {
                    return Err("native callable not bound".into());
                };
                let declaration = self.mir.symbols[symbol.index()]
                    .declarations
                    .iter()
                    .find_map(|&decl| {
                        self.mir.hir[decl.index()]
                            .children
                            .iter()
                            .find(|e| e.role == Role::Value)
                            .map(|e| e.node)
                    })
                    .ok_or("native indirect/native callable is not yet linked")?;
                self.callable(declaration, depth + 1)
            }
            _ => Err("native indirect callable is not yet linked".into()),
        }
    }
    fn direct_call(&mut self, node: HirId, depth: usize) -> EmitResult<Vec<ir::Value>> {
        let callee_node = child(self.mir, node, Role::Callee)?;
        if let Some(MemberSelection::EnumVariant { index }) = self.selected_member(callee_node) {
            let arguments = self.mir.hir[node.index()]
                .children
                .iter()
                .filter(|e| e.role == Role::Argument)
                .map(|e| e.node)
                .collect::<Vec<_>>();
            if arguments.len() != 1 {
                return Err("native enum constructor requires one payload".into());
            }
            let payload = self.expression(arguments[0], depth + 1)?;
            return self.enum_constructor(
                node,
                TypeKey::try_from(self.ty(node)?)?,
                index,
                &payload,
            );
        }
        let callee = functions::Key {
            node: self.callable(callee_node, 0)?,
            instance: self.instance_reference(callee_node),
        };
        let function = self.functions.declare(self.mir, callee, self.module)?;
        let (parameters, _, output, expected) = functions::shape(self.mir, self.layouts, callee)
            .map_err(|e| format!("{e}; callee {callee:?} from HIR {}, parent {:?}, reference {:?}, callee syntax {:?}", node.index(), self.function_key, self.mir.generic_references[callee_node.index()], self.mir.hir[callee_node.index()].kind))?;
        let arguments = self.mir.hir[node.index()]
            .children
            .iter()
            .filter(|e| e.role == Role::Argument)
            .map(|e| e.node)
            .collect::<Vec<_>>();
        if parameters.len() != arguments.len() {
            return Err("native direct argument count mismatch".into());
        }
        if output != TypeKey::try_from(self.ty(node)?)? {
            return Err("native direct result type mismatch".into());
        }
        let mut words = Vec::new();
        for (&arg, ty) in arguments.iter().zip(expected) {
            if TypeKey::try_from(self.ty(arg)?)? != ty {
                return Err("native direct argument type mismatch".into());
            }
            words.extend(self.expression(arg, depth + 1)?);
        }
        let data = self.stack_words(&words)?;
        let never = self.layouts.is_never(output)?;
        let width = if never {
            0
        } else {
            self.layouts.words(output)?
        };
        let zero = self.builder.ins().iconst(types::I64, 0);
        let out = self.stack_words(&vec![zero; width])?;
        let callee = self
            .module
            .declare_func_in_func(function, self.builder.func);
        let call = self.builder.ins().call(callee, &[self.context, data, out]);
        let status = self.builder.inst_results(call)[0];
        let failed = self.builder.create_block();
        let success = self.builder.create_block();
        self.builder.ins().brif(status, failed, &[], success, &[]);
        self.builder.switch_to_block(failed);
        self.builder.seal_block(failed);
        self.builder.ins().return_(&[status]);
        self.builder.switch_to_block(success);
        self.builder.seal_block(success);
        if never {
            self.report_failure(node, "native Never function returned unexpectedly")?;
            return Err(EmitError::Diverged);
        }
        Ok((0..width)
            .map(|i| {
                self.builder
                    .ins()
                    .load(types::I64, MemFlagsData::new(), out, (i * 8) as i32)
            })
            .collect())
    }
    fn stack_words(&mut self, values: &[ir::Value]) -> EmitResult<ir::Value> {
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
    ) -> EmitResult<Vec<ir::Value>> {
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
    fn literal_bytes(&mut self, text: &str) -> EmitResult<(ir::Value, ir::Value)> {
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
        Ok((pointer, length))
    }
    fn string(&mut self, node: HirId, ty: TypeKey, text: &str) -> EmitResult<Vec<ir::Value>> {
        let (pointer, length) = self.literal_bytes(text)?;
        self.object(node, helpers::STRING, ty, pointer, length)
    }
    fn expression(&mut self, node: HirId, depth: usize) -> EmitResult<Vec<ir::Value>> {
        if depth > 512 {
            return Err("native expression nesting limit".into());
        }
        let ty = self.ty(node)?;
        let key = TypeKey::try_from(ty)?;
        let syntax = &self.mir.hir[node.index()];
        if let Some(MemberSelection::EnumVariant { index }) = self.selected_member(node)
            && self.mir.types[ty.index()].constructor != TypeConstructor::Function
        {
            return self.enum_constructor(node, key, index, &[]);
        }
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
            HirKind::Match | HirKind::IfLet | HirKind::LetElse => self.pattern_branch(node, depth),
            HirKind::Return => {
                let value = self.expression(child(self.mir, node, Role::Value)?, depth + 1)?;
                self.return_value(&value)?;
                Err(EmitError::Diverged)
            }
            HirKind::Panic | HirKind::Raise(telora_core::ast::BlameAction::Fail) => {
                if syntax.children.iter().any(|e| e.role == Role::Subject) {
                    return Err("native fail subjects are not yet linked".into());
                }
                let message = child(self.mir, node, Role::Value)?;
                let HirKind::String(message) = &self.mir.hir[message.index()].kind else {
                    return Err("native dynamic fail message is not yet linked".into());
                };
                self.report_failure(node, message)?;
                Err(EmitError::Diverged)
            }
            HirKind::Binary(operation) => self.binary(node, operation, depth),
            HirKind::Unary(operation) => self.unary(node, operation, depth),
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
                let receiver_ty = TypeKey::try_from(self.ty(receiver_node)?)?;
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
                if self.mir.types[self.ty(receiver_node)?.index()].constructor
                    != TypeConstructor::Array
                {
                    return Err("native index currently requires solved Array".into());
                }
                let receiver = self.expression(receiver_node, depth + 1)?;
                let index_node = child(self.mir, node, Role::Index)?;
                if self.mir.types[self.ty(index_node)?.index()].constructor != TypeConstructor::Int
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
                if let Some(value) = self.locals.get(&symbol) {
                    return Ok(value.clone());
                }
                // Resolved exports of a statically selected Boolean member are
                // constants. Never recognize prelude names or execute a provider.
                for &declaration in &self.mir.symbols[symbol.index()].declarations {
                    if let Ok(value) = child(self.mir, declaration, Role::Value)
                        && matches!(
                            self.mir.member_selections[value.index()],
                            Some(MemberSelection::Boolean(_))
                        )
                    {
                        return self.expression(value, depth + 1);
                    }
                }
                Err(format!("native unsupported capture/export at {:?}", syntax.location).into())
            }
            HirKind::TypeAscription => {
                self.expression(child(self.mir, node, Role::Value)?, depth + 1)
            }
            HirKind::Binding { .. } => {
                let value = self.expression(child(self.mir, node, Role::Value)?, depth + 1)?;
                let symbol = self.mir.hir_symbols[node.index()]
                    .ok_or("native binding pattern is not yet supported")?;
                self.locals.insert(symbol, value.clone());
                Ok(value)
            }
            HirKind::Closure => {
                let function = self.functions.declare(
                    self.mir,
                    functions::Key {
                        node,
                        instance: self.function_key.instance,
                    },
                    self.module,
                )?;
                let value = self.layouts.value(
                    key,
                    Origin::from_loc(Some(syntax.location)),
                    &[function.as_u32() as u64],
                )?;
                Ok(value
                    .words()
                    .iter()
                    .map(|&w| self.builder.ins().iconst(types::I64, w as i64))
                    .collect())
            }
            HirKind::Call => self.direct_call(node, depth),
            HirKind::Block => {
                for edge in &syntax.children {
                    match edge.role {
                        Role::Binding => {
                            self.expression(edge.node, depth + 1)?;
                        }
                        Role::Result => {}
                        _ => return Err("native unsupported block child".into()),
                    }
                }
                self.expression(child(self.mir, node, Role::Result)?, depth + 1)
            }
            HirKind::If => {
                let condition_node = child(self.mir, node, Role::Condition)?;
                let condition_ty = self.ty(condition_node)?;
                if self.mir.types[condition_ty.index()].constructor != TypeConstructor::Bool {
                    return Err("native if condition is not solved Bool".into());
                }
                let condition = self.expression(condition_node, depth + 1)?;
                let yes = self.builder.create_block();
                let no = self.builder.create_block();
                let merge = self.builder.create_block();
                let width = if self.layouts.is_never(key)? {
                    0
                } else {
                    self.layouts.words(key)?
                };
                for _ in 0..width {
                    self.builder.append_block_param(merge, types::I64);
                }
                self.builder.ins().brif(condition[2], yes, &[], no, &[]);
                self.builder.switch_to_block(yes);
                self.builder.seal_block(yes);
                let mut live = 0;
                match self.expression(child(self.mir, node, Role::Then)?, depth + 1) {
                    Ok(values) => {
                        if values.len() != width {
                            return Err("native branch width mismatch".into());
                        }
                        let values = values
                            .into_iter()
                            .map(ir::BlockArg::from)
                            .collect::<Vec<_>>();
                        self.builder.ins().jump(merge, &values);
                        live += 1;
                    }
                    Err(EmitError::Diverged) => {}
                    Err(error) => return Err(error),
                }
                self.builder.switch_to_block(no);
                self.builder.seal_block(no);
                match self.expression(child(self.mir, node, Role::Else)?, depth + 1) {
                    Ok(values) => {
                        if values.len() != width {
                            return Err("native branch width mismatch".into());
                        }
                        let values = values
                            .into_iter()
                            .map(ir::BlockArg::from)
                            .collect::<Vec<_>>();
                        self.builder.ins().jump(merge, &values);
                        live += 1;
                    }
                    Err(EmitError::Diverged) => {}
                    Err(error) => return Err(error),
                }
                if live == 0 {
                    return Err(EmitError::Diverged);
                }
                self.builder.switch_to_block(merge);
                self.builder.seal_block(merge);
                Ok(self.builder.block_params(merge).to_vec())
            }
            _ => Err(format!(
                "native unsupported {:?} at {:?}",
                syntax.kind, syntax.location
            )
            .into()),
        }
    }
}
#[path = "jit/functions.rs"]
mod functions;
#[path = "jit/patterns.rs"]
mod patterns;
#[path = "jit/scalars.rs"]
mod scalars;
#[cfg(test)]
mod tests;
