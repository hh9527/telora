use crate::{abi::*, emit::Emitter};
use telora_core::mir::TypeId;
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn codec_encode_tuple(
        &mut self,
        source: TypeId,
        target: TypeId,
        input: u32,
    ) -> Result<u32, String> {
        let members: Vec<_> = if self.mir.types[source.index()].arguments.is_empty() {
            Vec::new()
        } else {
            self.plan.layouts[source.index()]
                .object
                .as_ref()
                .ok_or("Wasm: codec tuple layout missing")?
                .members
                .iter()
                .map(|m| (m.type_id, m.offset))
                .collect()
        };
        let width = self.width(target)?;
        let bytes = (members.len() as u32)
            .checked_mul(width)
            .ok_or("Wasm: codec tuple size overflow")?;
        let data = self.alloc(bytes);
        // Unit has no object handle in its value header.
        if !members.is_empty() {
            let base = self.table_data(RECORDS, input, DATA);
            for (index, (ty, offset)) in members.iter().enumerate() {
                let ty = self.plan.layouts[ty.ok_or("Wasm: tuple element type missing")?].id();
                let value = self.local(ValType::I32);
                self.extend([
                    I::LocalGet(base),
                    I::I32Const(offset.ok_or("Wasm: tuple element offset missing")? as i32),
                    I::I32Add,
                    I::LocalSet(value),
                ]);
                let encoded = self.codec_encode_scalar(ty, target, value)?;
                self.copy(data, index as u32 * width, encoded, width);
            }
        }
        let payload_ty = self.plan.layouts[target.index()]
            .variants
            .iter()
            .find(|v| v.name == "Array")
            .and_then(|v| v.type_id)
            .ok_or("Wasm: codec Array payload missing")?;
        let id = self.table_push(ARRAYS, data, bytes);
        let payload = self.value_as(self.key.node, self.plan.layouts[payload_ty].id(), 32)?;
        self.copy(payload, 0, input, 12);
        self.extend([
            I::LocalGet(payload),
            I::LocalGet(id),
            I::I32Store(memory(DATA, 2)),
        ]);
        self.store32(payload, 20, 0);
        self.store32(payload, 24, members.len() as u32);
        self.store32(payload, 28, 0);
        self.codec_variant(target, "Array", Some(payload), input)
    }
    pub(crate) fn codec_encode_array(
        &mut self,
        source: TypeId,
        target: TypeId,
        input: u32,
    ) -> Result<u32, String> {
        let inner = self.mir.types[source.index()].arguments[0];
        let dictionary =
            self.mir.types[source.index()].constructor == telora_core::mir::TypeConstructor::Dict;
        let tag = if dictionary { "Object" } else { "Array" };
        let payload_ty = self.plan.layouts[target.index()]
            .variants
            .iter()
            .find(|v| v.name == tag)
            .and_then(|v| v.type_id)
            .ok_or("Wasm: codec Value.Array payload missing")?;
        let payload_ty = self.plan.layouts[payload_ty].id();
        let stride = self.width(inner)?;
        let width = self.width(target)?;
        let base = self.table_data(ARRAYS, input, if dictionary { 24 } else { DATA });
        let start = if dictionary {
            self.local(ValType::I32)
        } else {
            self.read32(input, 20)
        };
        let end = self.read32(input, if dictionary { 20 } else { 24 });
        let bytes = self.local(ValType::I32);
        let data = self.local(ValType::I32);
        let cursor = self.local(ValType::I32);
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
        self.extend([
            I::LocalGet(base),
            I::LocalGet(cursor),
            I::I32Const(stride as i32),
            I::I32Mul,
            I::I32Add,
            I::LocalSet(item),
        ]);
        let encoded = self.codec_encode_scalar(inner, target, item)?;
        self.extend([
            I::LocalGet(data),
            I::LocalGet(cursor),
            I::LocalGet(start),
            I::I32Sub,
            I::I32Const(width as i32),
            I::I32Mul,
            I::I32Add,
            I::LocalGet(encoded),
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
        let payload = self.value_as(self.key.node, payload_ty, 32)?;
        self.copy(payload, 0, input, 12);
        if dictionary {
            let keys = self.read32(input, DATA);
            for (offset, value) in [(16, keys), (20, end), (24, id)] {
                self.extend([
                    I::LocalGet(payload),
                    I::LocalGet(value),
                    I::I32Store(memory(offset, 2)),
                ]);
            }
            self.store32(payload, 28, 0);
            return self.codec_variant(target, "Object", Some(payload), input);
        }
        self.extend([
            I::LocalGet(payload),
            I::LocalGet(id),
            I::I32Store(memory(DATA, 2)),
            I::LocalGet(payload),
            I::LocalGet(end),
            I::LocalGet(start),
            I::I32Sub,
            I::I32Store(memory(24, 2)),
        ]);
        self.store32(payload, 20, 0);
        self.store32(payload, 28, 0);
        self.codec_variant(target, "Array", Some(payload), input)
    }
}
