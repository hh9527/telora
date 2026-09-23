//! Explicit existential packing and exact projection; no inferred identities.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::{TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub fn dynamic_native(&mut self, name: &str) -> Result<u32, String> {
        if name == "from_dyn_fields" {
            return self.dynamic_construct();
        }
        if name == "field_raw" {
            return self.dynamic_named_field();
        }
        if name == "fields_raw" {
            return self.dynamic_fields();
        }
        if name == "array_items_raw" {
            return self.dynamic_array_items();
        }
        if name == "tuple_items_raw" {
            return self.dynamic_tuple_items();
        }
        if name == "kind" {
            return self.dynamic_value_kind();
        }
        if matches!(
            name,
            "get_variant_index" | "get_variant_payload" | "tag_raw" | "payload_raw"
        ) {
            return self.dynamic_variant(name);
        }
        if name == "get_field_value" {
            return self.dynamic_field_value();
        }
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        let output = *args.last().ok_or("Wasm: Dyn signature missing")?;
        let kind = |ty: TypeId| &self.mir.types[ty.index()].constructor;
        if name == "pack" {
            if args.len() != 3
                || kind(args[0]) != &T::TypeOf
                || self.mir.types[args[0].index()].arguments != [args[1]]
                || kind(output) != &T::Dyn
            {
                return Err("Wasm: Dyn pack signature mismatch".into());
            }
            let input = self.parameter(1);
            let width = self.width(args[1])?;
            if width == 0 {
                self.emit(I::Unreachable);
                return Ok(input);
            }
            let id = self.table_push(VALUES, input, width, Some(args[1]))?;
            let result = self.value_as(node, output, DYN_BYTES)?;
            self.store32(result, DATA, args[1].index() as u32);
            self.store32(result, DATA + 4, 1);
            self.extend([
                I::LocalGet(result),
                I::LocalGet(id),
                I::I64ExtendI32U,
                I::I64Store(memory(DATA + 8, 3)),
            ]);
            return Ok(result);
        }
        if name == "desc" {
            if args.len() != 2 || kind(args[0]) != &T::Dyn || kind(output) != &T::Type {
                return Err("Wasm: Dyn desc signature mismatch".into());
            }
            let input = self.parameter(0);
            let result = self.value_as(node, output, SCALAR_BYTES)?;
            self.extend([
                I::LocalGet(result),
                I::LocalGet(input),
                I::I32Load(memory(DATA, 2)),
                I::I64ExtendI32U,
                I::I64Store(memory(DATA, 3)),
            ]);
            return Ok(result);
        }
        if kind(output) != &T::Option || self.mir.types[output.index()].arguments.len() != 1 {
            return Err(format!("Wasm: Dyn native not implemented: {name}"));
        }
        let expected = self.mir.types[output.index()].arguments[0];
        let argument = if name == "project_with" {
            if args.len() != 3
                || kind(args[0]) != &T::TypeOf
                || kind(args[1]) != &T::Dyn
                || self.mir.types[args[0].index()].arguments != [expected]
            {
                return Err("Wasm: Dyn projection signature mismatch".into());
            }
            1
        } else {
            let scalar = match name {
                "check_int" => T::Int,
                "check_float" => T::Float,
                "check_string" => T::String,
                "check_bytes" => T::Bytes,
                _ => return Err(format!("Wasm: Dyn native not implemented: {name}")),
            };
            if args.len() != 2 || kind(args[0]) != &T::Dyn || *kind(expected) != scalar {
                return Err("Wasm: Dyn scalar projection signature mismatch".into());
            }
            0
        };
        let input = self.parameter(argument);
        let result = self.local(ValType::I32);
        self.extend([
            I::LocalGet(input),
            I::I32Load(memory(DATA, 2)),
            I::I32Const(expected.index() as i32),
            I::I32Eq,
            I::If(BlockType::Empty),
        ]);
        let value = self.table_data(VALUES, input, DATA + 8);
        let some = self.enum_value(node, output, 1, Some(value))?;
        self.extend([I::LocalGet(some), I::LocalSet(result), I::Else]);
        let none = self.enum_value(node, output, 0, None)?;
        self.extend([I::LocalGet(none), I::LocalSet(result), I::End]);
        Ok(result)
    }
}
