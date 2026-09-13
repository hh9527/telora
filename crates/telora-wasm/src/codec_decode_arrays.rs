//! Closed collection layouts and per-element decode calls.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::{TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn codec_decode_array(
        &mut self,
        source: TypeId,
        target: TypeId,
        input: u32,
    ) -> Result<u32, String> {
        let dict = self.mir.types[target.index()].constructor == T::Dict;
        let tag = if dict { "Object" } else { "Array" };
        let index = self.plan.layouts[source.index()]
            .variants
            .iter()
            .position(|v| v.name == tag)
            .ok_or("Wasm: codec collection variant missing")?;
        self.extend([
            I::LocalGet(input),
            I::I32Load(memory(DATA, 2)),
            I::I32Const(index as i32),
            I::I32Ne,
            I::If(BlockType::Empty),
        ]);
        self.codec_decode_reject(
            if dict {
                "expected Object"
            } else {
                "expected Array"
            },
            input,
        )?;
        self.emit(I::End);
        let collection = self.enum_payload(source, index as u32, input)?;
        let inner = self.mir.types[target.index()].arguments[0];
        let stride = self.width(source)?;
        let width = self.width(inner)?;
        let base = self.table_data(ARRAYS, collection, if dict { 24 } else { DATA });
        let start = if dict {
            self.local(ValType::I32)
        } else {
            self.read32(collection, 20)
        };
        let end = self.read32(collection, if dict { 20 } else { 24 });
        let keys = if dict {
            Some(self.table_data(ARRAYS, collection, DATA))
        } else {
            None
        };
        let bytes = self.local(ValType::I32);
        let data = self.local(ValType::I32);
        let cursor = self.local(ValType::I32);
        let context = self.alloc(32);
        self.copy(context, 0, 0, 32);
        let path = self.read32(0, 24);
        self.extend([
            I::LocalGet(end),
            I::LocalGet(start),
            I::I32Sub,
            I::I32Const(width as i32),
            I::I32Mul,
            I::LocalTee(bytes),
            I::Call(ALLOC),
            I::LocalSet(data),
            I::LocalGet(start),
            I::LocalSet(cursor),
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(cursor),
            I::LocalGet(end),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let item = self.local(ValType::I32);
        let position = self.local(ValType::I32);
        self.extend([
            I::LocalGet(base),
            I::LocalGet(cursor),
            I::I32Const(stride as i32),
            I::I32Mul,
            I::I32Add,
            I::LocalSet(item),
            I::LocalGet(cursor),
            I::LocalGet(start),
            I::I32Sub,
            I::LocalSet(position),
        ]);
        let child_path = if let Some(keys) = keys {
            let key = self.local(ValType::I32);
            self.extend([
                I::LocalGet(keys),
                I::LocalGet(cursor),
                I::I32Const(32),
                I::I32Mul,
                I::I32Add,
                I::LocalSet(key),
            ]);
            self.parse_text(7, path, key)?
        } else {
            self.parse_text(8, path, position)?
        };
        self.extend([
            I::LocalGet(context),
            I::LocalGet(child_path),
            I::I32Store(memory(24, 2)),
        ]);
        let decoded = self.codec_decode_call(source, inner, item, context)?;
        self.parse_propagate(decoded);
        self.extend([
            I::LocalGet(data),
            I::LocalGet(position),
            I::I32Const(width as i32),
            I::I32Mul,
            I::I32Add,
            I::LocalGet(decoded),
            I::I32Const(width as i32),
            I::MemoryCopy {
                src_mem: 0,
                dst_mem: 0,
            },
            I::LocalGet(cursor),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(cursor),
            I::Br(0),
            I::End,
            I::End,
        ]);
        let id = self.local(ValType::I32);
        self.extend([
            I::I32Const(table_address(ARRAYS) as i32),
            I::LocalGet(data),
            I::LocalGet(bytes),
            I::Call(TABLE_PUSH),
            I::LocalSet(id),
        ]);
        let value = self.value_as(self.key.node, target, 32)?;
        self.copy(value, 0, input, 12);
        if dict {
            let keys = self.read32(collection, DATA);
            for (offset, local) in [(16, keys), (20, end), (24, id)] {
                self.extend([
                    I::LocalGet(value),
                    I::LocalGet(local),
                    I::I32Store(memory(offset, 2)),
                ]);
            }
        } else {
            self.extend([
                I::LocalGet(value),
                I::LocalGet(id),
                I::I32Store(memory(DATA, 2)),
                I::LocalGet(value),
                I::LocalGet(end),
                I::LocalGet(start),
                I::I32Sub,
                I::I32Store(memory(24, 2)),
            ]);
            self.store32(value, 20, 0);
        }
        self.store32(value, 28, 0);
        Ok(value)
    }
}
