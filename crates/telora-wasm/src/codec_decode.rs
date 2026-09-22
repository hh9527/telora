//! Context: reserved words, path, rejection cell.
//! Rejection cell: message, subject, original checker Blame (all pointers).
use crate::{
    abi::*,
    emit::Emitter,
    plan::{Key, Special},
};
use telora_core::mir::{TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

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
        let context = self.alloc(32);
        let path = self.text_as(self.key.node, self.string_type()?, b"$")?;
        let error = self.alloc(12);
        self.store32(error, 0, 0);
        self.store32(error, 4, 0);
        self.store32(error, 8, 0);
        self.extend([
            I::LocalGet(context),
            I::LocalGet(path),
            I::I32Store(memory(24, 2)),
            I::LocalGet(context),
            I::LocalGet(error),
            I::I32Store(memory(28, 2)),
        ]);
        let decoded = self.codec_decode_call(args[2], target, input, context)?;
        self.extend([I::LocalGet(decoded), I::If(BlockType::Empty)]);
        let ok = self.enum_value(self.key.node, args[3], 1, Some(decoded))?;
        self.extend([I::LocalGet(ok), I::Return, I::End]);
        let blame = self.read32(error, 8);
        self.extend([I::LocalGet(blame), I::If(BlockType::Empty)]);
        let rejected = self.enum_value(self.key.node, args[3], 0, Some(blame))?;
        self.extend([I::LocalGet(rejected), I::Return, I::End]);
        let message = self.read32(error, 0);
        // No rejection means evaluation already failed; preserve that failure.
        self.checked(message);
        let subject = self.read32(error, 4);
        let blame = self.codec_blame(result_types[1], message, subject)?;
        self.enum_value(self.key.node, args[3], 0, Some(blame))
    }

    pub(crate) fn codec_decode_call(
        &mut self,
        source: TypeId,
        target: TypeId,
        input: u32,
        context: u32,
    ) -> Result<u32, String> {
        let key = Key {
            special: Special::Decode(source, target),
            callable: true,
            ..self.plan.root
        };
        let function = *self
            .plan
            .functions
            .get(&key)
            .ok_or("Wasm: closed decoder was not planned")?;
        let result = self.local(ValType::I32);
        self.extend([
            I::LocalGet(context),
            I::LocalGet(input),
            I::Call(function),
            I::LocalSet(result),
        ]);
        Ok(result)
    }

    pub(crate) fn codec_decode_type(
        &mut self,
        source: TypeId,
        target: TypeId,
        input: u32,
    ) -> Result<u32, String> {
        if target == source {
            return Ok(input);
        }
        if matches!(
            self.mir.types[target.index()].constructor,
            T::Nominal(_) | T::Record(_)
        ) {
            return self.codec_decode_nominal(source, target, input);
        }
        if self.mir.types[target.index()].constructor == T::Tuple {
            return self.codec_decode_tuple(source, target, input);
        }
        if matches!(
            self.mir.types[target.index()].constructor,
            T::Array | T::Dict
        ) {
            return self.codec_decode_array(source, target, input);
        }
        if self.mir.types[target.index()].constructor == T::Option {
            let null = self.plan.layouts[source.index()]
                .variants
                .iter()
                .position(|v| v.name == "None")
                .ok_or("Wasm: codec Value lacks None")?;
            self.extend([
                I::LocalGet(input),
                I::I32Load(memory(DATA, 2)),
                I::I32Const(null as i32),
                I::I32Eq,
                I::If(BlockType::Empty),
            ]);
            let none = self.enum_value(self.key.node, target, 0, None)?;
            self.copy(none, 0, input, LOC_BYTES);
            self.extend([I::LocalGet(none), I::Return, I::End]);
            let inner = self.mir.types[target.index()].arguments[0];
            let decoded = self.codec_decode_call(source, inner, input, 0)?;
            self.checked(decoded);
            let some = self.enum_value(self.key.node, target, 1, Some(decoded))?;
            self.copy(some, 0, input, LOC_BYTES);
            return Ok(some);
        }
        if matches!(
            self.mir.types[target.index()].constructor,
            T::Result | T::FoldControl | T::PropertyTarget
        ) {
            return self.codec_decode_enum(source, target, input, false);
        }
        let expected = match self.mir.types[target.index()].constructor {
            T::Int => "Int",
            T::Float => "Float",
            T::String => "String",
            T::Bytes => "Bytes",
            T::Bool => "Bool",
            T::Never => "Never",
            _ => return Err("Wasm: codec decode target not yet implemented".into()),
        };
        let variants: Vec<_> = self.plan.layouts[source.index()]
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
                let value = self.value_as(self.key.node, target, SCALAR_BYTES)?;
                self.store32(value, DATA, u32::from(name == "True"));
                self.store32(value, DATA + 4, 0);
                self.copy(value, 0, input, LOC_BYTES);
                value
            } else {
                if payload_ty != Some(target.index()) {
                    return Err("Wasm: codec scalar identity mismatch".into());
                }
                self.enum_payload(source, index as u32, input)?
            };
            self.extend([I::LocalGet(value), I::Return, I::End]);
        }
        self.codec_decode_reject(&format!("expected {expected}"), input)?;
        Ok(self.local(ValType::I32))
    }

    pub(crate) fn codec_decode_reject(&mut self, message: &str, input: u32) -> Result<(), String> {
        let path = self.read32(0, 24);
        self.codec_decode_reject_at(message, input, path)
    }

    pub(crate) fn codec_decode_reject_at(
        &mut self,
        message: &str,
        input: u32,
        path: u32,
    ) -> Result<(), String> {
        let message = self.text_as(self.key.node, self.string_type()?, message.as_bytes())?;
        let message = self.parse_text(6, path, message)?;
        let error = self.read32(0, 28);
        self.extend([
            I::LocalGet(error),
            I::LocalGet(message),
            I::I32Store(memory(0, 2)),
            I::LocalGet(error),
            I::LocalGet(input),
            I::I32Store(memory(4, 2)),
        ]);
        self.extend([I::I32Const(0), I::Return]);
        Ok(())
    }

    pub(crate) fn codec_blame(
        &mut self,
        ty: TypeId,
        message: u32,
        input: u32,
    ) -> Result<u32, String> {
        let object = self.alloc(BLAME_SUBJECTS + LOC_BYTES);
        self.copy(object, 0, message, STRING_BYTES);
        self.store32(object, BLAME_COUNT as u64, 1);
        self.store32(object, BLAME_COUNT as u64 + 4, 0);
        self.copy(object, BLAME_SUBJECTS, input, LOC_BYTES);
        let id = self.table_push(BLAMES, object, BLAME_SUBJECTS + LOC_BYTES, None)?;
        let blame = self.value_as(self.key.node, ty, SCALAR_BYTES)?;
        self.extend([
            I::LocalGet(blame),
            I::LocalGet(id),
            I::I64ExtendI32U,
            I::I64Store(memory(DATA, 3)),
        ]);
        Ok(blame)
    }
}
