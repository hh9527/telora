//! Decode trusted ordinary plan types; never recognize user field names alone.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::{TypeConstructor as T, TypeId};
use wasm_encoder::{Instruction as I, ValType};

#[derive(Clone)]
pub(crate) struct Stage {
    pub kind: String,
    pub callback: u32,
    pub output: TypeId,
}

pub(crate) enum Consumer {
    Both {
        left: Box<Self>,
        right: Box<Self>,
        output: TypeId,
    },
    Adapt {
        stage: Stage,
        next: Box<Self>,
    },
    Leaf {
        kind: String,
        input: TypeId,
        output: TypeId,
        callback: Option<(u32, TypeId)>,
        state: u32,
        done: u32,
        count: u32,
        capacity: u32,
    },
}

impl Emitter<'_> {
    pub(crate) fn sequence_name(&self, ty: TypeId, module: u32) -> Result<String, String> {
        let T::Nominal(id) = self.mir.types[ty.index()].constructor else {
            return Err("Wasm: sequence plan requires a trusted nominal type".into());
        };
        let symbol = &self.mir.symbols[id.index()];
        let admitted = symbol
            .module
            .and_then(|m| self.mir.modules[m.index()].native.as_ref());
        if admitted.map(|m| m.id) != Some(module) {
            return Err("Wasm: sequence plan is not declared by std::seq/std::sink".into());
        }
        Ok(symbol.name.clone())
    }

    pub(crate) fn sequence_field(
        &mut self,
        value: u32,
        ty: TypeId,
        name: &str,
    ) -> Result<(u32, TypeId), String> {
        let field = self.plan.layouts[ty.index()]
            .object
            .as_ref()
            .and_then(|o| o.members.iter().find(|m| m.name == name))
            .ok_or("Wasm: missing sequence plan field")?;
        let offset = field.offset.ok_or("Wasm: missing sequence field offset")? as i32;
        let field_ty =
            self.plan.layouts[field.type_id.ok_or("Wasm: missing sequence field type")?].id();
        let data = self.table_data(RECORDS, value, DATA);
        let pointer = self.local(ValType::I32);
        self.extend([
            I::LocalGet(data),
            I::I32Const(offset),
            I::I32Add,
            I::LocalSet(pointer),
        ]);
        Ok((pointer, field_ty))
    }

    pub(crate) fn sequence_stage(
        &mut self,
        value: u32,
        ty: TypeId,
        kind: String,
    ) -> Result<Stage, String> {
        let (callback, callback_ty) = self.sequence_field(value, ty, "callback")?;
        let output = *self.mir.types[callback_ty.index()]
            .arguments
            .last()
            .ok_or("Wasm: sequence callback has no result")?;
        Ok(Stage {
            kind,
            callback,
            output,
        })
    }

    pub(crate) fn sequence_source(
        &mut self,
        value: u32,
        ty: TypeId,
        stages: &mut Vec<Stage>,
        depth: usize,
    ) -> Result<(u32, TypeId), String> {
        if depth > 128 {
            return Err("Wasm: sequence plan nesting exceeds 128".into());
        }
        if self.sequence_name(ty, 36)? != "Seq" {
            return Err("Wasm: expected Seq plan".into());
        }
        let (plan, plan_ty) = self.sequence_field(value, ty, "plan")?;
        if self.mir.types[plan_ty.index()].constructor == T::Array {
            return Ok((plan, plan_ty));
        }
        let kind = self.sequence_name(plan_ty, 36)?;
        if !matches!(kind.as_str(), "Map" | "Filter" | "FilterMap") {
            return Err("Wasm: unsupported sequence stage".into());
        }
        let (source, source_ty) = self.sequence_field(plan, plan_ty, "source")?;
        let base = self.sequence_source(source, source_ty, stages, depth + 1)?;
        stages.push(self.sequence_stage(plan, plan_ty, kind)?);
        Ok(base)
    }

    pub(crate) fn sequence_consumer(
        &mut self,
        value: u32,
        ty: TypeId,
        depth: usize,
    ) -> Result<Consumer, String> {
        if depth > 128 {
            return Err("Wasm: consumer plan nesting exceeds 128".into());
        }
        if self.sequence_name(ty, 37)? != "Sink" {
            return Err("Wasm: expected Sink plan".into());
        }
        let args = self.mir.types[ty.index()].arguments.clone();
        let (input, output) = (args[0], args[1]);
        let (plan, plan_ty) = self.sequence_field(value, ty, "plan")?;
        let kind = self.sequence_name(plan_ty, 37)?;
        if kind == "Both" {
            let (left, left_ty) = self.sequence_field(plan, plan_ty, "left")?;
            let left = Box::new(self.sequence_consumer(left, left_ty, depth + 1)?);
            let (right, right_ty) = self.sequence_field(plan, plan_ty, "right")?;
            let right = Box::new(self.sequence_consumer(right, right_ty, depth + 1)?);
            return Ok(Consumer::Both {
                left,
                right,
                output,
            });
        }
        if matches!(kind.as_str(), "Map" | "Filter" | "FilterMap") {
            let stage = self.sequence_stage(plan, plan_ty, kind)?;
            let (next, next_ty) = self.sequence_field(plan, plan_ty, "downstream")?;
            return Ok(Consumer::Adapt {
                stage,
                next: Box::new(self.sequence_consumer(next, next_ty, depth + 1)?),
            });
        }
        let callback = if matches!(kind.as_str(), "Fold" | "Any" | "All" | "Find") {
            let (callback, ty) = self.sequence_field(plan, plan_ty, "callback")?;
            Some((
                callback,
                *self.mir.types[ty.index()].arguments.last().unwrap(),
            ))
        } else {
            None
        };
        let done = self.local(ValType::I32);
        let count = self.local(ValType::I32);
        let capacity = self.local(ValType::I32);
        let state = match kind.as_str() {
            "Collect" => {
                let data = self.local(ValType::I32);
                self.root_array_prefix(data, input, count)?;
                data
            }
            "Count" | "Any" | "All" => {
                self.scalar_as(self.key.node, output, i64::from(kind == "All"))?
            }
            "Find" => self.output_alloc(input)?,
            "Fold" => {
                let (initial, _) = self.sequence_field(plan, plan_ty, "initial")?;
                let state = self.local(ValType::I32);
                self.extend([I::LocalGet(initial), I::LocalSet(state)]);
                self.root_value(state, output);
                state
            }
            _ => return Err("Wasm: unsupported consumer plan".into()),
        };
        Ok(Consumer::Leaf {
            kind,
            input,
            output,
            callback,
            state,
            done,
            count,
            capacity,
        })
    }
}
