//! Elide only proven standard primitive Fmt nodes at a terminal text consumer.
use crate::{
    abi::*,
    emit::Emitter,
    plan::{child, symbol},
};
use telora_core::mir::{HirId, HirKind, MemberSelection, Role, TypeConstructor as T};
use wasm_encoder::{Instruction as I, ValType};

impl Emitter<'_> {
    pub(super) fn primitive_format_input(&self, node: HirId) -> Option<(u32, HirId, HirId)> {
        if std::env::var_os("TELORA_EXPERIMENTAL_TEXT_WRITER").is_none()
            || !matches!(self.mir.hir[node.index()].kind, HirKind::Call)
            || self.mir.value_adjustments[node.index()].is_some()
        {
            return None;
        }
        let args = self.call_arguments(node).ok()?;
        if args.len() != 1 && args.len() != 2 {
            return None;
        }
        let input = args[0];
        let (operation, native) =
            match self.mir.types[self.effective_ty(input).ok()?.index()].constructor {
                T::String => (1, "from_string"),
                T::Int => (3, "from_int"),
                T::Float => (4, "from_float"),
                _ => return None,
            };
        let callee = child(self.mir, node, Role::Callee).ok()?;
        let expected = |value| {
            let target = symbol(self.mir, value).ok()?;
            let s = &self.mir.symbols[target.index()];
            (matches!(
                s.kind,
                telora_core::mir::SymbolKind::Declaration(
                    telora_core::syntax::kinds::BindingKind::Native
                )
            ) && s.name == native
                && s.module
                    .is_some_and(|m| self.mir.modules[m.index()].name == "std/fmt"))
            .then_some(())
        };
        if args.len() == 1 && expected(callee).is_some() {
            return Some((operation, input, callee));
        }
        if args.len() != 2 || !self.standard_fmt_empty(args[1]) {
            return None;
        }
        let Some(MemberSelection::TraitMember {
            implementation,
            index: 0,
        }) = self.mir.member_selections[callee.index()]
        else {
            return None;
        };
        let instance = self
            .key
            .instance
            .and_then(|id| self.mir.generic_instances[id.index()].implementation(callee))
            .or(self.mir.implementation_instances[callee.index()]);
        let implementation = instance
            .map(|id| self.mir.generic_instances[id.index()].symbol)
            .or(implementation)?;
        let implementation = &self.mir.symbols[implementation.index()];
        if !implementation
            .module
            .is_some_and(|m| self.mir.modules[m.index()].name == "std/fmt")
        {
            return None;
        }
        let declaration = *implementation.declarations.last()?;
        let body = child(self.mir, declaration, Role::Value).ok()?;
        let body = if matches!(self.mir.hir[body.index()].kind, HirKind::Block) {
            if self.mir.hir[body.index()].children.iter().any(|edge| {
                edge.role == Role::Binding
                    && !matches!(
                        self.mir.hir[edge.node.index()].kind,
                        HirKind::Binding {
                            kind: telora_core::syntax::kinds::BindingKind::Type,
                            ..
                        }
                    )
            }) {
                return None;
            }
            child(self.mir, body, Role::Result).ok()?
        } else {
            body
        };
        if !matches!(self.mir.hir[body.index()].kind, HirKind::Dict) {
            return None;
        }
        let fields: Vec<_> = self.mir.hir[body.index()]
            .children
            .iter()
            .filter(|e| e.role == Role::Field)
            .collect();
        if fields.len() != 1 {
            return None;
        }
        let field = fields[0].node;
        let name = child(self.mir, field, Role::Name).ok()?;
        if !matches!(&self.mir.hir[name.index()].kind,HirKind::Name(name) if name=="fmt") {
            return None;
        }
        // Require the standard wrapper to forward its two parameters exactly.
        let wrapper = child(self.mir, field, Role::Value).ok()?;
        if !matches!(self.mir.hir[wrapper.index()].kind, HirKind::Closure) {
            return None;
        }
        let params: Vec<_> = self.mir.hir[wrapper.index()]
            .children
            .iter()
            .filter(|e| e.role == Role::Parameter)
            .map(|e| e.node)
            .collect();
        if params.len() != 2 {
            return None;
        }
        let body = child(self.mir, wrapper, Role::Body).ok()?;
        if self.mir.hir[body.index()]
            .children
            .iter()
            .any(|e| e.role == Role::Binding)
        {
            return None;
        }
        let call = child(self.mir, body, Role::Result).ok()?;
        if !matches!(self.mir.hir[call.index()].kind, HirKind::Call) {
            return None;
        }
        let native = match operation {
            1 => "write_str",
            3 => "write_int",
            4 => "write_float",
            _ => return None,
        };
        if !self.standard_fmt_native(child(self.mir, call, Role::Callee).ok()?, native) {
            return None;
        }
        let forwarded: Vec<_> = self.mir.hir[call.index()]
            .children
            .iter()
            .filter(|e| e.role == Role::Argument)
            .map(|e| e.node)
            .collect();
        if forwarded.len() != 2
            || symbol(self.mir, forwarded[0]).ok()? != self.mir.hir_symbols[params[1].index()]?
            || symbol(self.mir, forwarded[1]).ok()? != self.mir.hir_symbols[params[0].index()]?
        {
            return None;
        }
        Some((operation, input, callee))
    }
    fn standard_fmt_native(&self, node: HirId, name: &str) -> bool {
        symbol(self.mir, node).ok().is_some_and(|id| {
            let s = &self.mir.symbols[id.index()];
            s.name == name
                && matches!(
                    s.kind,
                    telora_core::mir::SymbolKind::Declaration(
                        telora_core::syntax::kinds::BindingKind::Native
                    )
                )
                && s.module
                    .is_some_and(|m| self.mir.modules[m.index()].name == "std/fmt")
        })
    }
    fn standard_fmt_empty(&self, node: HirId) -> bool {
        if !matches!(self.mir.hir[node.index()].kind, HirKind::Call) {
            return false;
        }
        self.call_arguments(node).is_ok_and(|a| a.is_empty())
            && child(self.mir, node, Role::Callee)
                .ok()
                .and_then(|c| symbol(self.mir, c).ok())
                .is_some_and(|id| {
                    let s = &self.mir.symbols[id.index()];
                    s.name == "empty"
                        && s.module
                            .is_some_and(|m| self.mir.modules[m.index()].name == "std/fmt")
                })
    }
    pub(super) fn primitive_format_value(
        &mut self,
        node: HirId,
        input: HirId,
        callee: HirId,
    ) -> Result<u32, String> {
        let function = self.expression(callee)?;
        let value = self.expression(input)?;
        let value = self.call_argument(node, 0, input, value)?;
        let args = self.call_arguments(node)?;
        if args.len() == 2 {
            self.expression(args[1])?;
        }
        self.extend([
            I::LocalGet(function),
            I::I32Load(memory(DATA, 2)),
            I::I32Eqz,
        ]);
        self.fail_if(node, ERROR_UNINITIALIZED_CALL);
        Ok(value)
    }
    // Only syntactic, single-consumer chains rooted in the shared empty value.
    // Arbitrary/custom Fmt values retain graph construction and depth semantics.
    fn format_write_chain(
        &self,
        node: HirId,
        steps: &mut Vec<(HirId, HirId, u32)>,
    ) -> Option<HirId> {
        if self.standard_fmt_empty(node) {
            return Some(node);
        }
        if !matches!(self.mir.hir[node.index()].kind, HirKind::Call)
            || self.mir.value_adjustments[node.index()].is_some()
        {
            return None;
        }
        let callee = child(self.mir, node, Role::Callee).ok()?;
        let operation = [
            ("write_str", 1),
            ("write_int", 3),
            ("write_float", 4),
            ("encode_json_string", 5),
        ]
        .into_iter()
        .find(|(name, _)| self.standard_fmt_native(callee, name))?
        .1;
        let args = self.call_arguments(node).ok()?;
        if args.len() != 2 {
            return None;
        }
        let empty = self.format_write_chain(args[0], steps)?;
        steps.push((node, callee, operation));
        Some(empty)
    }
    fn format_render_chain(
        &mut self,
        node: HirId,
        origin: HirId,
        outer: HirId,
        inner: HirId,
    ) -> Result<Option<u32>, String> {
        let mut steps = vec![];
        let Some(empty) = self.format_write_chain(inner, &mut steps) else {
            return Ok(None);
        };
        if steps.is_empty() {
            return Ok(None);
        }
        let outer = self.expression(outer)?;
        // Callees are evaluated before their left operand, exactly as for calls.
        let mut functions = vec![];
        for &(_, callee, _) in steps.iter().rev() {
            functions.push(self.expression(callee)?);
        }
        let empty = self.expression(empty)?;
        let parts = self.alloc(steps.len() as u32 * 8);
        let mut subject = empty;
        for (index, ((call, _, operation), function)) in
            steps.iter().zip(functions.into_iter().rev()).enumerate()
        {
            let input = self.call_arguments(*call)?[1];
            let value = self.expression(input)?;
            let value = self.call_argument(*call, 1, input, value)?;
            self.extend([
                I::LocalGet(function),
                I::I32Load(memory(DATA, 2)),
                I::I32Eqz,
            ]);
            self.fail_if(*call, ERROR_UNINITIALIZED_CALL);
            self.store32(parts, index as u64 * 8, *operation);
            self.extend([
                I::LocalGet(parts),
                I::LocalGet(value),
                I::I32Store(memory(index as u64 * 8 + 4, 2)),
            ]);
            subject = value;
        }
        self.extend([I::LocalGet(outer), I::I32Load(memory(DATA, 2)), I::I32Eqz]);
        self.fail_if(node, ERROR_UNINITIALIZED_CALL);
        let span = self.local(ValType::I32);
        self.extend([
            I::LocalGet(parts),
            I::I32Const(steps.len() as i32),
            I::Call(FORMAT_JOIN),
            I::LocalSet(span),
        ]);
        self.format_result(origin, self.effective_ty(node)?, span, subject)
            .map(Some)
    }
    pub(crate) fn format_render_call(&mut self, node: HirId) -> Result<Option<u32>, String> {
        if std::env::var_os("TELORA_EXPERIMENTAL_TEXT_WRITER").is_none() {
            return Ok(None);
        }
        let args = self.call_arguments(node)?;
        if args.len() != 1 {
            return Ok(None);
        }
        let callee = child(self.mir, node, Role::Callee)?;
        let Ok(target) = symbol(self.mir, callee) else {
            return Ok(None);
        };
        let s = &self.mir.symbols[target.index()];
        if s.name != "render"
            || !s
                .module
                .is_some_and(|m| self.mir.modules[m.index()].name == "std/fmt")
        {
            return Ok(None);
        }
        let origin = *s.declarations.last().ok_or("missing render declaration")?;
        let Some((operation, input, inner)) = self.primitive_format_input(args[0]) else {
            return self.format_render_chain(node, origin, callee, args[0]);
        };
        let outer = self.expression(callee)?;
        let value = self.primitive_format_value(args[0], input, inner)?;
        self.extend([I::LocalGet(outer), I::I32Load(memory(DATA, 2)), I::I32Eqz]);
        self.fail_if(node, ERROR_UNINITIALIZED_CALL);
        let parts = self.alloc(8);
        self.store32(parts, 0, operation);
        self.extend([
            I::LocalGet(parts),
            I::LocalGet(value),
            I::I32Store(memory(4, 2)),
        ]);
        let span = self.local(ValType::I32);
        self.extend([
            I::LocalGet(parts),
            I::I32Const(1),
            I::Call(FORMAT_JOIN),
            I::LocalSet(span),
        ]);
        self.format_result(origin, self.effective_ty(node)?, span, value)
            .map(Some)
    }
}
