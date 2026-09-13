//! Schema objects are constructed as ordinary, closed Value instances.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::{TypeConstructor as T, TypeId};
use wasm_encoder::{BlockType, Instruction as I, ValType};

impl Emitter<'_> {
    pub(crate) fn schema_string(
        &mut self,
        target: TypeId,
        text: &[u8],
        input: u32,
    ) -> Result<u32, String> {
        let text = self.text_as(self.key.node, self.string_type()?, text)?;
        self.copy(text, 0, input, 12);
        self.codec_variant(target, "String", Some(text), input)
    }
    pub(crate) fn schema_integer(
        &mut self,
        target: TypeId,
        number: i64,
        input: u32,
    ) -> Result<u32, String> {
        let ty = self
            .mir
            .types
            .iter()
            .enumerate()
            .find(|(_, t)| t.constructor == T::Int)
            .map(|(id, _)| self.plan.layouts[id].id())
            .ok_or("Wasm: schema Int identity missing")?;
        let value = self.value_as(self.key.node, ty, 24)?;
        self.copy(value, 0, input, 12);
        self.extend([
            I::LocalGet(value),
            I::I64Const(number),
            I::I64Store(memory(DATA, 3)),
        ]);
        self.codec_variant(target, "Int", Some(value), input)
    }
    pub(crate) fn schema_payload_type(&self, target: TypeId, name: &str) -> Result<TypeId, String> {
        self.plan.layouts[target.index()]
            .variants
            .iter()
            .find(|v| v.name == name)
            .and_then(|v| v.type_id)
            .map(|id| self.plan.layouts[id].id())
            .ok_or_else(|| format!("Wasm: schema Value lacks {name}"))
    }
    pub(crate) fn schema_array(
        &mut self,
        target: TypeId,
        items: &[u32],
        input: u32,
    ) -> Result<u32, String> {
        let width = self.width(target)?;
        let data = self.alloc(items.len() as u32 * width);
        for (index, &value) in items.iter().enumerate() {
            self.copy(data, index as u32 * width, value, width);
        }
        let count = self.local(ValType::I32);
        self.extend([I::I32Const(items.len() as i32), I::LocalSet(count)]);
        let value = self.array_result(
            self.schema_payload_type(target, "Array")?,
            data,
            count,
            width,
        )?;
        self.copy(value, 0, input, 12);
        self.codec_variant(target, "Array", Some(value), input)
    }
    pub(crate) fn schema_object(
        &mut self,
        target: TypeId,
        mut fields: Vec<(String, u32)>,
        input: u32,
    ) -> Result<u32, String> {
        fields.sort_by(|a, b| a.0.cmp(&b.0));
        let width = self.width(target)?;
        let keys = self.alloc(fields.len() as u32 * 32);
        let values = self.alloc(fields.len() as u32 * width);
        for (index, (name, value)) in fields.iter().enumerate() {
            let key = self.text_as(self.key.node, self.string_type()?, name.as_bytes())?;
            self.copy(key, 0, input, 12);
            self.copy(keys, index as u32 * 32, key, 32);
            self.copy(values, index as u32 * width, *value, width);
        }
        let count = self.local(ValType::I32);
        self.extend([I::I32Const(fields.len() as i32), I::LocalSet(count)]);
        let object = self.dict_result(
            self.schema_payload_type(target, "Object")?,
            keys,
            values,
            count,
            width,
        )?;
        self.copy(object, 0, input, 12);
        self.codec_variant(target, "Object", Some(object), input)
    }
    pub(crate) fn schema_add_field(
        &mut self,
        target: TypeId,
        object: u32,
        name: &str,
        value: u32,
        input: u32,
    ) -> Result<u32, String> {
        let key = self.text_as(self.key.node, self.string_type()?, name.as_bytes())?;
        self.schema_add_key(target, object, key, value, input)
    }
    pub(crate) fn schema_add_key(
        &mut self,
        target: TypeId,
        object: u32,
        key: u32,
        value: u32,
        input: u32,
    ) -> Result<u32, String> {
        self.copy(key, 0, input, 12);
        let index = self.plan.layouts[target.index()]
            .variants
            .iter()
            .position(|v| v.name == "Object")
            .ok_or("Wasm: schema Object missing")?;
        let object = self.enum_payload(target, index as u32, object)?;
        let old_count = self.read32(object, 20);
        let count = self.local(ValType::I32);
        self.extend([
            I::LocalGet(old_count),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(count),
        ]);
        let width = self.width(target)?;
        let old_keys = self.table_data(ARRAYS, object, DATA);
        let old_values = self.table_data(ARRAYS, object, 24);
        let pairs = self.array_storage(count, 8);
        let cursor = self.local(ValType::I32);
        self.extend([
            I::I32Const(0),
            I::LocalSet(cursor),
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(cursor),
            I::LocalGet(old_count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let pair = self.array_item(pairs, cursor, 8);
        let old_key = self.array_item(old_keys, cursor, 32);
        let item = self.array_item(old_values, cursor, width);
        for (offset, local) in [(0, old_key), (4, item)] {
            self.extend([
                I::LocalGet(pair),
                I::LocalGet(local),
                I::I32Store(memory(offset, 2)),
            ]);
        }
        self.extend([
            I::LocalGet(cursor),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(cursor),
            I::Br(0),
            I::End,
            I::End,
        ]);
        let pair = self.array_item(pairs, old_count, 8);
        for (offset, local) in [(0, key), (4, value)] {
            self.extend([
                I::LocalGet(pair),
                I::LocalGet(local),
                I::I32Store(memory(offset, 2)),
            ]);
        }
        self.extend([
            I::LocalGet(pairs),
            I::LocalGet(count),
            I::Call(SORT_PAIRS),
            I::Drop,
        ]);
        let keys = self.array_storage(count, 32);
        let values = self.array_storage(count, width);
        self.extend([
            I::I32Const(0),
            I::LocalSet(cursor),
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(cursor),
            I::LocalGet(count),
            I::I32GeU,
            I::BrIf(1),
        ]);
        let pair = self.array_item(pairs, cursor, 8);
        let key = self.read32(pair, 0);
        let value = self.read32(pair, 4);
        let destination = self.array_item(keys, cursor, 32);
        self.copy(destination, 0, key, 32);
        let destination = self.array_item(values, cursor, width);
        self.copy(destination, 0, value, width);
        self.extend([
            I::LocalGet(cursor),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(cursor),
            I::Br(0),
            I::End,
            I::End,
        ]);
        let object = self.dict_result(
            self.schema_payload_type(target, "Object")?,
            keys,
            values,
            count,
            width,
        )?;
        self.copy(object, 0, input, 12);
        self.codec_variant(target, "Object", Some(object), input)
    }
}
