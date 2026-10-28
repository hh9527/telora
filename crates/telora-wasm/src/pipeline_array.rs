//! ArrayMut owns its entry buffer. Initial arrays are copied; finish transfers
//! the buffer through a fresh immutable header after sealing the builder.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::TypeId;
use wasm_encoder::{BlockType, Instruction as I, ValType};
impl Emitter<'_> {
    pub(super) fn pipeline_array_new(
        &mut self,
        initial: u32,
        result: TypeId,
        state: TypeId,
        storage: TypeId,
    ) -> Result<u32, String> {
        let element = self.mir.types[result.index()].arguments[0];
        let width = self.width(element)?;
        let (source, count) = self.array_parts(initial, width);
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
        let entries = self.array_result(result, data, count, width)?;
        self.root_value(entries, result);
        let boolean = self.builder_field_type(state, "sealed")?;
        let sealed = self.scalar_as(self.key.node, boolean, 0)?;
        let object = self.packed_tuple(state, &[entries, sealed])?;
        self.root_value(object, state);
        let one = self.local(ValType::I32);
        self.extend([I::I32Const(1), I::LocalSet(one)]);
        let outer = self.array_storage(one, self.width(state)?);
        self.copy(outer, 0, object, self.width(state)?);
        self.array_result(storage, outer, one, self.width(state)?)
    }
    // Trusted private builder state owns this header and backing store.
    // Initial inputs were copied; finish seals before publishing an immutable view.
    pub(super) fn private_array_publish(
        &mut self,
        entries: u32,
        data: u32,
        count: u32,
        capacity: u32,
        element: TypeId,
    ) -> Result<(), String> {
        self.extend([
            I::LocalGet(entries),
            I::I32Load(memory(DATA + 12, 2)),
            I::I32Const(ARRAY_SLICE as i32),
            I::I32Eq,
            I::If(BlockType::Empty),
        ]);
        let object = self.read32(entries, DATA);
        for (offset, value) in [(4, data), (8, count), (12, capacity)] {
            self.extend([
                I::LocalGet(object),
                I::LocalGet(value),
                I::I32Store(memory(offset, 2)),
            ]);
        }
        self.extend([
            I::LocalGet(entries),
            I::LocalGet(count),
            I::I32Store(memory(DATA + 8, 2)),
            I::Else,
        ]);
        self.root_array_prefix(data, element, count)?;
        let object = self.array_object(data, count, capacity, element)?;
        self.extend([
            I::LocalGet(entries),
            I::LocalGet(object),
            I::I32Store(memory(DATA, 2)),
        ]);
        self.store32(entries, DATA + 4, 0);
        self.extend([
            I::LocalGet(entries),
            I::LocalGet(count),
            I::I32Store(memory(DATA + 8, 2)),
        ]);
        self.store32(entries, DATA + 12, ARRAY_SLICE);
        self.emit(I::End);
        Ok(())
    }
    pub(super) fn pipeline_array_native(
        &mut self,
        name: &str,
        states: u32,
        state: TypeId,
        result: TypeId,
        output: TypeId,
    ) -> Result<u32, String> {
        let (base, _) = self.array_parts(states, self.width(state)?);
        let (entries, _) = self.sequence_field(base, state, "entries")?;
        let (sealed, _) = self.sequence_field(base, state, "sealed")?;
        self.bits(sealed);
        self.extend([I::I64Eqz, I::I32Eqz]);
        self.fail_if(self.key.node, ERROR_UNINITIALIZED_CALL);
        if name == "finish" {
            self.store64(sealed, DATA, 1);
            let value = self.value_as(self.key.node, output, self.width(output)?)?;
            self.copy(value, 0, entries, self.width(output)?);
            return Ok(value);
        }
        let element = self.mir.types[result.index()].arguments[0];
        let (data, count) = self.array_parts(entries, self.width(element)?);
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
        let stored = self.read32(object, 12);
        self.extend([I::LocalGet(stored), I::LocalSet(capacity), I::End]);
        let item = self.parameter(1);
        self.sequence_append(data, count, capacity, element, item)?;
        if name == "oracle_push"
            || std::env::var_os("TELORA_EXPERIMENTAL_PRIVATE_BUILDERS").is_some()
        {
            self.private_array_publish(entries, data, count, capacity, element)?;
        } else {
            let grown = self.array_result_with_capacity(
                result,
                data,
                count,
                capacity,
                self.width(element)?,
            )?;
            self.copy(entries, 0, grown, self.width(result)?);
        }
        self.scalar_as(self.key.node, output, 1)
    }
}
