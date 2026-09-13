//! Linear materialization of a postorder parse plan into sealed Value layouts.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::{TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    fn parse_address(&mut self, base: u32, index: u32, stride: u32) -> u32 {
        let result = self.local(ValType::I32);
        self.extend([
            I::LocalGet(base),
            I::LocalGet(index),
            I::I32Const(stride as i32),
            I::I32Mul,
            I::I32Add,
            I::LocalSet(result),
        ]);
        result
    }
    fn parse_buffer(&mut self, count: u32, stride: u32) -> u32 {
        let result = self.local(ValType::I32);
        self.extend([
            I::LocalGet(count),
            I::I32Const(stride as i32),
            I::I32Mul,
            I::Call(ALLOC),
            I::LocalSet(result),
        ]);
        result
    }
    fn parse_column(&mut self, base: u32, count: u32, stride: u32) -> u32 {
        let id = self.local(ValType::I32);
        self.extend([
            I::I32Const(table_address(ARRAYS) as i32),
            I::LocalGet(base),
            I::LocalGet(count),
            I::I32Const(stride as i32),
            I::I32Mul,
            I::Call(TABLE_PUSH),
            I::LocalSet(id),
        ]);
        id
    }
    pub fn json_parse_native(&mut self) -> Result<u32, String> {
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        if args.len() != 3
            || self.mir.types[args[0].index()].constructor != T::TypeOf
            || self.mir.types[args[1].index()].constructor != T::String
            || self.mir.types[args[2].index()].constructor != T::Result
        {
            return Err("Wasm: JSON parse signature mismatch".into());
        }
        let target = self.mir.types[args[0].index()].arguments[0];
        let results = self.mir.types[args[2].index()].arguments.clone();
        if results[0] != target {
            return Err("Wasm: JSON parse result mismatch".into());
        }
        let input = self.parameter(1);
        let packet = self.local(ValType::I32);
        self.extend([I::LocalGet(input), I::Call(JSON_PARSE), I::LocalSet(packet)]);
        let error = self.read32(packet, 12);
        self.extend([I::LocalGet(error), I::If(BlockType::Empty)]);
        let message = self.text_span_value(args[1], error)?;
        let blame = self.codec_blame(results[1], message, input)?;
        let rejected = self.enum_value(node, args[2], 0, Some(blame))?;
        self.extend([I::LocalGet(rejected), I::Return, I::End]);
        let rows = self.read32(packet, 0);
        let count = self.read32(packet, 4);
        let values = self.parse_buffer(count, 4);
        let index = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(index),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let row = self.parse_address(rows, index, 16);
        let kind = self.read32(row, 0);
        let value = self.local(ValType::I32);
        for (code, name) in [
            "None", "True", "False", "Int", "Float", "String", "Array", "Object",
        ]
        .iter()
        .enumerate()
        {
            let variant = self.plan.layouts[target.index()]
                .variants
                .iter()
                .position(|v| v.name == *name)
                .ok_or("Wasm: incomplete Value parse contract")?;
            let payload_ty = self.plan.layouts[target.index()].variants[variant]
                .type_id
                .map(|id| self.plan.layouts[id].id());
            self.extend([
                I::LocalGet(kind),
                I::I32Const(code as i32),
                I::I32Eq,
                I::If(BlockType::Empty),
            ]);
            let payload = if let Some(ty) = payload_ty {
                let payload = match code {
                    3 | 4 => {
                        let p = self.value_as(node, ty, 24)?;
                        self.extend([
                            I::LocalGet(p),
                            I::LocalGet(row),
                            I::I64Load(memory(8, 3)),
                            I::I64Store(memory(DATA, 3)),
                        ]);
                        p
                    }
                    5 => {
                        let span = self.local(ValType::I32);
                        self.extend([
                            I::LocalGet(row),
                            I::I32Const(8),
                            I::I32Add,
                            I::LocalSet(span),
                        ]);
                        self.text_span_value(ty, span)?
                    }
                    6 | 7 => self.parse_collection(ty, target, row, values, input, code == 7)?,
                    _ => return Err("Wasm: unexpected payload in Value contract".into()),
                };
                self.copy(payload, 0, input, 12);
                Some(payload)
            } else {
                None
            };
            let item = self.enum_value(node, target, variant as u32, payload)?;
            self.copy(item, 0, input, 12);
            self.extend([I::LocalGet(item), I::LocalSet(value), I::End]);
        }
        let slot = self.parse_address(values, index, 4);
        self.extend([
            I::LocalGet(slot),
            I::LocalGet(value),
            I::I32Store(memory(0, 2)),
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(index),
            I::Br(0),
            I::End,
            I::End,
        ]);
        let root = self.read32(packet, 8);
        let slot = self.parse_address(values, root, 4);
        let value = self.read32(slot, 0);
        self.enum_value(node, args[2], 1, Some(value))
    }
    fn parse_collection(
        &mut self,
        ty: TypeId,
        target: TypeId,
        row: u32,
        values: u32,
        input: u32,
        object: bool,
    ) -> Result<u32, String> {
        let count = self.read32(row, 12);
        let entries = self.read32(row, 8);
        let width = self.width(target)?;
        let data = self.parse_buffer(count, width);
        let keys = if object {
            Some(self.parse_buffer(count, 32))
        } else {
            None
        };
        let index = self.local(ValType::I32);
        self.extend([
            I::I32Const(0),
            I::LocalSet(index),
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(index),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let entry = self.parse_address(entries, index, if object { 12 } else { 4 });
        let child = self.read32(entry, if object { 8 } else { 0 });
        let slot = self.parse_address(values, child, 4);
        let value = self.read32(slot, 0);
        let destination = self.parse_address(data, index, width);
        self.copy(destination, 0, value, width);
        if let Some(keys) = keys {
            let key = self.text_span_value(self.string_type()?, entry)?;
            self.copy(key, 0, input, 12);
            let destination = self.parse_address(keys, index, 32);
            self.copy(destination, 0, key, 32);
        }
        self.extend([
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(index),
            I::Br(0),
            I::End,
            I::End,
        ]);
        let id = self.parse_column(data, count, width);
        let output = self.value_as(self.key.node, ty, 32)?;
        if let Some(keys) = keys {
            let key_id = self.parse_column(keys, count, 32);
            for (offset, local) in [(16, key_id), (20, count), (24, id)] {
                self.extend([
                    I::LocalGet(output),
                    I::LocalGet(local),
                    I::I32Store(memory(offset, 2)),
                ]);
            }
        } else {
            self.extend([
                I::LocalGet(output),
                I::LocalGet(id),
                I::I32Store(memory(16, 2)),
                I::LocalGet(output),
                I::LocalGet(count),
                I::I32Store(memory(24, 2)),
            ]);
            self.store32(output, 20, 0);
        }
        self.store32(output, 28, 0);
        Ok(output)
    }
}
