//! Experimental scalar replacement of non-escaping literal pair arrays.
//! Summarize only closed unary wrappers of Enum(dict::from_pairs(parameter)).
//! No language-visible mutation and no ownership guess for shared storage.
use crate::{
    abi::*,
    emit::Emitter,
    plan::{Key, Special, child, symbol},
};
use std::collections::BTreeMap;
use telora_core::mir::{HirId, HirKind, MemberSelection, Role, TypeConstructor as T};
use wasm_encoder::Instruction as I;

impl Emitter<'_> {
    pub(crate) fn literal_pairs_call(&mut self, node: HirId) -> Result<Option<u32>, String> {
        if std::env::var_os("TELORA_EXPERIMENTAL_LITERAL_PAIRS").is_none() {
            return Ok(None);
        }
        let args = self.call_arguments(node)?;
        if args.len() != 1 {
            return Ok(None);
        }
        let mut array = args[0];
        while matches!(self.mir.hir[array.index()].kind, HirKind::TypeAscription) {
            if self.mir.value_adjustments[array.index()].is_some() {
                return Ok(None);
            }
            array = child(self.mir, array, Role::Value)?;
        }
        if !matches!(self.mir.hir[array.index()].kind, HirKind::Array) {
            return Ok(None);
        }
        let callee = child(self.mir, node, Role::Callee)?;
        let Ok(target) = symbol(self.mir, callee) else {
            return Ok(None);
        };
        let sym = &self.mir.symbols[target.index()];
        let direct = sym
            .module
            .is_some_and(|m| self.mir.modules[m.index()].name == "std/dict")
            && sym.name == "from_pairs";
        let wrapper = if direct {
            None
        } else {
            if !self.mir.symbol_generics[target.index()].is_empty() {
                return Ok(None);
            }
            let Some(binding) = sym.declarations.first().copied() else {
                return Ok(None);
            };
            let Ok(function) = child(self.mir, binding, Role::Value) else {
                return Ok(None);
            };
            if !matches!(self.mir.hir[function.index()].kind, HirKind::Closure) {
                return Ok(None);
            }
            let key = Key {
                node: function,
                instance: None,
                callable: true,
                special: Special::Normal,
            };
            if self.plan.captures.get(&key).is_none_or(|c| !c.is_empty()) {
                return Ok(None);
            }
            let params: Vec<_> = self.mir.hir[function.index()]
                .children
                .iter()
                .filter(|e| e.role == Role::Parameter)
                .map(|e| e.node)
                .collect();
            if params.len() != 1 {
                return Ok(None);
            }
            let Some(parameter) = self.mir.hir_symbols[params[0].index()] else {
                return Ok(None);
            };
            let mut body = child(self.mir, function, Role::Body)?;
            while matches!(self.mir.hir[body.index()].kind, HirKind::Block) {
                if self.mir.hir[body.index()]
                    .children
                    .iter()
                    .any(|e| e.role == Role::Binding)
                {
                    return Ok(None);
                }
                body = child(self.mir, body, Role::Result)?;
            }
            if !matches!(self.mir.hir[body.index()].kind, HirKind::Call) {
                return Ok(None);
            }
            let constructor = child(self.mir, body, Role::Callee)?;
            let Some(MemberSelection::EnumVariant { index }) =
                crate::enums::selection(self.mir, constructor)
            else {
                return Ok(None);
            };
            let calls: Vec<_> = self.mir.hir[body.index()]
                .children
                .iter()
                .filter(|e| e.role == Role::Argument)
                .map(|e| e.node)
                .collect();
            if calls.len() != 1 || !matches!(self.mir.hir[calls[0].index()].kind, HirKind::Call) {
                return Ok(None);
            }
            let call = calls[0];
            let from_pairs = child(self.mir, call, Role::Callee)?;
            let Ok(native) = symbol(self.mir, from_pairs) else {
                return Ok(None);
            };
            let native = &self.mir.symbols[native.index()];
            if native.name != "from_pairs"
                || !native
                    .module
                    .is_some_and(|m| self.mir.modules[m.index()].name == "std/dict")
            {
                return Ok(None);
            }
            let inputs: Vec<_> = self.mir.hir[call.index()]
                .children
                .iter()
                .filter(|e| e.role == Role::Argument)
                .map(|e| e.node)
                .collect();
            if inputs.len() != 1 || symbol(self.mir, inputs[0]).ok() != Some(parameter) {
                return Ok(None);
            }
            Some((key, constructor, index, call, from_pairs))
        };
        let output_ty = self.effective_ty(node)?;
        let dict_ty = if let Some((key, _, _, call, _)) = wrapper {
            key.effective_ty(self.mir, call)?
        } else {
            output_ty
        };
        if self.mir.types[dict_ty.index()].constructor != T::Dict {
            return Ok(None);
        }
        let element = self.mir.types[dict_ty.index()].arguments[0];
        let mut entries = Vec::new();
        let mut keys = std::collections::BTreeSet::new();
        for edge in &self.mir.hir[array.index()].children {
            if edge.role != Role::Item {
                continue;
            }
            let pair = edge.node;
            if !matches!(self.mir.hir[pair.index()].kind, HirKind::Tuple)
                || self.mir.value_adjustments[pair.index()].is_some()
            {
                return Ok(None);
            }
            let items: Vec<_> = self.mir.hir[pair.index()]
                .children
                .iter()
                .filter(|e| e.role == Role::Item)
                .map(|e| e.node)
                .collect();
            if items.len() != 2 {
                return Ok(None);
            }
            let HirKind::String(name) = &self.mir.hir[items[0].index()].kind else {
                return Ok(None);
            };
            if !keys.insert(name.clone()) {
                return Ok(None);
            }
            entries.push((name.clone(), items[0], items[1]));
        }
        if std::env::var_os("TELORA_EXPERIMENTAL_LITERAL_PAIRS_REPORT").is_some() {
            eprintln!(
                "{}",
                serde_json::json!({"record":"collection-lowering", "operation":"literal_pairs", "node":node.index(),
                "module":self.mir.modules[self.mir.hir[node.index()].module.index()].name,
                "keys":entries.len(), "cross_function":wrapper.is_some(),
                "proof":"closed single-consumer wrapper; unique static keys; fresh columns; no shared writes"})
            );
        }
        // Evaluate callee and values exactly once, in source order. All produced
        // values remain normal GC roots; only unobservable container heads vanish.
        let callable = self.expression(callee)?;
        let mut fields = BTreeMap::new();
        for (name, key_node, value_node) in entries {
            let value = self.expression(value_node)?;
            let value = self.adapt(value_node, self.effective_ty(value_node)?, element, value)?;
            fields.insert(name, (key_node, value));
        }
        self.extend([
            I::LocalGet(callable),
            I::I32Load(memory(DATA, 2)),
            I::I32Eqz,
        ]);
        self.fail_if(node, ERROR_UNINITIALIZED_CALL);
        if let Some((key, constructor, index, call, native_node)) = wrapper {
            let saved = self.key;
            self.key = key;
            let result = (|| {
                let ctor = self.expression(constructor)?;
                let native = self.expression(native_node)?;
                self.extend([I::LocalGet(native), I::I32Load(memory(DATA, 2)), I::I32Eqz]);
                self.fail_if(call, ERROR_UNINITIALIZED_CALL);
                let dict = self.literal_dict_part(call, dict_ty, fields)?;
                let result = self.enum_value(constructor, output_ty, index, Some(dict))?;
                self.copy(result, 0, ctor, LOC_BYTES);
                self.root_value(result, output_ty);
                Ok(result)
            })();
            self.key = saved;
            result.map(Some)
        } else {
            self.literal_dict_part(node, dict_ty, fields).map(Some)
        }
    }
}
