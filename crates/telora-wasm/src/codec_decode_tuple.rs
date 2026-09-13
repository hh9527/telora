//! Tuple slots are populated using their sealed types and physical offsets.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::TypeId;
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn codec_decode_tuple(
        &mut self,
        source: TypeId,
        target: TypeId,
        input: u32,
    ) -> Result<u32, String> {
        let index = self.plan.layouts[source.index()]
            .variants
            .iter()
            .position(|v| v.name == "Array")
            .ok_or("Wasm: codec Value lacks Array")?;
        self.extend([
            I::LocalGet(input),
            I::I32Load(memory(DATA, 2)),
            I::I32Const(index as i32),
            I::I32Ne,
            I::If(BlockType::Empty),
        ]);
        self.codec_decode_reject("expected Array", input)?;
        self.emit(I::End);
        let collection = self.enum_payload(source, index as u32, input)?;
        let children = self.mir.types[target.index()].arguments.clone();
        let start = self.read32(collection, 20);
        let end = self.read32(collection, 24);
        self.extend([
            I::LocalGet(end),
            I::LocalGet(start),
            I::I32Sub,
            I::I32Const(children.len() as i32),
            I::I32Ne,
            I::If(BlockType::Empty),
        ]);
        self.codec_decode_reject("expected tuple with the declared arity", input)?;
        self.emit(I::End);
        let mut fields = Vec::with_capacity(children.len());
        if !children.is_empty() {
            let base = self.table_data(ARRAYS, collection, DATA);
            let stride = self.width(source)?;
            let path = self.read32(0, 24);
            let context = self.alloc(32);
            self.copy(context, 0, 0, 32);
            for (index, ty) in children.into_iter().enumerate() {
                let item = self.local(ValType::I32);
                let position = self.local(ValType::I32);
                self.extend([
                    I::I32Const(index as i32),
                    I::LocalSet(position),
                    I::LocalGet(base),
                    I::LocalGet(start),
                    I::LocalGet(position),
                    I::I32Add,
                    I::I32Const(stride as i32),
                    I::I32Mul,
                    I::I32Add,
                    I::LocalSet(item),
                ]);
                let child_path = self.parse_text(8, path, position)?;
                self.extend([
                    I::LocalGet(context),
                    I::LocalGet(child_path),
                    I::I32Store(memory(24, 2)),
                ]);
                let decoded = self.codec_decode_call(source, ty, item, context)?;
                self.parse_propagate(decoded);
                fields.push(decoded);
            }
        }
        let value = self.packed_tuple(target, &fields)?;
        self.copy(value, 0, input, 12);
        Ok(value)
    }
}
