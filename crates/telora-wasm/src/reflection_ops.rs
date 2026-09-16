//! Type-bound construction from a sealed, linked metadata table.
use crate::{
    abi::*,
    emit::Emitter,
    reflection_data::{KINDS, ROW},
};
use telora_core::mir::{TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn read32(&mut self, pointer: u32, offset: u64) -> u32 {
        let value = self.local(ValType::I32);
        self.extend([
            I::LocalGet(pointer),
            I::I32Load(memory(offset, 2)),
            I::LocalSet(value),
        ]);
        value
    }
    pub(crate) fn reflection_row(&mut self, input: u32) -> (u32, u32) {
        self.bits(input);
        self.extend([
            I::I64Const(self.mir.types.len() as i64),
            I::I64GeU,
            I::If(BlockType::Empty),
            I::Unreachable,
            I::End,
        ]);
        let id = self.read32(input, DATA);
        self.type_row(id)
    }
    pub(crate) fn type_row(&mut self, id: u32) -> (u32, u32) {
        self.extend([
            I::LocalGet(id),
            I::I32Const(self.mir.types.len() as i32),
            I::I32GeU,
            I::If(BlockType::Empty),
            I::Unreachable,
            I::End,
        ]);
        let base = self.static_base();
        let row = self.local(ValType::I32);
        self.extend([
            I::LocalGet(base),
            I::LocalGet(id),
            I::I32Const(ROW as i32),
            I::I32Mul,
            I::I32Add,
            I::LocalSet(row),
        ]);
        (base, row)
    }
    pub(crate) fn reflected_scalar(
        &mut self,
        ty: TypeId,
        bits: u32,
        input: u32,
    ) -> Result<u32, String> {
        let value = self.value_as(self.key.node, ty, SCALAR_BYTES)?;
        self.extend([
            I::LocalGet(value),
            I::LocalGet(bits),
            I::I64ExtendI32U,
            I::I64Store(memory(DATA, 3)),
        ]);
        self.copy(value, 0, input, LOC_BYTES);
        Ok(value)
    }
    pub(crate) fn reflected_text(
        &mut self,
        ty: TypeId,
        base: u32,
        span: u32,
        offset: u64,
        input: u32,
    ) -> Result<u32, String> {
        let raw = self.alloc(8);
        self.extend([
            I::LocalGet(raw),
            I::LocalGet(base),
            I::LocalGet(span),
            I::I32Load(memory(offset, 2)),
            I::I32Add,
            I::I32Store(memory(0, 2)),
            I::LocalGet(raw),
            I::LocalGet(span),
            I::I32Load(memory(offset + 4, 2)),
            I::I32Store(memory(4, 2)),
        ]);
        let value = self.text_span_value(ty, raw)?;
        self.copy(value, 0, input, LOC_BYTES);
        Ok(value)
    }
    pub(crate) fn reflection_failure(&mut self, input: u32, message: &str) -> Result<(), String> {
        let message = self.text_as(self.key.node, self.string_type()?, message.as_bytes())?;
        let count = self.local(ValType::I32);
        self.extend([I::I32Const(1), I::LocalSet(count)]);
        self.report(self.key.node, message, input, count, false);
        Ok(())
    }
    pub fn reflect_native(&mut self, name: &str) -> Result<u32, String> {
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        if args.len() != 2 || self.mir.types[args[0].index()].constructor != T::Type {
            return Err("Wasm: type reflection signature mismatch".into());
        }
        let output = args[1];
        let input = self.parameter(0);
        let (base, row) = self.reflection_row(input);
        if matches!(name, "children" | "fields" | "variants") {
            return self.reflect_collection(name, output, input, base, row);
        }
        if name == "kind" {
            let variants = &self.plan.layouts[output.index()].variants;
            if variants.len() != KINDS.len() || variants.iter().any(|v| v.type_id.is_some()) {
                return Err("Wasm: type kind enum differs from admitted ABI".into());
            }
            let kind = self.read32(row, 0);
            for (tag, name) in KINDS.iter().enumerate() {
                let index = variants
                    .iter()
                    .position(|v| v.name == *name)
                    .ok_or("Wasm: type kind variant missing")?;
                self.extend([
                    I::LocalGet(kind),
                    I::I32Const(tag as i32),
                    I::I32Eq,
                    I::If(BlockType::Empty),
                ]);
                let value = self.enum_value(node, output, index as u32, None)?;
                self.copy(value, 0, input, LOC_BYTES);
                self.extend([I::LocalGet(value), I::Return, I::End]);
            }
            self.reflection_failure(input, "unsupported type descriptor kind")?;
            return Ok(self.local(ValType::I32));
        }
        let shape = &self.mir.types[output.index()];
        let result = self.local(ValType::I32);
        if name == "opaque_name" {
            if shape.constructor != T::Option
                || shape.arguments.len() != 1
                || self.mir.types[shape.arguments[0].index()].constructor != T::String
            {
                return Err("Wasm: opaque name result signature mismatch".into());
            }
            let string = shape.arguments[0];
            self.extend([
                I::LocalGet(row),
                I::I32Load(memory(28, 2)),
                I::If(BlockType::Empty),
            ]);
            let value = self.reflected_text(string, base, row, 24, input)?;
            let some = self.enum_value(node, output, 1, Some(value))?;
            self.extend([I::LocalGet(some), I::LocalSet(result), I::Else]);
            let none = self.enum_value(node, output, 0, None)?;
            self.extend([I::LocalGet(none), I::LocalSet(result), I::End]);
        } else if name == "resolve_raw" {
            if shape.constructor != T::Result
                || shape.arguments.len() != 2
                || shape.arguments[0] != args[0]
                || self.mir.types[shape.arguments[1].index()].constructor != T::String
            {
                return Err("Wasm: type resolve result signature mismatch".into());
            }
            let string = shape.arguments[1];
            let body = self.read32(row, 4);
            self.extend([
                I::LocalGet(body),
                I::I32Const(-1),
                I::I32Ne,
                I::If(BlockType::Empty),
            ]);
            let value = self.reflected_scalar(args[0], body, input)?;
            let ok = self.enum_value(node, output, 1, Some(value))?;
            self.extend([I::LocalGet(ok), I::LocalSet(result), I::Else]);
            let message = self.text_as(
                node,
                string,
                b"type descriptor is not a recursive reference",
            )?;
            self.copy(message, 0, input, LOC_BYTES);
            let error = self.enum_value(node, output, 0, Some(message))?;
            self.extend([I::LocalGet(error), I::LocalSet(result), I::End]);
        } else {
            return Err(format!("Wasm: type reflection not implemented: {name}"));
        }
        self.copy(result, 0, input, LOC_BYTES);
        Ok(result)
    }
}
