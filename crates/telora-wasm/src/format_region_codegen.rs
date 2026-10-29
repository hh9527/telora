//! Internal writer calls use an extra ABI footer, never a language-visible value.
use crate::{abi::*, emit::Emitter, plan::child};
use telora_core::mir::{HirId, Role, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};
impl Emitter<'_> {
    pub(crate) fn region_writer(&mut self) -> u32 {
        if let Some(w) = self.format_writer {
            return w;
        }
        let arity = self.mir.types[self.ty(self.key.node).unwrap().index()]
            .arguments
            .len()
            - 1;
        let w = self.local(ValType::I32);
        self.extend([
            I::LocalGet(1),
            I::I32Load(memory((arity as u64 + 1) * 4, 2)),
            I::LocalSet(w),
        ]);
        self.format_writer = Some(w);
        w
    }
    pub(crate) fn writer_invoke(
        &mut self,
        callee: u32,
        values: &[u32],
        signature: TypeId,
        origin: Option<u32>,
        writer: u32,
        checked: bool,
    ) -> Result<u32, String> {
        let selected = self.local(ValType::I32);
        let pointer = self.read32(callee, DATA);
        let targets: Vec<_> = self
            .plan
            .format_regions
            .functions
            .iter()
            .filter(|(k, _)| k.ty(self.mir, k.node) == Ok(signature))
            .map(|(&k, &i)| (k, i))
            .collect();
        for (k, index) in targets {
            self.emit(I::LocalGet(pointer));
            self.function_pointer(self.plan.functions[&k]);
            self.extend([I::I32Eq, I::If(BlockType::Empty)]);
            self.function_pointer(index);
            self.emit(I::LocalSet(selected));
            self.emit(I::End);
        }
        self.extend([
            I::LocalGet(selected),
            I::I32Eqz,
            I::If(BlockType::Empty),
            I::Unreachable,
            I::End,
        ]);
        let function = self.alloc(FUNCTION_BYTES);
        self.copy(function, 0, callee, FUNCTION_BYTES);
        self.extend([
            I::LocalGet(function),
            I::LocalGet(selected),
            I::I32Store(memory(DATA, 2)),
        ]);
        let zero = self.local(ValType::I32);
        let mut footer = values.to_vec();
        footer.push(origin.unwrap_or(zero));
        footer.push(writer);
        let args = self.argument_array(&footer, None);
        let output = *self.mir.types[signature.index()].arguments.last().unwrap();
        let out = self.output_alloc(output)?;
        let result = self.local(ValType::I32);
        self.extend([
            I::LocalGet(function),
            I::LocalGet(args),
            I::LocalGet(out),
            I::Call(INVOKE),
            I::LocalSet(result),
        ]);
        if checked {
            self.checked(result);
        }
        self.root_value(result, output);
        Ok(result)
    }
    pub(crate) fn format_region_render(&mut self, node: HirId) -> Result<Option<u32>, String> {
        if self.writer_mode {
            return Ok(None);
        }
        let args = self.call_arguments(node)?;
        if args.len() != 1 {
            return Ok(None);
        }
        let outer = child(self.mir, node, Role::Callee)?;
        let Ok(s) = crate::plan::symbol(self.mir, outer) else {
            return Ok(None);
        };
        let sym = &self.mir.symbols[s.index()];
        if sym.name != "render"
            || !sym
                .module
                .is_some_and(|m| self.mir.modules[m.index()].name == "std/fmt")
        {
            return Ok(None);
        }
        let inner = args[0];
        if !matches!(
            self.mir.hir[inner.index()].kind,
            telora_core::mir::HirKind::Call
        ) {
            return Ok(None);
        }
        let target = child(self.mir, inner, Role::Callee)?;
        let signature = self.ty(target)?;
        if !self.plan.format_regions.signatures.contains(&signature) {
            return Ok(None);
        }
        let inputs = self.call_arguments(inner)?;
        let types = &self.mir.types[signature.index()].arguments;
        for (i, &v) in inputs.iter().enumerate() {
            if crate::format_regions::is_fmt(self.mir, types[i]) && !self.standard_fmt_empty(v) {
                return Ok(None);
            }
        }
        let origin = *sym
            .declarations
            .last()
            .ok_or("missing render declaration")?;
        let outer_value = self.expression(outer)?;
        let function = self.expression(target)?;
        let mut values = vec![];
        for (i, &v) in inputs.iter().enumerate() {
            let value = self.expression(v)?;
            values.push(self.call_argument(inner, i, v, value)?);
        }
        self.extend([
            I::LocalGet(function),
            I::I32Load(memory(DATA, 2)),
            I::I32Eqz,
        ]);
        self.fail_if(inner, ERROR_UNINITIALIZED_CALL);
        self.format_region_apply(
            inner,
            origin,
            signature,
            function,
            &values,
            Some((outer_value, node)),
            self.effective_ty(node)?,
        )
        .map(Some)
    }
    pub(crate) fn format_region_value(
        &mut self,
        node: HirId,
        string: TypeId,
    ) -> Result<Option<u32>, String> {
        if self.writer_mode
            || !matches!(
                self.mir.hir[node.index()].kind,
                telora_core::mir::HirKind::Call
            )
        {
            return Ok(None);
        }
        let target = child(self.mir, node, Role::Callee)?;
        let signature = self.ty(target)?;
        if !self.plan.format_regions.signatures.contains(&signature) {
            return Ok(None);
        }
        let inputs = self.call_arguments(node)?;
        let types = &self.mir.types[signature.index()].arguments;
        for (i, &v) in inputs.iter().enumerate() {
            if crate::format_regions::is_fmt(self.mir, types[i]) && !self.standard_fmt_empty(v) {
                return Ok(None);
            }
        }
        let function = self.expression(target)?;
        let mut values = vec![];
        for (i, &v) in inputs.iter().enumerate() {
            let value = self.expression(v)?;
            values.push(self.call_argument(node, i, v, value)?);
        }
        self.extend([
            I::LocalGet(function),
            I::I32Load(memory(DATA, 2)),
            I::I32Eqz,
        ]);
        self.fail_if(node, ERROR_UNINITIALIZED_CALL);
        self.format_region_apply(node, node, signature, function, &values, None, string)
            .map(Some)
    }
    pub(crate) fn format_region_apply(
        &mut self,
        node: HirId,
        origin: HirId,
        signature: TypeId,
        function: u32,
        values: &[u32],
        outer: Option<(u32, HirId)>,
        string: TypeId,
    ) -> Result<u32, String> {
        let writer = self.local(ValType::I32);
        self.extend([
            I::I32Const(0),
            I::I32Const(-1),
            I::I32Const(0),
            I::Call(JSON_WRITE),
            I::LocalSet(writer),
        ]);
        let call_origin = self.computation_origin(node)?;
        let result = self.writer_invoke(
            function,
            values,
            signature,
            Some(call_origin),
            writer,
            false,
        )?;
        self.extend([
            I::LocalGet(result),
            I::I32Eqz,
            I::If(BlockType::Empty),
            I::I32Const(11),
            I::LocalGet(writer),
            I::I32Const(0),
            I::Call(JSON_WRITE),
            I::Drop,
            I::End,
        ]);
        self.checked(result);
        if let Some((outer, node)) = outer {
            self.extend([
                I::LocalGet(outer),
                I::I32Load(memory(DATA, 2)),
                I::I32Eqz,
                I::If(BlockType::Empty),
                I::I32Const(11),
                I::LocalGet(writer),
                I::I32Const(0),
                I::Call(JSON_WRITE),
                I::Drop,
                I::End,
            ]);
            self.extend([I::LocalGet(outer), I::I32Load(memory(DATA, 2)), I::I32Eqz]);
            self.fail_if(node, ERROR_UNINITIALIZED_CALL);
        }
        let span = self.local(ValType::I32);
        self.extend([
            I::I32Const(1),
            I::LocalGet(writer),
            I::I32Const(0),
            I::Call(JSON_WRITE),
            I::LocalSet(span),
        ]);
        self.format_result(origin, string, span, result)
    }
}
