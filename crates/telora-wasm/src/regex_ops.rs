use crate::{
    abi::*,
    emit::Emitter,
    plan::{Key, Special},
};
use telora_core::mir::TypeConstructor as T;
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub fn regex_native(&mut self, name: &str) -> Result<u32, String> {
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        let regex = |ty: telora_core::mir::TypeId| matches!(self.mir.types[ty.index()].constructor, T::Native(id) if (id.module,id.slot) == (19,0));
        if name == "parse_by" {
            if self.key.special != Special::Configured {
                if args.len() != 2
                    || !regex(args[0])
                    || self.mir.types[args[1].index()].constructor != T::Function
                {
                    return Err("Wasm: regex parse_by factory signature mismatch".into());
                }
                let pattern = self.parameter(0);
                return self.function_value(
                    node,
                    Key {
                        special: Special::Configured,
                        ..self.key
                    },
                    args[1],
                    &[pattern],
                );
            }
            if args.len() != 3
                || self.mir.types[args[1].index()].constructor != T::Option
                || self.mir.types[args[1].index()].arguments != [args[2]]
            {
                return Err("Wasm: regex parse_by provider signature mismatch".into());
            }
            let pattern = self.local(ValType::I32);
            self.extend([
                I::LocalGet(0),
                I::I32Load(memory(0, 2)),
                I::LocalSet(pattern),
            ]);
            return self.packed_tuple(args[2], &[pattern]);
        }
        if name == "compile"
            && args.len() == 2
            && self.mir.types[args[0].index()].constructor == T::String
            && regex(args[1])
        {
            let input = self.parameter(0);
            let packet = self.local(ValType::I32);
            self.extend([
                I::I32Const(0),
                I::LocalGet(input),
                I::I32Const(0),
                I::Call(REGEX),
                I::LocalSet(packet),
            ]);
            let error = self.read32(packet, 4);
            self.extend([I::LocalGet(error), I::If(BlockType::Empty)]);
            let message = self.text_span_value(args[0], error)?;
            let count = self.local(ValType::I32);
            self.extend([I::I32Const(1), I::LocalSet(count)]);
            self.report(node, message, input, count, false);
            self.emit(I::End);
            let id = self.read32(packet, 0);
            return self.reflected_scalar(args[1], id, input);
        }
        if name == "is_match"
            && args.len() == 3
            && regex(args[0])
            && self.mir.types[args[1].index()].constructor == T::String
            && self.mir.types[args[2].index()].constructor == T::Bool
        {
            let pattern = self.parameter(0);
            let input = self.parameter(1);
            let id = self.read32(pattern, DATA);
            let matched = self.local(ValType::I32);
            self.extend([
                I::I32Const(1),
                I::LocalGet(id),
                I::LocalGet(input),
                I::Call(REGEX),
                I::LocalSet(matched),
            ]);
            return self.reflected_scalar(args[2], matched, input);
        }
        Err(format!(
            "Wasm: regex native not implemented or signature mismatch: {name}"
        ))
    }
}
