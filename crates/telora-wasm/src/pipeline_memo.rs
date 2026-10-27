//! Per-node, single-entry memoization. All retained references are ordinary
//! typed values, so GC and snapshot relocation preserve token identity.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::TypeConstructor as T;
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(super) fn pipeline_memo_native(&mut self, name: &str) -> Result<u32, String> {
        let args = self.mir.types[self.ty(self.key.node)?.index()]
            .arguments
            .clone();
        let output = *args.last().ok_or("Wasm: missing pipeline memo result")?;
        if name == "new_run" {
            if args.len() != 1 || self.builder_name(output)? != "Run" {
                return Err("Wasm: invalid pipeline run signature".into());
            }
            let token_ty = self.builder_field_type(output, "token")?;
            let int = self.mir.types[token_ty.index()].arguments[0];
            if self.mir.types[token_ty.index()].constructor != T::Array
                || self.mir.types[int.index()].constructor != T::Int
            {
                return Err("Wasm: invalid pipeline token layout".into());
            }
            let one = self.local(ValType::I32);
            self.extend([I::I32Const(1), I::LocalSet(one)]);
            let data = self.array_storage(one, self.width(int)?);
            let zero = self.scalar_as(self.key.node, int, 0)?;
            self.copy(data, 0, zero, self.width(int)?);
            let token = self.array_result(token_ty, data, one, self.width(int)?)?;
            self.root_value(token, token_ty);
            return self.packed_tuple(output, &[token]);
        }
        let storage = if name == "new_slots" && args.len() == 1 {
            output
        } else if name == "memo" && args.len() == 4 {
            args[0]
        } else {
            return Err("Wasm: invalid pipeline memo operation".into());
        };
        if self.mir.types[storage.index()].constructor != T::Array {
            return Err("Wasm: pipeline memo requires typed slots".into());
        }
        let slot = self.mir.types[storage.index()].arguments[0];
        if self.builder_name(slot)? != "Slot" {
            return Err("Wasm: invalid pipeline memo slot".into());
        }
        let variants = &self.plan.layouts[slot.index()].variants;
        if variants.len() != 3
            || variants[0].name != "Empty"
            || variants[1].name != "Pending"
            || variants[2].name != "Ready"
        {
            return Err("Wasm: invalid pipeline memo variants".into());
        }
        let ready_ty =
            self.plan.layouts[variants[2].type_id.ok_or("Wasm: missing memo entry")?].id();
        let run_ty = self.builder_field_type(ready_ty, "run")?;
        let result_ty = self.builder_field_type(ready_ty, "value")?;
        if self.builder_name(run_ty)? != "Run" {
            return Err("Wasm: invalid pipeline memo run".into());
        }
        let width = self.width(slot)?;
        if name == "new_slots" {
            let one = self.local(ValType::I32);
            self.extend([I::I32Const(1), I::LocalSet(one)]);
            let data = self.array_storage(one, width);
            self.extend([
                I::LocalGet(data),
                I::I32Const(0),
                I::I32Const(width as i32),
                I::MemoryFill(0),
            ]);
            self.root_array_prefix(data, slot, one)?;
            return self.array_result(storage, data, one, width);
        }
        if args[1] != run_ty
            || output != result_ty
            || self.mir.types[args[2].index()].constructor != T::Function
            || self.mir.types[args[2].index()].arguments != [output]
        {
            return Err("Wasm: invalid pipeline memo signature".into());
        }
        let slots = self.parameter(0);
        let run = self.parameter(1);
        let calc = self.parameter(2);
        let (cell, count) = self.array_parts(slots, width);
        self.extend([I::LocalGet(count), I::I32Const(1), I::I32Ne]);
        self.fail_if(self.key.node, ERROR_INDEX);
        let status = self.read32(cell, DATA);
        self.extend([
            I::LocalGet(status),
            I::I32Const(2),
            I::I32Eq,
            I::If(BlockType::Empty),
        ]);
        let ready = self.enum_payload(slot, 2, cell)?;
        let (previous, _) = self.sequence_field(ready, ready_ty, "run")?;
        let (old_token, _) = self.sequence_field(previous, run_ty, "token")?;
        let (new_token, _) = self.sequence_field(run, run_ty, "token")?;
        let old_id = self.read32(old_token, DATA);
        let new_id = self.read32(new_token, DATA);
        self.extend([
            I::LocalGet(old_id),
            I::LocalGet(new_id),
            I::I32Eq,
            I::If(BlockType::Empty),
        ]);
        let (value, _) = self.sequence_field(ready, ready_ty, "value")?;
        self.extend([I::LocalGet(value), I::Return, I::End, I::End]);
        self.extend([I::LocalGet(status), I::I32Const(1), I::I32Eq]);
        self.emit(I::If(BlockType::Empty));
        self.reflection_failure(calc, "pipeline node computation depends on itself")?;
        self.emit(I::End);
        self.store64(cell, DATA, 1);
        let origin = self.computation_origin(self.key.node)?;
        let arguments = self.argument_array(&[], Some(origin));
        let buffer = self.output_alloc(output)?;
        let result = self.local(ValType::I32);
        self.extend([
            I::LocalGet(calc),
            I::LocalGet(arguments),
            I::LocalGet(buffer),
            I::Call(INVOKE),
            I::LocalSet(result),
            I::LocalGet(result),
            I::I32Eqz,
            I::If(BlockType::Empty),
        ]);
        // A recoverable language failure never leaves a Pending or Ready value.
        self.store64(cell, DATA, 0);
        self.extend([I::I32Const(0), I::Return, I::End]);
        self.root_value(result, output);
        let ready = self.packed_tuple(ready_ty, &[run, result])?;
        self.root_value(ready, ready_ty);
        let stored = self.enum_value(self.key.node, slot, 2, Some(ready))?;
        self.copy(cell, 0, stored, width);
        Ok(result)
    }
}
