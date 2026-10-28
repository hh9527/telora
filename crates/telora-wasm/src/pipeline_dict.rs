//! Private MapMut columns and a sorted integer index. No state escapes its scope.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::TypeId;
use wasm_encoder::{BlockType, Instruction as I, ValType};
pub(super) struct Layout {
    state: TypeId,
    keys: TypeId,
    values: TypeId,
    index: TypeId,
    key: TypeId,
    value: TypeId,
    boolean: TypeId,
}
impl Emitter<'_> {
    pub(super) fn pipeline_layout(&self, builder: TypeId) -> Result<Layout, String> {
        let state = self.mir.types[builder.index()].arguments[0];
        let keys = self.builder_field_type(state, "0")?;
        let values = self.builder_field_type(state, "1")?;
        let index = self.builder_field_type(state, "2")?;
        Ok(Layout {
            state,
            keys,
            values,
            index,
            key: self.mir.types[keys.index()].arguments[0],
            value: self.mir.types[values.index()].arguments[0],
            boolean: self.builder_field_type(state, "3")?,
        })
    }
    fn map_mut_column_copy(
        &mut self,
        initial: u32,
        map: TypeId,
        col: TypeId,
        element: TypeId,
        offset: u64,
    ) -> Result<u32, String> {
        let source = self.table_data(ARRAYS, initial, offset);
        let count = self.read32(initial, DATA + 4);
        let width = self.width(element)?;
        let data = self.array_storage(count, width);
        self.extend([
            I::LocalGet(data),
            I::LocalGet(source),
            I::LocalGet(count),
            I::I32Const(width as i32),
            I::I32Mul,
            I::MemoryCopy {
                src_mem: 0,
                dst_mem: 0,
            },
        ]);
        let _ = map;
        self.array_result(col, data, count, width)
    }
    pub(super) fn pipeline_new(
        &mut self,
        args: &[TypeId],
        l: &Layout,
        initial: u32,
    ) -> Result<u32, String> {
        let keys = self.map_mut_column_copy(initial, args[0], l.keys, l.key, DATA)?;
        self.root_value(keys, l.keys);
        let values = self.map_mut_column_copy(initial, args[0], l.values, l.value, DATA + 8)?;
        self.root_value(values, l.values);
        let count = self.read32(initial, DATA + 4);
        let capacity = self.local(ValType::I32);
        self.extend([
            I::LocalGet(count),
            I::I32Const(4),
            I::LocalGet(count),
            I::I32Const(4),
            I::I32GeU,
            I::Select,
            I::LocalSet(capacity),
        ]);
        let bytes = self.local(ValType::I32);
        self.extend([
            I::LocalGet(capacity),
            I::I32Const(4),
            I::I32Mul,
            I::LocalSet(bytes),
        ]);
        let data = self.array_storage(capacity, 4);
        let i = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(i),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let at = self.array_item(data, i, 4);
        self.extend([
            I::LocalGet(at),
            I::LocalGet(i),
            I::I32Store(memory(0, 2)),
            I::LocalGet(i),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(i),
            I::Br(0),
            I::End,
            I::End,
        ]);
        let index = self.byte_span_value(l.index, data, bytes)?;
        self.root_value(index, l.index);
        let sealed = self.scalar_as(self.key.node, l.boolean, 0)?;
        let mut fields = Vec::new();
        for m in &self.plan.layouts[l.state.index()]
            .object
            .as_ref()
            .unwrap()
            .members
        {
            fields.push(match m.name.as_str() {
                "0" => keys,
                "1" => values,
                "2" => index,
                "3" => sealed,
                _ => return Err("invalid MapMut field".into()),
            });
        }
        let state = self.packed_tuple(l.state, &fields)?;
        self.root_value(state, l.state);
        let one = self.local(ValType::I32);
        self.extend([I::I32Const(1), I::LocalSet(one)]);
        let outer = self.array_storage(one, self.width(l.state)?);
        self.copy(outer, 0, state, self.width(l.state)?);
        self.array_result(args[1], outer, one, self.width(l.state)?)
    }
    fn map_mut_append(
        &mut self,
        array: u32,
        ty: TypeId,
        element: TypeId,
        item: u32,
    ) -> Result<(), String> {
        let (data, count) = self.array_parts(array, self.width(element)?);
        let capacity = self.local(ValType::I32);
        self.extend([
            I::LocalGet(count),
            I::LocalSet(capacity),
            I::LocalGet(array),
            I::I32Load(memory(DATA + 12, 2)),
            I::I32Const(ARRAY_SLICE as i32),
            I::I32Eq,
            I::If(BlockType::Empty),
        ]);
        let object = self.read32(array, DATA);
        let stored = self.read32(object, 12);
        self.extend([I::LocalGet(stored), I::LocalSet(capacity), I::End]);
        self.sequence_append(data, count, capacity, element, item)?;
        let grown =
            self.array_result_with_capacity(ty, data, count, capacity, self.width(element)?)?;
        self.copy(array, 0, grown, self.width(ty)?);
        Ok(())
    }
    pub(super) fn pipeline_dict_native(
        &mut self,
        name: &str,
        args: &[TypeId],
        builder: u32,
    ) -> Result<u32, String> {
        let output = *args.last().unwrap();
        let l = self.pipeline_layout(args[0])?;
        let (state, _) = self.array_parts(builder, self.width(l.state)?);
        let (keys, _) = self.sequence_field(state, l.state, "0")?;
        let (values, _) = self.sequence_field(state, l.state, "1")?;
        let (index, _) = self.sequence_field(state, l.state, "2")?;
        let (sealed, _) = self.sequence_field(state, l.state, "3")?;
        self.bits(sealed);
        self.extend([I::I64Eqz, I::I32Eqz]);
        self.fail_if(self.key.node, ERROR_UNINITIALIZED_CALL);
        if name == "dict_finish" {
            self.store64(sealed, DATA, 1);
            let (kdata, count) = self.array_parts(keys, self.width(l.key)?);
            let (vdata, _) = self.array_parts(values, self.width(l.value)?);
            let (idata, _) = self.map_mut_index_parts(index);
            let kout = self.array_storage(count, self.width(l.key)?);
            let vout = self.array_storage(count, self.width(l.value)?);
            let i = self.local(ValType::I32);
            self.extend([
                I::Block(BlockType::Empty),
                I::Loop(BlockType::Empty),
                I::LocalGet(i),
                I::LocalGet(count),
                I::I32GeU,
                I::BrIf(1),
            ]);
            let idx = self.array_item(idata, i, 4);
            let pos = self.read32(idx, 0);
            let k = self.array_item(kdata, pos, self.width(l.key)?);
            let v = self.array_item(vdata, pos, self.width(l.value)?);
            let dst = self.array_item(kout, i, self.width(l.key)?);
            self.copy(dst, 0, k, self.width(l.key)?);
            let dst = self.array_item(vout, i, self.width(l.value)?);
            self.copy(dst, 0, v, self.width(l.value)?);
            self.extend([
                I::LocalGet(i),
                I::I32Const(1),
                I::I32Add,
                I::LocalSet(i),
                I::Br(0),
                I::End,
                I::End,
            ]);
            return self.column_map_result(output, kout, vout, count, l.key, l.value);
        }
        let edit = self.parameter(1);
        let edit_ty = args[1];
        let variants = &self.plan.layouts[edit_ty.index()].variants;
        let thunk_pair = self.plan.layouts[variants[0].type_id.unwrap()].id();
        let update_pair = self.plan.layouts[variants[1].type_id.unwrap()].id();
        let tag = self.read32(edit, DATA);
        let key = self.local(ValType::I32);
        let payload = self.local(ValType::I32);
        self.extend([I::LocalGet(tag), I::If(BlockType::Empty)]);
        for (which, ty) in [(1, update_pair), (0, thunk_pair)] {
            if which == 0 {
                self.emit(I::Else)
            }
            let p = self.enum_payload(edit_ty, which, edit)?;
            let k = self.projection_value_type(ty, p, 0)?;
            let v = self.projection_value_type(ty, p, 1)?;
            self.extend([
                I::LocalGet(k),
                I::LocalSet(key),
                I::LocalGet(v),
                I::LocalSet(payload),
            ]);
        }
        self.emit(I::End);
        let (kdata, count) = self.array_parts(keys, self.width(l.key)?);
        let (idata, _) = self.map_mut_index_parts(index);
        let (at, found) = self.column_lower(kdata, Some((idata, 4)), count, key, l.key)?;
        self.extend([
            I::LocalGet(found),
            I::LocalGet(tag),
            I::I32Eqz,
            I::I32And,
            I::If(BlockType::Empty),
        ]);
        let yes = self.scalar_as(self.key.node, output, 1)?;
        self.extend([I::LocalGet(yes), I::Return, I::End]);
        let value = self.local(ValType::I32);
        self.extend([
            I::LocalGet(tag),
            I::If(BlockType::Empty),
            I::LocalGet(payload),
            I::LocalSet(value),
            I::Else,
        ]);
        let computed = self.invoke(payload, &[], l.value)?;
        self.extend([I::LocalGet(computed), I::LocalSet(value), I::End]);
        self.root_value(value, l.value);
        self.extend([I::LocalGet(found), I::If(BlockType::Empty)]);
        let idx = self.array_item(idata, at, 4);
        let pos = self.read32(idx, 0);
        let (vdata, _) = self.array_parts(values, self.width(l.value)?);
        let target = self.array_item(vdata, pos, self.width(l.value)?);
        self.copy(target, 0, value, self.width(l.value)?);
        self.emit(I::Else);
        self.map_mut_append(keys, l.keys, l.key, key)?;
        self.map_mut_append(values, l.values, l.value, value)?;
        let (old_data, capacity) = self.map_mut_index_parts(index);
        self.extend([
            I::LocalGet(count),
            I::LocalGet(capacity),
            I::I32GeU,
            I::If(BlockType::Empty),
        ]);
        self.extend([
            I::LocalGet(capacity),
            I::I32Const((u32::MAX / 8) as i32),
            I::I32GtU,
        ]);
        self.fail_if(self.key.node, ERROR_OVERFLOW);
        let next = self.local(ValType::I32);
        self.extend([
            I::LocalGet(capacity),
            I::I32Const(2),
            I::I32Mul,
            I::LocalSet(next),
        ]);
        let buffer = self.array_storage(next, 4);
        self.extend([
            I::LocalGet(buffer),
            I::LocalGet(old_data),
            I::LocalGet(count),
            I::I32Const(4),
            I::I32Mul,
            I::MemoryCopy {
                src_mem: 0,
                dst_mem: 0,
            },
        ]);
        let bytes = self.local(ValType::I32);
        self.extend([
            I::LocalGet(next),
            I::I32Const(4),
            I::I32Mul,
            I::LocalSet(bytes),
        ]);
        let grown = self.byte_span_value(l.index, buffer, bytes)?;
        self.copy(index, 0, grown, self.width(l.index)?);
        self.emit(I::End);
        let (idata, _) = self.map_mut_index_parts(index);
        let destination = self.array_item(idata, at, 4);
        self.extend([
            I::LocalGet(destination),
            I::I32Const(4),
            I::I32Add,
            I::LocalGet(destination),
            I::LocalGet(count),
            I::LocalGet(at),
            I::I32Sub,
            I::I32Const(4),
            I::I32Mul,
            I::MemoryCopy {
                src_mem: 0,
                dst_mem: 0,
            },
            I::LocalGet(destination),
            I::LocalGet(count),
            I::I32Store(memory(0, 2)),
        ]);
        self.emit(I::End);
        self.scalar_as(self.key.node, output, 1)
    }
    fn map_mut_index_parts(&mut self, index: u32) -> (u32, u32) {
        let raw = self.read32(index, DATA);
        let data = self.local(ValType::I32);
        let capacity = self.read32(raw, 4);
        self.extend([
            I::LocalGet(raw),
            I::I32Const(8),
            I::I32Add,
            I::LocalSet(data),
            I::LocalGet(capacity),
            I::I32Const(4),
            I::I32DivU,
            I::LocalSet(capacity),
        ]);
        (data, capacity)
    }
    fn projection_value_type(&mut self, ty: TypeId, p: u32, index: usize) -> Result<u32, String> {
        let fields = self.plan.layouts[ty.index()]
            .object
            .as_ref()
            .ok_or("MapMut edit pair")?;
        let offset = fields.members[index].offset.unwrap() as u32;
        let base = self.table_data(RECORDS, p, DATA);
        let result = self.local(ValType::I32);
        self.extend([
            I::LocalGet(base),
            I::I32Const(offset as i32),
            I::I32Add,
            I::LocalSet(result),
        ]);
        Ok(result)
    }
    pub(crate) fn column_map_result(
        &mut self,
        ty: TypeId,
        keys: u32,
        values: u32,
        count: u32,
        key: TypeId,
        value: TypeId,
    ) -> Result<u32, String> {
        let k = self.array_object(keys, count, count, key)?;
        let v = self.array_object(values, count, count, value)?;
        let out = self.value_as(self.key.node, ty, STRING_BYTES)?;
        for (offset, local) in [(DATA, k), (DATA + 4, count), (DATA + 8, v)] {
            self.extend([
                I::LocalGet(out),
                I::LocalGet(local),
                I::I32Store(memory(offset, 2)),
            ]);
        }
        Ok(out)
    }
}
