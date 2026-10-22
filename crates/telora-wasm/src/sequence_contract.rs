//! Validate phantom element/result parameters before lowering ordinary plan values.
use crate::emit::Emitter;
use telora_core::mir::{TypeConstructor as T, TypeId};

impl Emitter<'_> {
    fn seq_contract_field(&self, ty: TypeId, name: &str) -> Result<TypeId, String> {
        self.plan.layouts[ty.index()]
            .object
            .as_ref()
            .and_then(|o| o.members.iter().find(|m| m.name == name))
            .and_then(|m| m.type_id)
            .map(|id| self.plan.layouts[id].id())
            .ok_or_else(|| "Wasm: invalid sequence plan field contract".into())
    }
    fn seq_contract_callback(&self, ty: TypeId, inputs: &[TypeId]) -> Result<TypeId, String> {
        let ty = &self.mir.types[ty.index()];
        if ty.constructor != T::Function
            || ty.arguments.len() != inputs.len() + 1
            || &ty.arguments[..inputs.len()] != inputs
        {
            return Err("Wasm: invalid sequence callback contract".into());
        }
        Ok(*ty.arguments.last().unwrap())
    }
    fn seq_contract_equal(&self, actual: TypeId, expected: TypeId) -> Result<(), String> {
        if actual == expected {
            Ok(())
        } else {
            Err("Wasm: sequence plan element/result type mismatch".into())
        }
    }
    fn seq_contract_kind(&self, ty: TypeId, expected: T) -> Result<(), String> {
        if self.mir.types[ty.index()].constructor == expected {
            Ok(())
        } else {
            Err("Wasm: invalid sequence result type".into())
        }
    }
    pub(crate) fn sequence_source_contract(
        &self,
        ty: TypeId,
        depth: usize,
    ) -> Result<TypeId, String> {
        if depth > 128 {
            return Err("Wasm: sequence plan nesting exceeds 128".into());
        }
        if self.sequence_name(ty, 36)? != "Seq" {
            return Err("Wasm: expected Seq plan".into());
        }
        let element = self.mir.types[ty.index()].arguments[0];
        let plan = self.seq_contract_field(ty, "plan")?;
        if self.mir.types[plan.index()].constructor == T::Array {
            self.seq_contract_equal(element, self.mir.types[plan.index()].arguments[0])?;
        } else {
            let source = self.seq_contract_field(plan, "source")?;
            let input = self.sequence_source_contract(source, depth + 1)?;
            let callback = self.seq_contract_field(plan, "callback")?;
            let output = self.seq_contract_callback(callback, &[input])?;
            match self.sequence_name(plan, 36)?.as_str() {
                "Map" => self.seq_contract_equal(element, output)?,
                "Filter" => {
                    self.seq_contract_kind(output, T::Bool)?;
                    self.seq_contract_equal(element, input)?;
                }
                "FilterMap" => {
                    self.seq_contract_kind(output, T::Option)?;
                    self.seq_contract_equal(element, self.mir.types[output.index()].arguments[0])?;
                }
                _ => return Err("Wasm: unsupported sequence stage".into()),
            }
        }
        Ok(element)
    }
    pub(crate) fn sequence_sink_contract(
        &self,
        ty: TypeId,
        input: TypeId,
        depth: usize,
    ) -> Result<TypeId, String> {
        if depth > 128 {
            return Err("Wasm: consumer plan nesting exceeds 128".into());
        }
        if self.sequence_name(ty, 37)? != "Sink" {
            return Err("Wasm: expected Sink plan".into());
        }
        let args = &self.mir.types[ty.index()].arguments;
        self.seq_contract_equal(input, args[0])?;
        let output = args[1];
        let plan = self.seq_contract_field(ty, "plan")?;
        match self.sequence_name(plan, 37)?.as_str() {
            "Both" => {
                let left = self.sequence_sink_contract(
                    self.seq_contract_field(plan, "left")?,
                    input,
                    depth + 1,
                )?;
                let right = self.sequence_sink_contract(
                    self.seq_contract_field(plan, "right")?,
                    input,
                    depth + 1,
                )?;
                self.seq_contract_kind(output, T::Tuple)?;
                if self.mir.types[output.index()].arguments != [left, right] {
                    return Err("Wasm: invalid both result type".into());
                }
            }
            kind @ ("Map" | "Filter" | "FilterMap") => {
                let callback = self.seq_contract_field(plan, "callback")?;
                let mapped = self.seq_contract_callback(callback, &[input])?;
                let next_input = match kind {
                    "Map" => mapped,
                    "Filter" => {
                        self.seq_contract_kind(mapped, T::Bool)?;
                        input
                    }
                    _ => {
                        self.seq_contract_kind(mapped, T::Option)?;
                        self.mir.types[mapped.index()].arguments[0]
                    }
                };
                let result = self.sequence_sink_contract(
                    self.seq_contract_field(plan, "downstream")?,
                    next_input,
                    depth + 1,
                )?;
                self.seq_contract_equal(result, output)?;
            }
            "Collect" => {
                self.seq_contract_kind(output, T::Array)?;
                self.seq_contract_equal(self.mir.types[output.index()].arguments[0], input)?;
            }
            "Count" => self.seq_contract_kind(output, T::Int)?,
            "Fold" => {
                self.seq_contract_equal(self.seq_contract_field(plan, "initial")?, output)?;
                let result = self.seq_contract_callback(
                    self.seq_contract_field(plan, "callback")?,
                    &[output, input],
                )?;
                self.seq_contract_equal(result, output)?;
            }
            kind @ ("Any" | "All" | "Find") => {
                let result = self
                    .seq_contract_callback(self.seq_contract_field(plan, "callback")?, &[input])?;
                self.seq_contract_kind(result, T::Bool)?;
                if kind == "Find" {
                    self.seq_contract_kind(output, T::Option)?;
                    self.seq_contract_equal(self.mir.types[output.index()].arguments[0], input)?;
                } else {
                    self.seq_contract_kind(output, T::Bool)?;
                }
            }
            _ => return Err("Wasm: unsupported consumer plan".into()),
        }
        Ok(output)
    }
}
