//! Assemble a prepared template using the sealed tuple/array/String identities.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::TypeConstructor as T;
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub fn template_prepare(&mut self) -> Result<u32, String> {
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        if args.len() != 2 || self.mir.types[args[0].index()].constructor != T::String {
            return Err("Wasm: template prepare argument mismatch".into());
        }
        let output = &self.mir.types[args[1].index()];
        if output.constructor != T::Tuple
            || output.arguments.len() != 2
            || output.arguments.iter().any(|ty| {
                let shape = &self.mir.types[ty.index()];
                shape.constructor != T::Array || shape.arguments != [args[0]]
            })
        {
            return Err("Wasm: template prepare result mismatch".into());
        }
        let arrays = output.arguments.clone();
        let source = self.parameter(0);
        let raw = self.local(ValType::I32);
        let error = self.local(ValType::I32);
        self.extend([
            I::LocalGet(source),
            I::Call(TEMPLATE_PREPARE),
            I::LocalTee(raw),
            I::I32Load(memory(16, 2)),
            I::LocalTee(error),
            I::If(BlockType::Empty),
        ]);
        let message = self.text_span_value(args[0], error)?;
        let count = self.local(ValType::I32);
        self.extend([I::I32Const(1), I::LocalSet(count)]);
        self.report(node, message, source, count, false);
        self.emit(I::End);
        let mut values = vec![];
        for (index, ty) in arrays.into_iter().enumerate() {
            let base = self.local(ValType::I32);
            let count = self.local(ValType::I32);
            self.extend([
                I::LocalGet(raw),
                I::I32Load(memory(index as u64 * 8, 2)),
                I::LocalSet(base),
                I::LocalGet(raw),
                I::I32Load(memory(index as u64 * 8 + 4, 2)),
                I::LocalSet(count),
            ]);
            values.push(self.text_span_array(ty, base, count, None)?);
        }
        self.packed_tuple(args[1], &values)
    }
}
