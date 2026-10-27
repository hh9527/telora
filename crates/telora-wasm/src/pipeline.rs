//! Private collection backends for immutable pipeline operators.
#[path = "pipeline_array.rs"]
mod array;
#[path = "pipeline_dict.rs"]
mod dict;
#[path = "pipeline_memo.rs"]
mod memo;
use crate::emit::Emitter;
use telora_core::mir::{TypeConstructor as T, TypeId};

impl Emitter<'_> {
    fn builder_name(&self, ty: TypeId) -> Result<String, String> {
        self.sequence_name(ty, 39)
            .map_err(|_| "Wasm: MutBuilder requires trusted std::pipeline nominal types".into())
    }
    fn builder_field_type(&self, ty: TypeId, name: &str) -> Result<TypeId, String> {
        self.plan.layouts[ty.index()]
            .object
            .as_ref()
            .and_then(|o| o.members.iter().find(|m| m.name == name))
            .and_then(|m| m.type_id)
            .map(|id| self.plan.layouts[id].id())
            .ok_or_else(|| "Wasm: invalid MutBuilder field contract".into())
    }
    fn builder_contract(
        &self,
        builder: TypeId,
    ) -> Result<(TypeId, TypeId, TypeId, TypeId, String), String> {
        if self.builder_name(builder)? != "MutBuilder" {
            return Err("Wasm: expected private MutBuilder".into());
        }
        let params = &self.mir.types[builder.index()].arguments;
        if params.len() != 3 {
            return Err("Wasm: invalid MutBuilder arity".into());
        }
        let (item, result, state) = (params[0], params[1], params[2]);
        let storage = self.builder_field_type(builder, "states")?;
        if self.mir.types[storage.index()].constructor != T::Array
            || self.mir.types[storage.index()].arguments != [state]
        {
            return Err("Wasm: invalid MutBuilder storage".into());
        }
        let kind = self.builder_name(state)?;
        let parameters = &self.mir.types[state.index()].arguments;
        if parameters.len() != 1 {
            return Err("Wasm: invalid concrete MutBuilder arity".into());
        }
        let value = parameters[0];
        let valid = match kind.as_str() {
            "DictMut" => {
                self.mir.types[result.index()].constructor == T::Dict
                    && self.mir.types[result.index()].arguments == [value]
                    && self.builder_name(item)? == "Edit"
                    && self.mir.types[item.index()].arguments == [value]
            }
            "ArrayMut" => {
                item == value
                    && self.mir.types[result.index()].constructor == T::Array
                    && self.mir.types[result.index()].arguments == [value]
            }
            _ => false,
        };
        if !valid {
            return Err("Wasm: MutBuilder item/result/implementation mismatch".into());
        }
        Ok((item, result, state, storage, kind))
    }
    pub(crate) fn pipeline_native(&mut self, name: &str) -> Result<u32, String> {
        if matches!(name, "new_run" | "new_slots" | "memo") {
            return self.pipeline_memo_native(name);
        }
        let args = self.mir.types[self.ty(self.key.node)?.index()]
            .arguments
            .clone();
        let output = *args.last().ok_or("Wasm: missing MutBuilder result")?;
        let creating = name == "create_mut_builder";
        let builder_ty = if creating { output } else { args[0] };
        let (item, result, state, storage, kind) = self.builder_contract(builder_ty)?;
        if creating {
            let plan_ty = args[0];
            if self.builder_name(plan_ty)? != "Plan"
                || self.mir.types[plan_ty.index()].arguments != [item, result, state]
                || self.builder_field_type(plan_ty, "initial")? != result
            {
                return Err("Wasm: invalid immutable builder plan contract".into());
            }
            let plan = self.parameter(0);
            let (initial, _) = self.sequence_field(plan, plan_ty, "initial")?;
            let states = if kind == "DictMut" {
                let layout = self.pipeline_layout(storage)?;
                self.pipeline_new(&[result, storage], &layout, initial)?
            } else {
                self.pipeline_array_new(initial, result, state, storage)?
            };
            self.root_value(states, storage);
            return self.packed_tuple(builder_ty, &[states]);
        }
        let builder = self.parameter(0);
        let (states, _) = self.sequence_field(builder, builder_ty, "states")?;
        let native = match name {
            "extend"
                if args.len() == 3
                    && args[1] == item
                    && self.mir.types[output.index()].constructor == T::Bool =>
            {
                "dict_accept"
            }
            "finish" if args.len() == 2 && output == result => "dict_finish",
            _ => return Err("Wasm: invalid restricted MutBuilder operation".into()),
        };
        if kind == "DictMut" {
            self.pipeline_dict_native(
                native,
                &[
                    storage,
                    if name == "extend" { item } else { result },
                    output,
                ],
                states,
            )
        } else {
            self.pipeline_array_native(name, states, state, result, output)
        }
    }
}
