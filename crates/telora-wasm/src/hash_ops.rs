use crate::{abi::*, emit::Emitter};
use telora_core::mir::TypeConstructor as T;
use wasm_encoder::{Instruction as I, ValType};
impl Emitter<'_> {
    pub fn hash_native(&mut self, name: &str) -> Result<u32, String> {
        let args = self.mir.types[self.ty(self.key.node)?.index()]
            .arguments
            .clone();
        let operation = [
            "sha256",
            "new",
            "update_bytes",
            "update_string",
            "update_int",
            "finish",
        ]
        .iter()
        .position(|&n| n == name)
        .ok_or("Wasm: unknown hash operation")?;
        let arity = match operation {
            1 => 0,
            0 | 5 => 1,
            _ => 2,
        };
        if args.len() != arity + 1 {
            return Err("Wasm: hash arity mismatch".into());
        }
        let output = args[arity];
        let hash = |ty: telora_core::mir::TypeId| matches!(self.mir.types[ty.index()].constructor, T::Native(id) if (id.module,id.slot)==(16,3));
        if (operation == 0
            && (self.mir.types[args[0].index()].constructor != T::String
                || self.mir.types[output.index()].constructor != T::String))
            || (operation == 1 && !hash(output))
            || (operation >= 2 && !hash(args[0]))
            || ((2..=4).contains(&operation)
                && (!hash(output)
                    || self.mir.types[args[1].index()].constructor
                        != match operation {
                            2 => T::Bytes,
                            3 => T::String,
                            _ => T::Int,
                        }))
            || (operation == 5 && self.mir.types[output.index()].constructor != T::Bytes)
        {
            return Err("Wasm: hash signature mismatch".into());
        }
        let a = if arity == 0 {
            let zero = self.local(ValType::I32);
            zero
        } else {
            self.parameter(0)
        };
        let first = if operation >= 2 {
            self.read32(a, DATA)
        } else {
            a
        };
        let b = if arity == 2 {
            self.parameter(1)
        } else {
            self.local(ValType::I32)
        };
        let result = self.local(ValType::I32);
        self.extend([
            I::I32Const(operation as i32),
            I::LocalGet(first),
            I::LocalGet(b),
            I::Call(HASH),
            I::LocalSet(result),
        ]);
        if operation == 0 {
            return self.text_span_value(output, result);
        }
        if operation == 5 {
            let id = self.table_push(BYTES, result, 32);
            let value = self.value_as(self.key.node, output, STRING_BYTES)?;
            self.extend([
                I::LocalGet(value),
                I::LocalGet(id),
                I::I32Store(memory(DATA, 2)),
            ]);
            self.store32(value, DATA + 4, 0);
            self.store32(value, DATA + 8, 32);
            self.store32(value, DATA + 12, 0);
            return Ok(value);
        }
        let value = self.value_as(self.key.node, output, SCALAR_BYTES)?;
        self.extend([
            I::LocalGet(value),
            I::LocalGet(result),
            I::I64ExtendI32U,
            I::I64Store(memory(DATA, 3)),
        ]);
        Ok(value)
    }
}
