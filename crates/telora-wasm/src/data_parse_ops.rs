//! Format parsers return postorder plans, materialized into sealed Value layouts.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::{TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    fn parse_address(&mut self, base: u32, index: u32, stride: u32) -> u32 {
        let result = self.local(ValType::I32);
        self.extend([
            I::LocalGet(base),
            I::LocalGet(index),
            I::I32Const(stride as i32),
            I::I32Mul,
            I::I32Add,
            I::LocalSet(result),
        ]);
        result
    }
    fn parse_buffer(&mut self, count: u32, stride: u32) -> u32 {
        let result = self.local(ValType::I32);
        self.extend([
            I::LocalGet(count),
            I::I32Const(stride as i32),
            I::I32Mul,
            I::Call(ALLOC),
            I::LocalSet(result),
        ]);
        result
    }
    fn parse_column(&mut self, base: u32, count: u32, stride: u32) -> u32 {
        let id = self.local(ValType::I32);
        self.extend([
            I::I32Const(table_address(ARRAYS) as i32),
            I::LocalGet(base),
            I::LocalGet(count),
            I::I32Const(stride as i32),
            I::I32Mul,
            I::Call(TABLE_PUSH),
            I::LocalSet(id),
        ]);
        id
    }
    pub fn data_parse_native(&mut self, parser: u32) -> Result<u32, String> {
        let node = self.key.node;
        let args = self.mir.types[self.ty(node)?.index()].arguments.clone();
        if args.len() != 3
            || self.mir.types[args[0].index()].constructor != T::TypeOf
            || self.mir.types[args[1].index()].constructor != T::String
            || self.mir.types[args[2].index()].constructor != T::Result
        {
            return Err("Wasm: data parse signature mismatch".into());
        }
        let target = self.mir.types[args[0].index()].arguments[0];
        let results = self.mir.types[args[2].index()].arguments.clone();
        if results[0] != target {
            return Err("Wasm: data parse result mismatch".into());
        }
        let input = self.parameter(1);
        let packet = self.local(ValType::I32);
        self.extend([I::LocalGet(input), I::Call(parser), I::LocalSet(packet)]);
        let error = self.read32(packet, 12);
        self.extend([I::LocalGet(error), I::If(BlockType::Empty)]);
        let message = self.text_span_value(args[1], error)?;
        let blame = self.codec_blame(results[1], message, input)?;
        let rejected = self.enum_value(node, args[2], 0, Some(blame))?;
        self.extend([I::LocalGet(rejected), I::Return, I::End]);
        let value = self.materialize_data_plan(target, packet)?;
        self.enum_value(node, args[2], 1, Some(value))
    }

    pub(crate) fn materialize_data_plan(&mut self, target: TypeId, packet: u32) -> Result<u32, String> {
        let node = self.key.node;
        let rows = self.read32(packet, 0);
        let count = self.read32(packet, 4);
        let values = self.parse_buffer(count, 4);
        let index = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(index),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let row = self.parse_address(rows, index, 24);
        let kind = self.read32(row, 0);
        let value = self.local(ValType::I32);
        for (code, name) in [
            "None",
            "True",
            "False",
            "Int",
            "Float",
            "String",
            "Array",
            "Object",
            "LocalDate",
            "LocalTime",
            "LocalDateTime",
            "OffsetDateTime",
            "Bytes",
        ]
        .iter()
        .enumerate()
        {
            let variant = self.plan.layouts[target.index()]
                .variants
                .iter()
                .position(|v| v.name == *name)
                .ok_or("Wasm: incomplete Value parse contract")?;
            let payload_ty = self.plan.layouts[target.index()].variants[variant]
                .type_id
                .map(|id| self.plan.layouts[id].id());
            self.extend([
                I::LocalGet(kind),
                I::I32Const(code as i32),
                I::I32Eq,
                I::If(BlockType::Empty),
            ]);
            let payload = if let Some(ty) = payload_ty {
                let payload = match code {
                    3 | 4 => {
                        let p = self.value_as(node, ty, SCALAR_BYTES)?;
                        self.extend([
                            I::LocalGet(p),
                            I::LocalGet(row),
                            I::I64Load(memory(16, 3)),
                            I::I64Store(memory(DATA, 3)),
                        ]);
                        p
                    }
                    5 | 8..=11 => {
                        let span = self.local(ValType::I32);
                        self.extend([
                            I::LocalGet(row),
                            I::I32Const(16),
                            I::I32Add,
                            I::LocalSet(span),
                        ]);
                        self.text_span_value(ty, span)?
                    }
                    6 | 7 => self.parse_collection(ty, target, row, values, code == 7)?,
                    12 => {
                        let pointer = self.read32(row, 16);
                        let count = self.read32(row, 20);
                        let id = self.local(ValType::I32);
                        self.extend([
                            I::I32Const(table_address(BYTES) as i32),
                            I::LocalGet(pointer),
                            I::LocalGet(count),
                            I::Call(TABLE_PUSH),
                            I::LocalSet(id),
                        ]);
                        let value = self.value_as(node, ty, STRING_BYTES)?;
                        for (offset, local) in [(DATA, id), (DATA + 8, count)] {
                            self.extend([
                                I::LocalGet(value),
                                I::LocalGet(local),
                                I::I32Store(memory(offset, 2)),
                            ]);
                        }
                        self.store32(value, DATA + 4, 0);
                        self.store32(value, DATA + 12, 0);
                        value
                    }
                    _ => return Err("Wasm: unexpected payload in Value contract".into()),
                };
                self.parse_location(payload, row, 4);
                Some(payload)
            } else {
                None
            };
            let item = self.enum_value(node, target, variant as u32, payload)?;
            self.parse_location(item, row, 4);
            self.extend([I::LocalGet(item), I::LocalSet(value), I::End]);
        }
        let slot = self.parse_address(values, index, 4);
        self.extend([
            I::LocalGet(slot),
            I::LocalGet(value),
            I::I32Store(memory(0, 2)),
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(index),
            I::Br(0),
            I::End,
            I::End,
        ]);
        let root = self.read32(packet, 8);
        let slot = self.parse_address(values, root, 4);
        Ok(self.read32(slot, 0))
    }
    fn parse_location(&mut self, value: u32, record: u32, offset: u64) {
        for field in [0, 4, 8] {
            self.extend([I::LocalGet(value), I::LocalGet(record),
                I::I32Load(memory(offset + field, 2)), I::I32Store(memory(field, 2))]);
        }
    }
    fn parse_collection(
        &mut self,
        ty: TypeId,
        target: TypeId,
        row: u32,
        values: u32,
        object: bool,
    ) -> Result<u32, String> {
        let count = self.read32(row, 20);
        let entries = self.read32(row, 16);
        let width = self.width(target)?;
        let data = self.parse_buffer(count, width);
        let keys = if object {
            Some(self.parse_buffer(count, STRING_BYTES))
        } else {
            None
        };
        let index = self.local(ValType::I32);
        self.extend([
            I::I32Const(0),
            I::LocalSet(index),
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(index),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let entry = self.parse_address(entries, index, if object { 24 } else { 4 });
        let child = self.read32(entry, if object { 8 } else { 0 });
        let slot = self.parse_address(values, child, 4);
        let value = self.read32(slot, 0);
        let destination = self.parse_address(data, index, width);
        self.copy(destination, 0, value, width);
        if let Some(keys) = keys {
            let key = self.text_span_value(self.string_type()?, entry)?;
            self.parse_location(key, entry, 12);
            let destination = self.parse_address(keys, index, STRING_BYTES);
            self.copy(destination, 0, key, STRING_BYTES);
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
        let id = self.parse_column(data, count, width);
        let output = self.value_as(self.key.node, ty, STRING_BYTES)?;
        if let Some(keys) = keys {
            let key_id = self.parse_column(keys, count, STRING_BYTES);
            for (offset, local) in [(DATA, key_id), (DATA + 4, count), (DATA + 8, id)] {
                self.extend([
                    I::LocalGet(output),
                    I::LocalGet(local),
                    I::I32Store(memory(offset, 2)),
                ]);
            }
        } else {
            self.extend([
                I::LocalGet(output),
                I::LocalGet(id),
                I::I32Store(memory(DATA, 2)),
                I::LocalGet(output),
                I::LocalGet(count),
                I::I32Store(memory(DATA + 8, 2)),
            ]);
            self.store32(output, DATA + 4, 0);
        }
        self.store32(output, DATA + 12, 0);
        Ok(output)
    }
}


/// The same sealed Value construction used by std parsers and external inputs.
/// Parameters are (parse packet, reserved); no runtime type selection occurs.
pub(crate) fn materializer(
    mir: &telora_core::mir::Mir,
    plan: &crate::plan::Plan,
    target: Option<u32>,
) -> Result<crate::object::ObjectFunction, String> {
    let mut emit = Emitter::new(mir, plan, crate::plan::Key { callable: true, ..plan.root });
    if let Some(target) = target {
        let error = emit.read32(0, 12);
        emit.extend([I::LocalGet(error), I::If(BlockType::Empty), I::Unreachable, I::End]);
        let value = emit.materialize_data_plan(plan.layouts[target as usize].id(), 0)?;
        emit.emit(I::LocalGet(value));
    } else {
        emit.emit(I::Unreachable);
    }
    Ok(emit.finish())
}
