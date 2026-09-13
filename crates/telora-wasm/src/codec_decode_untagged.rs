//! Untagged candidates keep ordinary rejections separate from evaluation failure.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::{NativeTypeId, TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn codec_decode_untagged(
        &mut self,
        source: TypeId,
        target: TypeId,
        input: u32,
    ) -> Result<u32, String> {
        let variants: Vec<_> = self.plan.layouts[target.index()]
            .variants
            .iter()
            .map(|v| v.type_id)
            .collect();
        let null = self.plan.layouts[source.index()]
            .variants
            .iter()
            .position(|v| v.name == "None")
            .ok_or("Wasm: codec Value lacks None")?;
        let blame_ty = self
            .mir
            .types
            .iter()
            .enumerate()
            .find(|(_, t)| t.constructor == T::Native(NativeTypeId::BLAME_ERROR))
            .map(|(id, _)| self.plan.layouts[id].id())
            .ok_or("Wasm: codec Blame identity missing")?;
        let error = self.read32(0, 28);
        let count = self.local(ValType::I32);
        let selected = self.local(ValType::I32);
        let first = self.local(ValType::I32);
        let messages = self.text_as(self.key.node, self.string_type()?, b"")?;
        for (index, ty) in variants.iter().enumerate() {
            for offset in [0, 4, 8] {
                self.store32(error, offset, 0);
            }
            if ty.is_none() {
                self.extend([
                    I::LocalGet(input),
                    I::I32Load(memory(DATA, 2)),
                    I::I32Const(null as i32),
                    I::I32Eq,
                    I::If(BlockType::Empty),
                ]);
                let value = self.enum_value(self.key.node, target, index as u32, None)?;
                self.copy(value, 0, input, 12);
                self.extend([
                    I::LocalGet(value),
                    I::LocalSet(selected),
                    I::LocalGet(count),
                    I::I32Const(1),
                    I::I32Add,
                    I::LocalSet(count),
                    I::End,
                ]);
                continue;
            }
            let value = self.codec_decode_variant_call(source, target, index as u32, input)?;
            self.extend([
                I::LocalGet(value),
                I::If(BlockType::Empty),
                I::LocalGet(value),
                I::LocalSet(selected),
                I::LocalGet(count),
                I::I32Const(1),
                I::I32Add,
                I::LocalSet(count),
                I::Else,
            ]);
            let blame = self.read32(error, 8);
            self.extend([I::LocalGet(blame), I::I32Eqz, I::If(BlockType::Empty)]);
            let message = self.read32(error, 0);
            self.parse_propagate(message);
            let subject = self.read32(error, 4);
            let created = self.codec_blame(blame_ty, message, subject)?;
            self.extend([I::LocalGet(created), I::LocalSet(blame), I::End]);
            self.extend([I::LocalGet(first), I::If(BlockType::Empty)]);
            let separator = self.text_as(self.key.node, self.string_type()?, b"; ")?;
            let joined = self.parse_text(9, messages, separator)?;
            self.extend([
                I::LocalGet(joined),
                I::LocalSet(messages),
                I::Else,
                I::LocalGet(blame),
                I::LocalSet(first),
                I::End,
            ]);
            let (message, _, _) = self.blame_parts(blame);
            let joined = self.parse_text(9, messages, message)?;
            self.extend([I::LocalGet(joined), I::LocalSet(messages), I::End]);
        }
        for offset in [0, 4, 8] {
            self.store32(error, offset, 0);
        }
        self.extend([
            I::LocalGet(count),
            I::I32Const(1),
            I::I32Eq,
            I::If(BlockType::Empty),
            I::LocalGet(selected),
            I::Return,
            I::End,
        ]);
        self.extend([I::LocalGet(count), I::I32Eqz, I::If(BlockType::Empty)]);
        let prefix = self.text_as(
            self.key.node,
            self.string_type()?,
            b"value matches no untagged Enum variant (",
        )?;
        let suffix = self.text_as(self.key.node, self.string_type()?, b")")?;
        let message = self.parse_text(9, prefix, messages)?;
        let message = self.parse_text(9, message, suffix)?;
        let path = self.read32(0, 24);
        let message = self.parse_text(6, path, message)?;
        let blame = self.local(ValType::I32);
        self.extend([I::LocalGet(first), I::If(BlockType::Empty)]);
        let reworded = self.codec_blame_reword(first, message)?;
        self.extend([I::LocalGet(reworded), I::LocalSet(blame), I::Else]);
        let created = self.codec_blame(blame_ty, message, input)?;
        self.extend([
            I::LocalGet(created),
            I::LocalSet(blame),
            I::End,
            I::LocalGet(error),
            I::LocalGet(blame),
            I::I32Store(memory(8, 2)),
            I::I32Const(0),
            I::Return,
            I::End,
        ]);
        self.codec_decode_reject(
            "value ambiguously matches multiple untagged Enum variants",
            input,
        )?;
        Ok(self.local(ValType::I32))
    }

    fn codec_blame_reword(&mut self, original: u32, message: u32) -> Result<u32, String> {
        let (data, _, count) = self.blame_parts(original);
        let bytes = self.local(ValType::I32);
        let object = self.local(ValType::I32);
        let id = self.local(ValType::I32);
        self.extend([
            I::LocalGet(count),
            I::I32Const(12),
            I::I32Mul,
            I::I32Const(40),
            I::I32Add,
            I::LocalTee(bytes),
            I::Call(ALLOC),
            I::LocalSet(object),
            I::LocalGet(object),
            I::LocalGet(data),
            I::LocalGet(bytes),
            I::MemoryCopy {
                src_mem: 0,
                dst_mem: 0,
            },
        ]);
        self.copy(object, 0, message, 32);
        self.extend([
            I::I32Const(table_address(BLAMES) as i32),
            I::LocalGet(object),
            I::LocalGet(bytes),
            I::Call(TABLE_PUSH),
            I::LocalSet(id),
        ]);
        let result = self.alloc(24);
        self.copy(result, 0, original, 24);
        self.extend([
            I::LocalGet(result),
            I::LocalGet(id),
            I::I64ExtendI32U,
            I::I64Store(memory(DATA, 3)),
        ]);
        Ok(result)
    }
}
