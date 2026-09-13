use crate::{abi::*, emit::Emitter};
use telora_core::mir::TypeId;
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    fn packed_tuple(&mut self, ty: TypeId, fields: &[u32]) -> Result<u32, String> {
        let layout = self.plan.layouts[ty.index()]
            .object
            .as_ref()
            .ok_or("Wasm: tuple layout missing")?;
        if fields.len() != layout.members.len() {
            return Err("Wasm: tuple arity differs from sealed layout".into());
        }
        let bytes = u32::try_from(layout.bytes.ok_or("Wasm: tuple size missing")?)
            .map_err(|_| "Wasm: tuple size overflow")?;
        let object = self.alloc(bytes);
        for (field, member) in fields.iter().zip(&layout.members) {
            let ty =
                self.plan.layouts[member.type_id.ok_or("Wasm: tuple field type missing")?].id();
            self.copy(
                object,
                member.offset.ok_or("Wasm: tuple field offset missing")? as u32,
                *field,
                self.width(ty)?,
            );
        }
        let id = self.table_push(RECORDS, object, bytes);
        let result = self.value_as(self.key.node, ty, self.width(ty)?)?;
        self.extend([
            I::LocalGet(result),
            I::LocalGet(id),
            I::I64ExtendI32U,
            I::I64Store(memory(DATA, 3)),
        ]);
        Ok(result)
    }
    pub fn array_build(
        &mut self,
        name: &str,
        args: &[TypeId],
        base: u32,
        count: u32,
        width: u32,
    ) -> Result<u32, String> {
        let node = self.key.node;
        let output = *args.last().unwrap();
        if matches!(name, "concat" | "flat_map") {
            return self.array_flatten(name, args, base, count, width);
        }
        if name == "push" {
            let length = self.local(ValType::I32);
            self.extend([
                I::LocalGet(count),
                I::I32Const(1),
                I::I32Add,
                I::LocalTee(length),
                I::I32Eqz,
            ]);
            self.fail_if(node, ERROR_OVERFLOW);
            let data = self.array_storage(length, width);
            self.extend([
                I::LocalGet(data),
                I::LocalGet(base),
                I::LocalGet(count),
                I::I32Const(width as i32),
                I::I32Mul,
                I::MemoryCopy {
                    src_mem: 0,
                    dst_mem: 0,
                },
            ]);
            let last = self.array_item(data, count, width);
            let value = self.parameter(1);
            self.copy(last, 0, value, width);
            return self.array_result(output, data, length, width);
        }
        let array_ty = if name == "zip" {
            self.mir.types[output.index()].arguments[0]
        } else {
            output
        };
        let tuple_ty = self.mir.types[array_ty.index()].arguments[0];
        let tuple_fields = &self.mir.types[tuple_ty.index()].arguments;
        let second = if name == "zip" {
            let value = self.parameter(1);
            let other_width = self.width(self.mir.types[args[1].index()].arguments[0])?;
            let (other, length) = self.array_parts(value, other_width);
            self.extend([
                I::LocalGet(count),
                I::LocalGet(length),
                I::I32Ne,
                I::If(BlockType::Empty),
            ]);
            let none = self.enum_value(node, output, 0, None)?;
            self.extend([I::LocalGet(none), I::Return, I::End]);
            Some((other, other_width))
        } else {
            None
        };
        let tuple_width = self.width(tuple_ty)?;
        let data = self.array_storage(count, tuple_width);
        let index = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(index),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let item = self.array_item(base, index, width);
        let fields = if let Some((other, other_width)) = second {
            vec![item, self.array_item(other, index, other_width)]
        } else {
            let number = self.scalar_as(node, tuple_fields[0], 0)?;
            self.extend([
                I::LocalGet(number),
                I::LocalGet(index),
                I::I64ExtendI32U,
                I::I64Store(memory(DATA, 3)),
            ]);
            vec![number, item]
        };
        let tuple = self.packed_tuple(tuple_ty, &fields)?;
        let target = self.array_item(data, index, tuple_width);
        self.copy(target, 0, tuple, tuple_width);
        self.extend([
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(index),
            I::Br(0),
            I::End,
            I::End,
        ]);
        let array = self.array_result(array_ty, data, count, tuple_width)?;
        if name == "zip" {
            self.enum_value(node, output, 1, Some(array))
        } else {
            Ok(array)
        }
    }
    fn array_flatten(
        &mut self,
        name: &str,
        args: &[TypeId],
        base: u32,
        count: u32,
        width: u32,
    ) -> Result<u32, String> {
        let output = *args.last().unwrap();
        let element = self.mir.types[output.index()].arguments[0];
        let stride = self.width(element)?;
        let callback = (name == "flat_map").then(|| self.parameter(1));
        // Evaluate callbacks once; retain only their array pointers during planning.
        let arrays = self.array_storage(count, 4);
        let total = self.local(ValType::I32);
        let index = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(index),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let item = self.array_item(base, index, width);
        let array = if let Some(callback) = callback {
            self.invoke(callback, &[item])?
        } else {
            item
        };
        let (_, length) = self.array_parts(array, stride);
        let next = self.local(ValType::I32);
        self.extend([
            I::LocalGet(total),
            I::LocalGet(length),
            I::I32Add,
            I::LocalTee(next),
            I::LocalGet(total),
            I::I32LtU,
        ]);
        self.fail_if(self.key.node, ERROR_OVERFLOW);
        self.extend([I::LocalGet(next), I::LocalSet(total)]);
        let target = self.array_item(arrays, index, 4);
        self.extend([
            I::LocalGet(target),
            I::LocalGet(array),
            I::I32Store(memory(0, 2)),
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(index),
            I::Br(0),
            I::End,
            I::End,
        ]);
        let data = self.array_storage(total, stride);
        let written = self.local(ValType::I32);
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
        let pointer = self.array_item(arrays, index, 4);
        let array = self.local(ValType::I32);
        self.extend([
            I::LocalGet(pointer),
            I::I32Load(memory(0, 2)),
            I::LocalSet(array),
        ]);
        let (source, length) = self.array_parts(array, stride);
        let target = self.array_item(data, written, stride);
        self.extend([
            I::LocalGet(target),
            I::LocalGet(source),
            I::LocalGet(length),
            I::I32Const(stride as i32),
            I::I32Mul,
            I::MemoryCopy {
                src_mem: 0,
                dst_mem: 0,
            },
            I::LocalGet(written),
            I::LocalGet(length),
            I::I32Add,
            I::LocalSet(written),
            I::LocalGet(index),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(index),
            I::Br(0),
            I::End,
            I::End,
        ]);
        self.array_result(output, data, total, stride)
    }
}
