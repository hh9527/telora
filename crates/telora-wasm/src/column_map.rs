//! Typed wrappers for immutable column maps. Private mutation stays inside RT.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::TypeConstructor as T;
use telora_wasm_shared::column_map::Request;
use wasm_encoder::{BlockType, Instruction as I, ValType};
impl Emitter<'_> {
    pub(crate) fn column_map_native(&mut self, name: &str) -> Result<u32, String> {
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        let output = *args.last().ok_or("Map signature")?;
        let map = if name == "rt_from_pairs" {
            output
        } else {
            args[0]
        };
        if self.mir.types[map.index()].constructor != T::Dict {
            return Err("Map signature mismatch".into());
        }
        let value = self.mir.types[map.index()].arguments[0];
        let key = self.mir.types[map.index()].arguments[1];
        let input = self.parameter(0);
        if name == "length" {
            let out = self.value_as(node, output, SCALAR_BYTES)?;
            self.extend([
                I::LocalGet(out),
                I::LocalGet(input),
                I::I32Load(memory(DATA + 4, 2)),
                I::I64ExtendI32U,
                I::I64Store(memory(DATA, 3)),
            ]);
            return Ok(out);
        }
        if matches!(name, "keys" | "values") {
            let out = self.value_as(node, output, STRING_BYTES)?;
            self.extend([
                I::LocalGet(input),
                I::I32Load(memory(DATA + 4, 2)),
                I::If(BlockType::Empty),
            ]);
            let column = self.read32(input, if name == "keys" { DATA } else { DATA + 8 });
            let count = self.read32(input, DATA + 4);
            self.extend([
                I::LocalGet(out),
                I::LocalGet(column),
                I::I32Store(memory(DATA, 2)),
                I::LocalGet(out),
                I::LocalGet(count),
                I::I32Store(memory(DATA + 8, 2)),
            ]);
            self.store32(out, DATA + 12, ARRAY_SLICE);
            self.emit(I::End);
            return Ok(out);
        }
        let op = match name {
            "rt_from_pairs" => 20,
            "rt_get" => 21,
            "rt_insert" => 22,
            "rt_remove" => 23,
            "rt_merge" => 24,
            "pairs" => 25,
            _ => return Err(format!("unknown Map native {name}")),
        };
        let mut d = Request {
            operation: op,
            key_type: key.index() as u32,
            key_width: self.width(key)?,
            value_type: value.index() as u32,
            value_width: self.width(value)?,
            ..Request::default()
        };
        if op == 20 || op == 25 {
            let array = if op == 20 { args[0] } else { output };
            let pair = self.mir.types[array.index()].arguments[0];
            if self.width(pair)? == 0 && op == 20 {
                let zero = self.local(ValType::I32);
                return self.column_map_result(output, zero, zero, zero, key, value);
            }
            let layout = self.plan.layouts[pair.index()]
                .object
                .as_ref()
                .ok_or("Map pair layout")?;
            d.pair_type = pair.index() as u32;
            d.pair_width = self.width(pair)?;
            d.pair_bytes = layout.bytes.unwrap_or(0) as u32;
            d.key_offset = layout.members[0].offset.unwrap_or(0) as u32;
            d.value_offset = layout.members[1].offset.unwrap_or(0) as u32;
            d.pair_inline = u32::from(self.inline_fields(pair));
        }
        let request = self.scratch_alloc(Request::BYTES, true);
        for (i, word) in d.words().into_iter().enumerate() {
            self.store32(request, i as u64 * 4, word)
        }
        let mut params = vec![(4, input)];
        if op != 20 && op != 25 {
            params.push((8, self.parameter(1)));
            if op == 22 {
                params.push((12, self.parameter(2)));
            }
        }
        if op != 25 {
            let c = if op == 20 {
                1
            } else if op == 22 {
                3
            } else {
                2
            };
            params.push((16, self.parameter(c)));
            let order = self.parameter(c + 1);
            self.extend([
                I::LocalGet(request),
                I::LocalGet(order),
                I::I32Load(memory(DATA, 2)),
                I::I32Store(memory(20, 2)),
            ]);
        }
        for (offset, local) in params {
            self.extend([
                I::LocalGet(request),
                I::LocalGet(local),
                I::I32Store(memory(offset, 2)),
            ])
        }
        let result = self.local(ValType::I32);
        self.extend([
            I::LocalGet(request),
            I::Call(COLUMN_MAP),
            I::LocalSet(result),
        ]);
        self.root_value(result, if op == 21 { value } else { output });
        self.extend([
            I::LocalGet(request),
            I::I32Load(memory(Request::STATUS as u64, 2)),
            I::I32Const(-1),
            I::I32Eq,
            I::If(BlockType::Empty),
            I::I32Const(0),
            I::Return,
            I::End,
        ]);
        if op == 21 {
            let out = self.local(ValType::I32);
            self.extend([I::LocalGet(result), I::If(BlockType::Empty)]);
            let some = self.enum_value(node, output, 1, Some(result))?;
            self.extend([I::LocalGet(some), I::LocalSet(out), I::Else]);
            let none = self.enum_value(node, output, 0, None)?;
            self.extend([I::LocalGet(none), I::LocalSet(out), I::End]);
            return Ok(out);
        }
        Ok(result)
    }
}
