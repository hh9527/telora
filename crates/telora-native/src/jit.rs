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

type Entry = unsafe extern "C" fn(*mut CallContext, *const u64, *mut u64, *const u64) -> u32;

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
    identity: u64,
    _memory: CodeMemory,
    entry: Entry,
    layouts: Layouts,
    arguments: Vec<TypeKey>,
    output: TypeKey,
    entries: BTreeMap<HirId, CompiledEntry>,
    demands: Vec<(crate::runtime::DemandKey, TypeKey)>,
    initializers: Vec<(*const u64, Origin)>,
    export_slots: BTreeMap<SymbolId, usize>,
}
struct CompiledEntry {
    entry: Entry,
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
    /// Drive every registered initializer, then publish the complete root set.
    /// Internal reads still use the same demand table and may run ahead.
    pub fn initialize(&self, context: &mut CallContext) -> Result<()> {
        context
            .runtime_mut()?
            .bind_code_plan(self.identity, &self.demands)?;
        if context.runtime()?.is_published() {
            return Ok(());
        }
        let mut failed = false;
        for slot in 0..self.demands.len() {
            if self.read_demand(context, slot).is_err() {
                failed = true;
            }
        }
        if failed {
            return Err("native initialization failed".into());
        }
        context.runtime_mut()?.publish(&[])?;
        Ok(())
    }
    pub fn export(&self, context: &mut CallContext, symbol: SymbolId) -> Result<Value> {
        if !context.runtime()?.is_published() {
            return Err("native exports require successful initialization".into());
        }
        let slot = *self
            .export_slots
            .get(&symbol)
            .ok_or("native export not in code plan")?;
        self.read_demand(context, slot)
    }
    fn read_demand(&self, context: &mut CallContext, slot: usize) -> Result<Value> {
        context
            .runtime_mut()?
            .bind_code_plan(self.identity, &self.demands)?;
        let ty = self.demands[slot].1;
        let width = self.layouts.words(ty)?;
        let mut words = vec![0; width].into_boxed_slice();
        let (address, origin) = self.initializers[slot];
        // SAFETY: these addresses belong to this borrowed owner, each is a
        // zero-argument initializer with this exact closed result width.
        let status = unsafe {
            helpers::demand(
                context,
                ty,
                slot as u64,
                address,
                words.as_mut_ptr(),
                origin,
            )
        };
        if status != 0 {
            return Err("native initialization failed".into());
        }
        let mut value = Value::from_result(words, ty, width)?;
        value.arena = context.runtime()?.identity();
        Ok(value)
    }
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
        self.invoke(context, values, self.entry, &self.arguments, self.output)
    }
    pub fn call_root(
        &self,
        root: HirId,
        context: &mut CallContext,
        values: &[Value],
    ) -> Result<Value> {
        let entry = self
            .entries
            .get(&root)
            .ok_or("native root not in code plan")?;
        self.invoke(context, values, entry.entry, &entry.arguments, entry.output)
    }
    pub fn root_signature(&self, root: HirId) -> Result<(&[TypeKey], TypeKey)> {
        let entry = self
            .entries
            .get(&root)
            .ok_or("native root not in code plan")?;
        Ok((&entry.arguments, entry.output))
    }
    fn invoke(
        &self,
        context: &mut CallContext,
        values: &[Value],
        entry: Entry,
        arguments: &[TypeKey],
        output: TypeKey,
    ) -> Result<Value> {
        if let Ok(runtime) = context.runtime_mut() {
            runtime.bind_code_plan(self.identity, &self.demands)?;
        }
        if values.len() != arguments.len() {
            return Err("native argument count mismatch".into());
        }
        let mut args = Vec::new();
        for (value, &ty) in values.iter().zip(arguments) {
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
        let never = self.layouts.is_never(output)?;
        let words = if never {
            0
        } else {
            self.layouts.words(output)?
        };
        let mut result = vec![0; words].into_boxed_slice();
        // SAFETY: only our verified signature is transmuted; buffers have checked
        // widths, stay alive and do not move during execution. Generated code may
        // neither retain pointers nor unwind across this ABI.
        let status = unsafe {
            (entry)(
                context,
                args.as_ptr(),
                result.as_mut_ptr(),
                std::ptr::null(),
            )
        };
        match status {
            0 => {
                if never {
                    return Err("native Never function returned successfully".into());
                }
                let mut value = Value::from_result(result, output, words)?;
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
    compile_roots(mir, &[root])
}

/// Compile all roots into one executable owner and FunctionId namespace.
/// Root registration is deterministic regardless of request order.
pub fn compile_roots(mir: &SealedMir<'_>, roots: &[HirId]) -> Result<Compiled> {
    compile_plan(mir, roots, &[])
}

/// Register every value binding in the selected modules, including private
/// bindings unused by entry. Unsupported initializers are compile errors.
pub fn compile_modules(
    mir: &SealedMir<'_>,
    modules: &[telora_core::mir::ModuleId],
    roots: &[HirId],
) -> Result<Compiled> {
    use telora_core::ast::BindingKind;
    use telora_core::mir::SymbolKind;
    let graph = mir.mir();
    let mut globals = std::collections::BTreeSet::new();
    for module in modules {
        let scope = graph
            .module_scopes
            .get(module.index())
            .and_then(|s| *s)
            .ok_or("native module has no resolved scope")?;
        for binding in &graph.scopes[scope.index()].bindings {
            if matches!(
                graph.symbols[binding.symbol.index()].kind,
                SymbolKind::Declaration(
                    BindingKind::Let | BindingKind::Def | BindingKind::Decl | BindingKind::Native
                )
            ) {
                globals.insert(binding.symbol);
            }
        }
    }
    let globals = globals.into_iter().collect::<Vec<_>>();
    let fallback = globals
        .first()
        .and_then(|s| graph.symbols[s.index()].declarations.first())
        .copied();
    let roots = if roots.is_empty() {
        fallback.into_iter().collect::<Vec<_>>()
    } else {
        roots.to_vec()
    };
    compile_plan(mir, &roots, &globals)
}

fn compile_plan(mir: &SealedMir<'_>, roots: &[HirId], globals: &[SymbolId]) -> Result<Compiled> {
    let root = *roots
        .first()
        .ok_or("native code plan needs at least one root")?;
    let roots = roots
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let layouts = Layouts::from_mir(mir)?;
    let graph = mir.mir();
    let (_, _, output, arguments) = functions::shape(graph, &layouts, root.into())?;
    let mut builder =
        JITBuilder::new(cranelift_module::default_libcall_names()).map_err(|e| e.to_string())?;
    builder.symbol("telora_native_object", helpers::object as *const u8);
    let mut module = CodeMemory(Some(JITModule::new(builder)));
    let mut ctx = module.make_context();
    let pointer = module.target_config().pointer_type();
    ctx.func.signature.params = vec![AbiParam::new(pointer); 4];
    ctx.func.signature.returns.push(AbiParam::new(types::I32));
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
    for &root in &roots {
        functions::shape(graph, &layouts, root.into())?;
        let function = module
            .declare_function(
                &format!("telora_entry_{}", root.index()),
                Linkage::Export,
                &ctx.func.signature,
            )
            .map_err(|e| e.to_string())?;
        functions.registered.insert(root.into(), function);
        functions.pending.push(root.into());
    }
    for &symbol in globals {
        functions.global(graph, symbol, &mut module)?;
    }
    let mut next = 0;
    while next < functions.pending.len() {
        let node = functions.pending[next];
        next += 1;
        functions::emit(graph, &layouts, node, helper, &mut module, &mut functions)?;
    }
    functions::emit_dispatchers(graph, &layouts, root, helper, &mut module, &mut functions)?;
    module.finalize_definitions().map_err(|e| e.to_string())?;
    let mut entries = BTreeMap::new();
    for root in roots {
        let address = module.get_finalized_function(functions.registered[&root.into()]);
        // SAFETY: target default C ABI, exactly four pointers and a u32 status.
        let entry = unsafe { std::mem::transmute::<*const u8, Entry>(address) };
        let (_, _, output, arguments) = functions::shape(graph, &layouts, root.into())?;
        if functions
            .captures
            .get(&root.into())
            .is_some_and(|captures| !captures.is_empty())
        {
            return Err("capturing closure cannot be called as a bare native root".into());
        }
        entries.insert(
            root,
            CompiledEntry {
                entry,
                arguments,
                output,
            },
        );
    }
    let entry = entries[&root].entry;
    let mut initializers = functions
        .globals
        .values()
        .map(|&(slot, key, _)| {
            (
                slot,
                module
                    .get_finalized_function(functions.registered[&key])
                    .cast::<u64>(),
                Origin::from_loc(Some(graph.hir[key.node.index()].location)),
            )
        })
        .collect::<Vec<_>>();
    initializers.sort_by_key(|i| i.0);
    Ok(Compiled {
        identity: NEXT_CODE_PLAN
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |id| id.checked_add(1),
            )
            .map_err(|_| "native code plan identity overflow")?,
        _memory: module,
        entry,
        layouts,
        arguments,
        output,
        entries,
        initializers: initializers
            .into_iter()
            .map(|(_, address, origin)| (address, origin))
            .collect(),
        export_slots: {
            let mut exports = functions
                .globals
                .iter()
                .map(|(&symbol, &(slot, _, _))| (symbol, slot as usize))
                .collect::<BTreeMap<_, _>>();
            for &symbol in graph.exports.iter().flatten() {
                if let ResolveState::Bound(target) = graph.symbols[symbol.index()].resolution
                    && let Some(&(slot, _, _)) = functions.globals.get(&target)
                {
                    exports.insert(symbol, slot as usize);
                }
            }
            exports
        },
        demands: {
            let mut globals = functions
                .globals
                .iter()
                .map(|(&symbol, &(slot, _, ty))| {
                    (slot, crate::runtime::DemandKey::Export(symbol), ty)
                })
                .collect::<Vec<_>>();
            globals.sort_by_key(|g| g.0);
            globals.into_iter().map(|(_, key, ty)| (key, ty)).collect()
        },
    })
}

static NEXT_CODE_PLAN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

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
    fn return_value(&mut self, node: HirId, result: &[ir::Value]) -> EmitResult<()> {
        let result = self.fit_metadata(node, self.return_type, result.to_vec())?;
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
    fn fit_metadata(
        &mut self,
        node: HirId,
        expected: TypeKey,
        mut value: Vec<ir::Value>,
    ) -> EmitResult<Vec<ir::Value>> {
        let actual = TypeKey::try_from(self.ty(node)?)?;
        if actual == expected {
            return Ok(value);
        }
        if self.mir.types[actual.index()].constructor == TypeConstructor::TypeOf
            && self.mir.types[expected.index()].constructor == TypeConstructor::Type
        {
            // TypeOf(T) -> Type only erases the static witness. The represented
            // TypeId and source stay intact; no type computation occurs here.
            if value.len() != 3 {
                return Err("native metadata width mismatch".into());
            }
            let origin = self.builder.ins().band_imm_s(value[1], 0xffff_ffff);
            let stamp = self
                .builder
                .ins()
                .iconst(types::I64, (u64::from(expected.raw()) << 32) as i64);
            value[1] = self.builder.ins().bor(origin, stamp);
            return Ok(value);
        }
        Err("native value does not match the closed boundary type".into())
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
        let local = if let Some(slot) = self.mir.hir[callee_node.index()].resolution
            && let ResolveState::Bound(symbol) = self.mir.resolve_slots[slot.index()]
            && let Some(value) = self.locals.get(&symbol)
        {
            Some(value.clone())
        } else {
            None
        };
        let (function, closure, output, expected) =
            if local.is_some() || self.callable(callee_node, 0).is_err() {
                let ty = self.ty(callee_node)?;
                let signature = &self.mir.types[ty.index()];
                if signature.constructor != TypeConstructor::Function {
                    return Err("native indirect callee must have closed function type".into());
                }
                let (&output, parameters) = signature
                    .arguments
                    .split_last()
                    .ok_or("native function signature missing result")?;
                let output = TypeKey::try_from(output)?;
                let expected = parameters
                    .iter()
                    .copied()
                    .map(TypeKey::try_from)
                    .collect::<Result<Vec<_>>>()?;
                let closure = match local {
                    Some(value) => value,
                    None => self.expression(callee_node, depth + 1)?,
                };
                let function = self
                    .functions
                    .dispatcher(TypeKey::try_from(ty)?, self.module)?;
                (function, closure, output, expected)
            } else {
                let callee = functions::Key {
                    node: self.callable(callee_node, 0)?,
                    instance: self.instance_reference(callee_node),
                };
                let function = self.functions.declare(self.mir, callee, self.module)?;
                let closure = self.function_value(callee_node, callee)?;
                let (_, _, output, expected) = functions::shape(self.mir, self.layouts, callee)?;
                (function, closure, output, expected)
            };
        let environment = self.stack_words(&closure)?;
        let arguments = self.mir.hir[node.index()]
            .children
            .iter()
            .filter(|e| e.role == Role::Argument)
            .map(|e| e.node)
            .collect::<Vec<_>>();
        if expected.len() != arguments.len() {
            return Err("native direct argument count mismatch".into());
        }
        let result_ty = TypeKey::try_from(self.ty(node)?)?;
        let metadata_result = matches!(
            (
                &self.mir.types[output.index()].constructor,
                &self.mir.types[result_ty.index()].constructor
            ),
            (TypeConstructor::Type, TypeConstructor::TypeOf)
                | (TypeConstructor::TypeOf, TypeConstructor::Type)
        );
        if output != result_ty && !metadata_result {
            return Err("native direct result type mismatch".into());
        }
        let mut words = Vec::new();
        for (&arg, ty) in arguments.iter().zip(expected) {
            let value = self.expression(arg, depth + 1)?;
            words.extend(self.fit_metadata(arg, ty, value)?);
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
        let call = self
            .builder
            .ins()
            .call(callee, &[self.context, data, out, environment]);
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
        let mut result = (0..width)
            .map(|i| {
                self.builder
                    .ins()
                    .load(types::I64, MemFlagsData::new(), out, (i * 8) as i32)
            })
            .collect::<Vec<_>>();
        if output != result_ty {
            let origin = self.builder.ins().band_imm_s(result[1], 0xffff_ffff);
            let stamp = self
                .builder
                .ins()
                .iconst(types::I64, (u64::from(result_ty.raw()) << 32) as i64);
            result[1] = self.builder.ins().bor(origin, stamp);
        }
        Ok(result)
    }
    fn function_value(&mut self, origin: HirId, key: functions::Key) -> EmitResult<Vec<ir::Value>> {
        let function = self.functions.declare(self.mir, key, self.module)?;
        let mut captures = BTreeMap::new();
        let mut pending = vec![key.node];
        let mut owned = std::collections::BTreeSet::new();
        while let Some(node) = pending.pop() {
            owned.insert(node);
            pending.extend(self.mir.hir[node.index()].children.iter().map(|e| e.node));
        }
        pending.push(key.node);
        while let Some(node) = pending.pop() {
            let syntax = &self.mir.hir[node.index()];
            if let Some(slot) = syntax.resolution
                && let ResolveState::Bound(symbol) = self.mir.resolve_slots[slot.index()]
                && self.locals.contains_key(&symbol)
                && !self.mir.symbols[symbol.index()]
                    .declarations
                    .iter()
                    .any(|d| owned.contains(d))
            {
                captures.insert(symbol, TypeKey::try_from(self.ty(node)?)?);
            }
            pending.extend(syntax.children.iter().map(|e| e.node));
        }
        let captures = captures.into_iter().collect::<Vec<_>>();
        if let Some(previous) = self.functions.captures.get(&key) {
            if previous != &captures {
                return Err("native closure capture plan mismatch".into());
            }
        } else {
            self.functions.captures.insert(key, captures.clone());
        }
        let ty = TypeKey::try_from(key.ty(self.mir, key.node)?)?;
        if captures.is_empty() {
            let value = self.layouts.value(
                ty,
                Origin::from_loc(Some(self.mir.hir[origin.index()].location)),
                &[u64::from(function.as_u32())],
            )?;
            return Ok(value
                .words()
                .iter()
                .map(|&w| self.builder.ins().iconst(types::I64, w as i64))
                .collect());
        }
        let mut words = vec![
            self.builder
                .ins()
                .iconst(types::I64, i64::from(function.as_u32())),
        ];
        for (symbol, _) in captures {
            words.extend(self.locals[&symbol].iter().copied());
        }
        let count = self.builder.ins().iconst(types::I64, words.len() as i64);
        let data = self.stack_words(&words)?;
        self.object(origin, helpers::CLOSURE, ty, data, count)
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
            HirKind::TypeMetadata => {
                let metadata = &self.mir.types[ty.index()];
                if metadata.constructor != TypeConstructor::TypeOf || metadata.arguments.len() != 1
                {
                    return Err("native metadata has no closed represented type".into());
                }
                Some(vec![u64::from(
                    TypeKey::try_from(metadata.arguments[0])?.raw(),
                )])
            }
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
                let expression = child(self.mir, node, Role::Value)?;
                let value = self.expression(expression, depth + 1)?;
                self.return_value(expression, &value)?;
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
                if self.mir.types[self.ty(node)?.index()].constructor == TypeConstructor::Function
                    && let Ok(function) = self.callable(node, 0)
                {
                    return self.function_value(
                        node,
                        functions::Key {
                            node: function,
                            instance: self.instance_reference(node),
                        },
                    );
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
                let (slot, function, ty) = self.functions.global(self.mir, symbol, self.module)?;
                if ty != key {
                    return Err("native global reference type mismatch".into());
                }
                let function = self
                    .module
                    .declare_func_in_func(function, self.builder.func);
                let address = self
                    .builder
                    .ins()
                    .func_addr(self.module.target_config().pointer_type(), function);
                let slot = self.builder.ins().iconst(types::I64, i64::from(slot));
                self.object(node, helpers::DEMAND, ty, address, slot)
            }
            HirKind::TypeAscription => {
                self.expression(child(self.mir, node, Role::Value)?, depth + 1)
            }
            HirKind::TypeApply => {
                let function = self.callable(node, 0)?;
                self.function_value(
                    node,
                    functions::Key {
                        node: function,
                        instance: self.instance_reference(node),
                    },
                )
            }
            HirKind::Binding { .. } => {
                let value = self.expression(child(self.mir, node, Role::Value)?, depth + 1)?;
                let symbol = self.mir.hir_symbols[node.index()]
                    .ok_or("native binding pattern is not yet supported")?;
                self.locals.insert(symbol, value.clone());
                Ok(value)
            }
            HirKind::Closure => self.function_value(
                node,
                functions::Key {
                    node,
                    instance: self.function_key.instance,
                },
            ),
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
