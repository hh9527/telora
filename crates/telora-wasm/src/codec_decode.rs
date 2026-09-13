use crate::{abi::*, emit::Emitter};
use telora_core::mir::TypeConstructor as T;
use wasm_encoder::{BlockType, Instruction as I};

impl Emitter<'_> {
    pub(crate) fn codec_decode_native(&mut self) -> Result<u32, String> {
        let args = self.mir.types[self.ty(self.key.node)?.index()]
            .arguments
            .clone();
        if args.len() != 4
            || self.mir.types[args[1].index()].constructor != T::TypeOf
            || self.mir.types[args[3].index()].constructor != T::Result
        {
            return Err("Wasm: codec decode signature mismatch".into());
        }
        let target = self.mir.types[args[1].index()].arguments[0];
        let result_types = self.mir.types[args[3].index()].arguments.clone();
        if result_types[0] != target {
            return Err("Wasm: codec decode result mismatch".into());
        }
        let input = self.parameter(2);
        if target == args[2] {
            return self.enum_value(self.key.node, args[3], 1, Some(input));
        }
        let expected = match self.mir.types[target.index()].constructor {
            T::Int => "Int",
            T::Float => "Float",
            T::String => "String",
            T::Bytes => "Bytes",
            T::Bool => "Bool",
            _ => return Err("Wasm: codec decode target not yet implemented".into()),
        };
        let variants: Vec<_> = self.plan.layouts[args[2].index()]
            .variants
            .iter()
            .enumerate()
            .filter(|(_, v)| {
                v.name == expected
                    || expected == "Bool" && matches!(v.name.as_str(), "True" | "False")
            })
            .map(|(i, v)| (i, v.name.clone(), v.type_id))
            .collect();
        for (index, name, payload_ty) in variants {
            self.extend([
                I::LocalGet(input),
                I::I32Load(memory(DATA, 2)),
                I::I32Const(index as i32),
                I::I32Eq,
                I::If(BlockType::Empty),
            ]);
            let value = if expected == "Bool" {
                let value = self.value_as(self.key.node, target, 24)?;
                self.store32(value, DATA, u32::from(name == "True"));
                self.store32(value, 20, 0);
                self.copy(value, 0, input, 12);
                value
            } else {
                if payload_ty != Some(target.index()) {
                    return Err("Wasm: codec scalar identity mismatch".into());
                }
                self.enum_payload(args[2], index as u32, input)?
            };
            let result = self.enum_value(self.key.node, args[3], 1, Some(value))?;
            self.extend([I::LocalGet(result), I::Return, I::End]);
        }
        let message = self.text_as(
            self.key.node,
            self.string_type()?,
            format!("$: expected {expected}").as_bytes(),
        )?;
        let object = self.alloc(56);
        self.copy(object, 0, message, 32);
        self.store32(object, 32, 1);
        self.store32(object, 36, 0);
        self.copy(object, 40, input, 12);
        let id = self.table_push(BLAMES, object, 56);
        let blame = self.value_as(self.key.node, result_types[1], 24)?;
        self.extend([
            I::LocalGet(blame),
            I::LocalGet(id),
            I::I64ExtendI32U,
            I::I64Store(memory(DATA, 3)),
        ]);
        self.enum_value(self.key.node, args[3], 0, Some(blame))
    }
}
