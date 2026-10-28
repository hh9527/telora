//! Binary search shared by immutable maps and private indexed builders.
use crate::{abi::*, emit::Emitter};
use telora_core::mir::TypeId;
use wasm_encoder::{BlockType, Instruction as I, ValType};
impl Emitter<'_> {
    pub(super) fn column_lower(
        &mut self,
        keys: u32,
        index: Option<(u32, u32)>,
        count: u32,
        key: u32,
        ty: TypeId,
    ) -> Result<(u32, u32), String> {
        let lo = self.local(ValType::I32);
        let hi = self.local(ValType::I32);
        let mid = self.local(ValType::I32);
        let found = self.local(ValType::I32);
        self.extend([
            I::LocalGet(count),
            I::LocalSet(hi),
            I::Block(BlockType::Empty),
            I::Loop(BlockType::Empty),
            I::LocalGet(lo),
            I::LocalGet(hi),
            I::I32GeU,
            I::BrIf(1),
            I::LocalGet(lo),
            I::LocalGet(hi),
            I::LocalGet(lo),
            I::I32Sub,
            I::I32Const(1),
            I::I32ShrU,
            I::I32Add,
            I::LocalSet(mid),
        ]);
        let entry = if let Some((base, stride)) = index {
            let at = self.array_item(base, mid, stride);
            self.read32(at, if stride == 4 { 0 } else { DATA })
        } else {
            mid
        };
        let existing = self.array_item(keys, entry, self.width(ty)?);
        let cmp = self.map_order_value(ty, existing, key, 0)?;
        self.extend([
            I::LocalGet(cmp),
            I::I32Eqz,
            I::If(BlockType::Empty),
            I::I32Const(1),
            I::LocalSet(found),
            I::LocalGet(mid),
            I::LocalSet(lo),
            I::Br(2),
            I::End,
            I::LocalGet(cmp),
            I::I32Const(0),
            I::I32LtS,
            I::If(BlockType::Empty),
            I::LocalGet(mid),
            I::I32Const(1),
            I::I32Add,
            I::LocalSet(lo),
            I::Else,
            I::LocalGet(mid),
            I::LocalSet(hi),
            I::End,
            I::Br(0),
            I::End,
            I::End,
        ]);
        Ok((lo, found))
    }
    pub(super) fn column_lookup(&mut self, map: u32, key: u32, ty: TypeId) -> Result<u32, String> {
        let kt = self.mir.types[ty.index()].arguments[1];
        let vt = self.mir.types[ty.index()].arguments[0];
        let keys = self.table_data(ARRAYS, map, DATA);
        let values = self.table_data(ARRAYS, map, DATA + 8);
        let count = self.read32(map, DATA + 4);
        let (at, found) = self.column_lower(keys, None, count, key, kt)?;
        let result = self.local(ValType::I32);
        self.extend([I::LocalGet(found), I::If(BlockType::Empty)]);
        let value = self.array_item(values, at, self.width(vt)?);
        self.extend([I::LocalGet(value), I::LocalSet(result), I::End]);
        Ok(result)
    }
}
