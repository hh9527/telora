use crate::{abi::*, emit::Emitter};
use telora_core::mir::{HirId, TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn dict_from_pairs(&mut self, args: &[TypeId]) -> Result<u32, String> {
        let node = self.key.node;
        if args.len() != 2
            || self.mir.types[args[0].index()].constructor != T::Array
            || self.mir.types[args[1].index()].constructor != T::Dict
        {
            return Err("Wasm: Dict from_pairs signature mismatch".into());
        }
        let pair = self.mir.types[args[0].index()].arguments[0];
        let element = self.mir.types[args[1].index()].arguments[0];
        let string = self.string_type()?;
        if self.mir.types[pair.index()].constructor != T::Tuple
            || self.mir.types[pair.index()].arguments != [string, element]
        {
            return Err("Wasm: Dict input pair type mismatch".into());
        }
        if self.width(pair)? == 0 {
            // An array of uninhabited pairs can only be empty.
            let zero = self.local(ValType::I32);
            return self.dict_result(args[1], zero, zero, zero, 0);
        }
        let layout = self.plan.layouts[pair.index()]
            .object
            .as_ref()
            .ok_or("Wasm: pair layout missing")?;
        let key_offset = layout.members[0]
            .offset
            .ok_or("Wasm: pair key offset missing")? as i32;
        let value_offset = layout.members[1]
            .offset
            .ok_or("Wasm: pair value offset missing")? as i32;
        let input = self.parameter(0);
        let (base, count) = self.array_parts(input, self.width(pair)?);
        let pointers = self.array_storage(count, 8);
        let index = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(index),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let item = self.array_item(base, index, self.width(pair)?);
        let data = self.table_data(RECORDS, item, DATA);
        let destination = self.array_item(pointers, index, 8);
        for (offset, field) in [(0, key_offset), (4, value_offset)] {
            self.extend([
                I::LocalGet(destination),
                I::LocalGet(data),
                I::I32Const(field),
                I::I32Add,
                I::I32Store(memory(offset, 2)),
            ]);
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
        let duplicate = self.local(ValType::I32);
        self.extend([
            I::LocalGet(pointers),
            I::LocalGet(count),
            I::Call(SORT_PAIRS),
            I::LocalTee(duplicate),
            I::If(BlockType::Empty),
        ]);
        let span = self.local(ValType::I32);
        self.extend([
            I::LocalGet(duplicate),
            I::Call(DUPLICATE_KEY_MESSAGE),
            I::LocalSet(span),
        ]);
        let message = self.text_span_value(string, span)?;
        let one = self.local(ValType::I32);
        self.extend([I::I32Const(1), I::LocalSet(one)]);
        self.report(node, message, duplicate, one, false);
        self.emit(I::End);
        let width = self.width(element)?;
        let keys = self.array_storage(count, STRING_BYTES);
        let values = self.array_storage(count, width);
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
        let source = self.array_item(pointers, index, 8);
        for (out, offset, width) in [(keys, 0, STRING_BYTES), (values, 4, width)] {
            let value = self.local(ValType::I32);
            self.extend([
                I::LocalGet(source),
                I::I32Load(memory(offset, 2)),
                I::LocalSet(value),
            ]);
            let destination = self.array_item(out, index, width);
            self.copy(destination, 0, value, width);
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
        self.dict_result(args[1], keys, values, count, width)
    }

    pub(crate) fn dict_merge_values(
        &mut self,
        node: HirId,
        ty: TypeId,
        left: u32,
        right: u32,
    ) -> Result<u32, String> {
        let width = self.width(self.mir.types[ty.index()].arguments[0])?;
        let left_count = self.local(ValType::I32);
        let right_count = self.local(ValType::I32);
        let capacity = self.local(ValType::I32);
        self.extend([
            I::LocalGet(left),
            I::I32Load(memory(DATA + 4, 2)),
            I::LocalSet(left_count),
            I::LocalGet(right),
            I::I32Load(memory(DATA + 4, 2)),
            I::LocalSet(right_count),
            I::LocalGet(left_count),
            I::LocalGet(right_count),
            I::I32Add,
            I::LocalTee(capacity),
            I::LocalGet(left_count),
            I::I32LtU,
        ]);
        self.fail_if(node, ERROR_OVERFLOW);
        let keys = self.array_storage(capacity, STRING_BYTES);
        let values = self.array_storage(capacity, width);
        let lk = self.table_data(ARRAYS, left, DATA);
        let lv = self.table_data(ARRAYS, left, DATA + 8);
        let rk = self.table_data(ARRAYS, right, DATA);
        let rv = self.table_data(ARRAYS, right, DATA + 8);
        let a = self.local(ValType::I32);
        let b = self.local(ValType::I32);
        let count = self.local(ValType::I32);
        let choose_left = self.local(ValType::I32);
        self.extend([
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(a),
            I::LocalGet(left_count),
            I::I32GeU,
            I::LocalGet(b),
            I::LocalGet(right_count),
            I::I32GeU,
            I::I32And,
            I::BrIf(1),
            I::LocalGet(b),
            I::LocalGet(right_count),
            I::I32Eq,
            I::LocalSet(choose_left),
            I::LocalGet(b),
            I::LocalGet(right_count),
            I::I32LtU,
            I::LocalGet(a),
            I::LocalGet(left_count),
            I::I32LtU,
            I::I32And,
            I::If(BlockType::Empty),
        ]);
        let lkey = self.array_item(lk, a, STRING_BYTES);
        let rkey = self.array_item(rk, b, STRING_BYTES);
        let order = self.local(ValType::I32);
        self.extend([
            I::LocalGet(lkey),
            I::LocalGet(rkey),
            I::Call(STRING_COMPARE),
            I::LocalTee(order),
            I::I32Const(0),
            I::I32LtS,
            I::LocalSet(choose_left),
            I::LocalGet(order),
            I::I32Eqz,
            I::If(BlockType::Empty),
            I::LocalGet(a),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(a),
            I::End,
            I::End,
        ]);
        self.extend([I::LocalGet(choose_left), I::If(BlockType::Empty)]);
        for (branch, key_base, value_base, cursor) in [(0, lk, lv, a), (1, rk, rv, b)] {
            if branch == 1 {
                self.emit(I::Else);
            }
            let key = self.array_item(key_base, cursor, STRING_BYTES);
            let item = self.array_item(value_base, cursor, width);
            let destination = self.array_item(keys, count, STRING_BYTES);
            self.copy(destination, 0, key, STRING_BYTES);
            let destination = self.array_item(values, count, width);
            self.copy(destination, 0, item, width);
            self.extend([
                I::LocalGet(cursor),
                I::I32Const(1),
                I::I32Add,
                I::LocalSet(cursor),
            ]);
        }
        self.extend([
            I::End,
            I::LocalGet(count),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(count),
            I::Br(0),
            I::End,
            I::End,
        ]);
        self.dict_result_at(node, ty, keys, values, count, width)
    }
}
