use crate::{abi::*, emit::Emitter};
use telora_core::mir::{HirId, TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn dict_result(
        &mut self,
        ty: TypeId,
        keys: u32,
        values: u32,
        count: u32,
        width: u32,
    ) -> Result<u32, String> {
        self.dict_result_at(self.key.node, ty, keys, values, count, width)
    }
    pub(crate) fn dict_result_at(
        &mut self,
        node: HirId,
        ty: TypeId,
        keys: u32,
        values: u32,
        count: u32,
        width: u32,
    ) -> Result<u32, String> {
        let bytes = self.local(ValType::I32);
        self.extend([I::LocalGet(count), I::I32Const(STRING_BYTES as i32), I::I32Mul, I::LocalSet(bytes)]);
        let key_id = self.array_object(keys, bytes, self.string_type()?)?;
        self.extend([I::LocalGet(count), I::I32Const(width as i32), I::I32Mul, I::LocalSet(bytes)]);
        let element = self.mir.types[ty.index()].arguments[0];
        let value_id = self.array_object(values, bytes, element)?;
        let result = self.value_as(node, ty, STRING_BYTES)?;
        for (offset, value) in [(DATA, key_id), (DATA + 4, count), (DATA + 8, value_id)] {
            self.extend([
                I::LocalGet(result),
                I::LocalGet(value),
                I::I32Store(memory(offset, 2)),
            ]);
        }
        self.store32(result, DATA + 12, 0);
        Ok(result)
    }
    pub fn dict_native(&mut self, name: &str) -> Result<u32, String> {
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        if name == "from_pairs" {
            return self.dict_from_pairs(&args);
        }
        if args.len() < 2 || self.mir.types[args[0].index()].constructor != T::Dict {
            return Err("Wasm: Dict native signature mismatch".into());
        }
        let output = *args.last().unwrap();
        let element = self.mir.types[args[0].index()].arguments[0];
        let width = self.width(element)?;
        let value = self.parameter(0);
        let count = self.local(ValType::I32);
        self.extend([
            I::LocalGet(value),
            I::I32Load(memory(DATA + 4, 2)),
            I::LocalSet(count),
        ]);
        if matches!(name, "keys" | "values") {
            if args.len() != 2 {
                return Err("Wasm: Dict column arity mismatch".into());
            }
            let expected = if name == "keys" {
                self.string_type()?
            } else {
                element
            };
            if self.mir.types[output.index()].constructor != T::Array
                || self.mir.types[output.index()].arguments != [expected]
            {
                return Err("Wasm: Dict column type mismatch".into());
            }
            let result = self.value_as(node, output, STRING_BYTES)?;
            self.extend([
                I::LocalGet(result),
                I::LocalGet(value),
                I::I32Load(memory(if name == "keys" { DATA } else { DATA + 8 }, 2)),
                I::I32Store(memory(DATA, 2)),
                I::LocalGet(result),
                I::LocalGet(count),
                I::I32Store(memory(DATA + 8, 2)),
            ]);
            return Ok(result);
        }
        if name == "get" {
            if args.len() != 3
                || self.mir.types[args[1].index()].constructor != T::String
                || self.mir.types[output.index()].constructor != T::Option
                || self.mir.types[output.index()].arguments != [element]
            {
                return Err("Wasm: Dict get signature mismatch".into());
            }
            let key = self.parameter(1);
            let found = self.dictionary_lookup(value, key, width);
            self.extend([I::LocalGet(found), I::I32Eqz, I::If(BlockType::Empty)]);
            let none = self.enum_value(node, output, 0, None)?;
            self.extend([I::LocalGet(none), I::Return, I::End]);
            return self.enum_value(node, output, 1, Some(found));
        }
        if name == "merge" {
            if args.len() != 3 || args[0] != args[1] || args[0] != args[2] {
                return Err("Wasm: Dict merge signature mismatch".into());
            }
            let right = self.parameter(1);
            return self.dict_merge_values(node, output, value, right);
        }
        match name {
            "pairs" => {
                if args.len() != 2 || self.mir.types[output.index()].constructor != T::Array {
                    return Err("Wasm: Dict pairs result mismatch".into());
                }
                let pair = &self.mir.types[self.mir.types[output.index()].arguments[0].index()];
                if pair.constructor != T::Tuple || pair.arguments != [self.string_type()?, element]
                {
                    return Err("Wasm: Dict pair fields mismatch".into());
                }
            }
            "map_values" | "filter" => {
                if args.len() != 3 || self.mir.types[output.index()].constructor != T::Dict {
                    return Err("Wasm: Dict callback result mismatch".into());
                }
                let callback = &self.mir.types[args[1].index()];
                if callback.constructor != T::Function
                    || callback.arguments.len() != 2
                    || callback.arguments[0] != element
                {
                    return Err("Wasm: Dict callback input mismatch".into());
                }
                let target = self.mir.types[output.index()].arguments[0];
                if (name == "filter"
                    && (target != element
                        || self.mir.types[callback.arguments[1].index()].constructor != T::Bool))
                    || (name == "map_values" && callback.arguments[1] != target)
                {
                    return Err("Wasm: Dict callback output mismatch".into());
                }
            }
            "fold" => {
                if args.len() != 4 || args[1] != output {
                    return Err("Wasm: Dict fold accumulator mismatch".into());
                }
                let callback = &self.mir.types[args[2].index()];
                if callback.constructor != T::Function
                    || callback.arguments != [output, self.string_type()?, element, output]
                {
                    return Err("Wasm: Dict fold callback mismatch".into());
                }
            }
            _ => return Err(format!("Wasm: unsupported Dict native {name}")),
        }
        let keys = self.table_data(ARRAYS, value, DATA);
        let values = self.table_data(ARRAYS, value, DATA + 8);
        let output_element = if name == "fold" {
            output
        } else {
            self.mir.types[output.index()].arguments[0]
        };
        let out_width = self.width(output_element)?;
        let callback = if name == "pairs" {
            None
        } else {
            Some(self.parameter(if name == "fold" { 2 } else { 1 }))
        };
        let accumulator = if name == "fold" {
            self.parameter(1)
        } else {
            self.local(ValType::I32)
        };
        let out_values = if name == "fold" {
            self.local(ValType::I32)
        } else {
            self.array_storage(count, out_width)
        };
        let out_keys = if name == "filter" {
            self.array_storage(count, STRING_BYTES)
        } else {
            keys
        };
        let used = self.local(ValType::I32);
        let index = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(index),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let key = self.array_item(keys, index, STRING_BYTES);
        let item = self.array_item(values, index, width);
        let mapped = match name {
            "pairs" => self.packed_tuple(output_element, &[key, item])?,
            "map_values" | "filter" => self.invoke(callback.unwrap(), &[item])?,
            "fold" => self.invoke(callback.unwrap(), &[accumulator, key, item])?,
            _ => return Err(format!("Wasm: unsupported Dict native {name}")),
        };
        if name == "filter" {
            self.bits(mapped);
            self.extend([I::I64Eqz, I::I32Eqz, I::If(BlockType::Empty)]);
            let to_key = self.array_item(out_keys, used, STRING_BYTES);
            let to_value = self.array_item(out_values, used, width);
            self.copy(to_key, 0, key, STRING_BYTES);
            self.copy(to_value, 0, item, width);
            self.extend([
                I::LocalGet(used),
                I::I32Const(1),
                I::I32Add,
                I::LocalSet(used),
                I::End,
            ]);
        } else if name == "fold" {
            self.extend([I::LocalGet(mapped), I::LocalSet(accumulator)]);
        } else {
            let destination = self.array_item(out_values, index, out_width);
            self.copy(destination, 0, mapped, out_width);
        }
        self.extend([
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(index),
            I::Br(0),
            I::End,
            I::End,
        ]);
        match name {
            "fold" => Ok(accumulator),
            "pairs" => self.array_result(output, out_values, count, out_width),
            "filter" => self.dict_result(output, out_keys, out_values, used, width),
            "map_values" => self.dict_result(output, keys, out_values, count, out_width),
            _ => unreachable!(),
        }
    }
}
