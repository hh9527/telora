//! Scoped prototype Dict builder. All references live in ordinary typed arrays;
//! the open-addressed index stores integers, never untraced value pointers.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::{TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

pub(super) struct Layout {
    state: TypeId,
    entries: TypeId,
    pair: TypeId,
    index: TypeId,
    int: TypeId,
    boolean: TypeId,
    offsets: [u32; 3],
}
impl Emitter<'_> {
    pub(super) fn pipeline_layout(&self, builder: TypeId) -> Result<Layout, String> {
        if self.mir.types[builder.index()].constructor != T::Array {
            return Err("pipeline builder must be typed Array".into());
        }
        let state = self.mir.types[builder.index()].arguments[0];
        let members = &self.plan.layouts[state.index()]
            .object
            .as_ref()
            .ok_or("pipeline state layout")?
            .members;
        if members.iter().map(|f| f.name.as_str()).collect::<Vec<_>>()
            != ["entries", "index", "sealed"]
        {
            return Err("invalid pipeline state".into());
        }
        let tys = members
            .iter()
            .map(|f| self.plan.layouts[f.type_id.unwrap()].id())
            .collect::<Vec<_>>();
        let entries = tys[0];
        let index = tys[1];
        Ok(Layout {
            state,
            entries,
            pair: self.mir.types[entries.index()].arguments[0],
            index,
            int: self.mir.types[index.index()].arguments[0],
            boolean: tys[2],
            offsets: [
                members[0].offset.unwrap() as u32,
                members[1].offset.unwrap() as u32,
                members[2].offset.unwrap() as u32,
            ],
        })
    }
    fn pipeline_state(&mut self, builder: u32, layout: &Layout) -> u32 {
        let (base, _) = self.array_parts(builder, self.width(layout.state).unwrap());
        self.table_data(RECORDS, base, DATA)
    }
    fn pipeline_field(&mut self, base: u32, offset: u32) -> u32 {
        let result = self.local(ValType::I32);
        self.extend([
            I::LocalGet(base),
            I::I32Const(offset as i32),
            I::I32Add,
            I::LocalSet(result),
        ]);
        result
    }
    fn pipeline_pair_fields(&mut self, pair: TypeId, value: u32) -> Result<(u32, u32), String> {
        let object = self.table_data(RECORDS, value, DATA);
        let fields = &self.plan.layouts[pair.index()]
            .object
            .as_ref()
            .ok_or("pipeline pair layout")?
            .members;
        let key_offset = fields[0].offset.unwrap() as u32;
        let value_offset = fields[1].offset.unwrap() as u32;
        Ok((
            self.pipeline_field(object, key_offset),
            self.pipeline_field(object, value_offset),
        ))
    }
    // A zero-filled index is safe because Int has no traced references.
    fn pipeline_empty_index(&mut self, layout: &Layout, capacity: u32) -> Result<u32, String> {
        let width = self.width(layout.int)?;
        let data = self.array_storage(capacity, width);
        self.extend([
            I::LocalGet(data),
            I::I32Const(0),
            I::LocalGet(capacity),
            I::I32Const(width as i32),
            I::I32Mul,
            I::MemoryFill(0),
        ]);
        self.array_result(layout.index, data, capacity, width)
    }
    fn pipeline_index_slot(
        &mut self,
        base: u32,
        capacity: u32,
        key: u32,
        entries: u32,
        pair: TypeId,
        int: TypeId,
    ) -> Result<(u32, u32), String> {
        let mask = self.local(ValType::I32);
        let cursor = self.local(ValType::I32);
        let found = self.local(ValType::I32);
        let cell = self.local(ValType::I32);
        self.extend([
            I::LocalGet(capacity),
            I::I32Const(1),
            I::I32Sub,
            I::LocalSet(mask),
            I::LocalGet(key),
            I::Call(DICT_KEY_HASH),
            I::LocalGet(mask),
            I::I32And,
            I::LocalSet(cursor),
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
        ]);
        let slot = self.array_item(base, cursor, self.width(int)?);
        self.extend([
            I::LocalGet(slot),
            I::LocalSet(cell),
            I::LocalGet(slot),
            I::I32Load(memory(DATA, 2)),
            I::LocalTee(found),
            I::I32Eqz,
            I::BrIf(1),
        ]);
        let position = self.local(ValType::I32);
        self.extend([
            I::LocalGet(found),
            I::I32Const(1),
            I::I32Sub,
            I::LocalSet(position),
        ]);
        let item = self.array_item(entries, position, self.width(pair)?);
        let (existing, _) = self.pipeline_pair_fields(pair, item)?;
        self.extend([
            I::LocalGet(existing),
            I::LocalGet(key),
            I::Call(STRING_COMPARE),
            I::I32Eqz,
            I::BrIf(1),
            I::LocalGet(cursor),
            I::I32Const(1),
            I::I32Add,
            I::LocalGet(mask),
            I::I32And,
            I::LocalSet(cursor),
            I::Br(0),
            I::End,
            I::End,
        ]);
        Ok((cell, found))
    }
    fn pipeline_reindex(
        &mut self,
        layout: &Layout,
        entries: u32,
        capacity: u32,
    ) -> Result<u32, String> {
        let index = self.pipeline_empty_index(layout, capacity)?;
        self.root_value(index, layout.index);
        let (base, _) = self.array_parts(index, self.width(layout.int)?);
        let (data, count) = self.array_parts(entries, self.width(layout.pair)?);
        let cursor = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(cursor),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let item = self.array_item(data, cursor, self.width(layout.pair)?);
        let (key, _) = self.pipeline_pair_fields(layout.pair, item)?;
        let (slot, _) =
            self.pipeline_index_slot(base, capacity, key, data, layout.pair, layout.int)?;
        self.extend([
            I::LocalGet(slot),
            I::LocalGet(cursor),
            I::I64ExtendI32U,
            I::I64Const(1),
            I::I64Add,
            I::I64Store(memory(DATA, 3)),
            I::LocalGet(cursor),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(cursor),
            I::Br(0),
            I::End,
            I::End,
        ]);
        Ok(index)
    }
    pub(super) fn pipeline_dict_native(
        &mut self,
        name: &str,
        args: &[TypeId],
        builder: u32,
    ) -> Result<u32, String> {
        let output = *args.last().unwrap();
        let layout = self.pipeline_layout(args[0])?;
        let state = self.pipeline_state(builder, &layout);
        let sealed = self.pipeline_field(state, layout.offsets[2]);
        self.bits(sealed);
        self.extend([I::I64Eqz, I::I32Eqz]);
        self.fail_if(self.key.node, ERROR_UNINITIALIZED_CALL);
        let entries = self.pipeline_field(state, layout.offsets[0]);
        if name == "dict_finish" {
            self.store64(sealed, DATA, 1);
            return self.dict_from_pairs_value(&[layout.entries, output], entries);
        }
        if name != "dict_accept" {
            return Err("unknown pipeline operation".into());
        }
        let edit = self.parameter(1);
        let edit_ty = args[1];
        let variants = &self.plan.layouts[edit_ty.index()].variants;
        if variants.len() != 2 || variants[0].name != "Thunk" || variants[1].name != "Update" {
            return Err("invalid Dict edit".into());
        }
        let thunk_pair = self.plan.layouts[variants[0].type_id.unwrap()].id();
        let update_pair = self.plan.layouts[variants[1].type_id.unwrap()].id();
        let tag = self.read32(edit, DATA);
        let key = self.local(ValType::I32);
        let payload = self.local(ValType::I32);
        self.extend([I::LocalGet(tag), I::If(BlockType::Empty)]);
        for (which, ty) in [(1, update_pair), (0, thunk_pair)] {
            if which == 0 {
                self.emit(I::Else);
            }
            let p = self.enum_payload(edit_ty, which, edit)?;
            let (k, v) = self.pipeline_pair_fields(ty, p)?;
            self.extend([
                I::LocalGet(k),
                I::LocalSet(key),
                I::LocalGet(v),
                I::LocalSet(payload),
            ]);
        }
        self.emit(I::End);
        let index = self.pipeline_field(state, layout.offsets[1]);
        let (index_base, index_capacity) = self.array_parts(index, self.width(layout.int)?);
        let (entry_base, count) = self.array_parts(entries, self.width(layout.pair)?);
        let (slot, found) = self.pipeline_index_slot(
            index_base,
            index_capacity,
            key,
            entry_base,
            layout.pair,
            layout.int,
        )?;
        // A duplicate thunk is never called; updates overwrite the existing entry.
        self.extend([
            I::LocalGet(found),
            I::I32Eqz,
            I::I32Eqz,
            I::LocalGet(tag),
            I::I32Eqz,
            I::I32And,
            I::If(BlockType::Empty),
        ]);
        let yes = self.scalar_as(self.key.node, output, 1)?;
        self.extend([I::LocalGet(yes), I::Return, I::End]);
        let value_ty = self.mir.types[layout.pair.index()].arguments[1];
        let value = self.local(ValType::I32);
        self.extend([
            I::LocalGet(tag),
            I::If(BlockType::Empty),
            I::LocalGet(payload),
            I::LocalSet(value),
            I::Else,
        ]);
        let computed = self.invoke(payload, &[], value_ty)?;
        self.extend([I::LocalGet(computed), I::LocalSet(value), I::End]);
        self.root_value(value, value_ty);
        let pair = self.packed_tuple(layout.pair, &[key, value])?;
        self.root_value(pair, layout.pair);
        self.extend([I::LocalGet(found), I::If(BlockType::Empty)]);
        let position = self.local(ValType::I32);
        self.extend([
            I::LocalGet(found),
            I::I32Const(1),
            I::I32Sub,
            I::LocalSet(position),
        ]);
        let target = self.array_item(entry_base, position, self.width(layout.pair)?);
        self.copy(target, 0, pair, self.width(layout.pair)?);
        self.emit(I::Else);
        let capacity = self.local(ValType::I32);
        self.extend([
            I::LocalGet(count),
            I::LocalSet(capacity),
            I::LocalGet(entries),
            I::I32Load(memory(DATA + 12, 2)),
            I::I32Const(ARRAY_SLICE as i32),
            I::I32Eq,
            I::If(BlockType::Empty),
        ]);
        let object = self.read32(entries, DATA);
        let stored_capacity = self.read32(object, 12);
        self.extend([I::LocalGet(stored_capacity), I::LocalSet(capacity), I::End]);
        let old_count = self.local(ValType::I32);
        self.extend([I::LocalGet(count), I::LocalSet(old_count)]);
        self.sequence_append(entry_base, count, capacity, layout.pair, pair)?;
        let grown = self.array_result_with_capacity(
            layout.entries,
            entry_base,
            count,
            capacity,
            self.width(layout.pair)?,
        )?;
        self.copy(entries, 0, grown, self.width(layout.entries)?);
        self.extend([
            I::LocalGet(count),
            I::LocalGet(index_capacity),
            I::I32Const(2),
            I::I32DivU,
            I::I32GeU,
            I::If(BlockType::Empty),
        ]);
        self.extend([
            I::LocalGet(index_capacity),
            I::I32Const((u32::MAX / 2) as i32),
            I::I32GtU,
        ]);
        self.fail_if(self.key.node, ERROR_OVERFLOW);
        let next_capacity = self.local(ValType::I32);
        self.extend([
            I::LocalGet(index_capacity),
            I::I32Const(2),
            I::I32Mul,
            I::LocalSet(next_capacity),
        ]);
        let next_index = self.pipeline_reindex(&layout, entries, next_capacity)?;
        self.copy(index, 0, next_index, self.width(layout.index)?);
        self.emit(I::Else);
        self.extend([
            I::LocalGet(slot),
            I::LocalGet(old_count),
            I::I64ExtendI32U,
            I::I64Const(1),
            I::I64Add,
            I::I64Store(memory(DATA, 3)),
            I::End,
            I::End,
        ]);
        self.scalar_as(self.key.node, output, 1)
    }
    pub(super) fn pipeline_new(
        &mut self,
        args: &[TypeId],
        layout: &Layout,
        initial: u32,
    ) -> Result<u32, String> {
        let count = self.read32(initial, DATA + 4);
        let keys = self.table_data(ARRAYS, initial, DATA);
        let values = self.table_data(ARRAYS, initial, DATA + 8);
        let value_ty = self.mir.types[args[0].index()].arguments[0];
        let data = self.array_storage(count, self.width(layout.pair)?);
        let cursor = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(cursor),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        self.root_array_prefix(data, layout.pair, cursor)?;
        let key = self.array_item(keys, cursor, STRING_BYTES);
        let value = self.array_item(values, cursor, self.width(value_ty)?);
        let pair = self.packed_tuple(layout.pair, &[key, value])?;
        let target = self.array_item(data, cursor, self.width(layout.pair)?);
        self.copy(target, 0, pair, self.width(layout.pair)?);
        self.extend([
            I::LocalGet(cursor),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(cursor),
            I::Br(0),
            I::End,
            I::End,
        ]);
        let entries = self.array_result(layout.entries, data, count, self.width(layout.pair)?)?;
        self.root_value(entries, layout.entries);
        let capacity = self.local(ValType::I32);
        self.extend([
            I::I32Const(16),
            I::LocalSet(capacity),
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(capacity),
            I::I32Const(2),
            I::I32DivU,
            I::LocalGet(count),
            I::I32GtU,
            I::BrIf(1),
            I::LocalGet(capacity),
            I::I32Const((u32::MAX / 2) as i32),
            I::I32GtU,
        ]);
        self.fail_if(self.key.node, ERROR_OVERFLOW);
        self.extend([
            I::LocalGet(capacity),
            I::I32Const(2),
            I::I32Mul,
            I::LocalSet(capacity),
            I::Br(0),
            I::End,
            I::End,
        ]);
        let index = self.pipeline_reindex(layout, entries, capacity)?;
        self.root_value(index, layout.index);
        let sealed = self.scalar_as(self.key.node, layout.boolean, 0)?;
        let state = self.packed_tuple(layout.state, &[entries, index, sealed])?;
        self.root_value(state, layout.state);
        let one = self.local(ValType::I32);
        self.extend([I::I32Const(1), I::LocalSet(one)]);
        let outer = self.array_storage(one, self.width(layout.state)?);
        self.copy(outer, 0, state, self.width(layout.state)?);
        self.array_result(*args.last().unwrap(), outer, one, self.width(layout.state)?)
    }
}
