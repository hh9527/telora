use super::*;

impl Runtime {
    /// Box only the fixed-width descriptor. Its referenced objects stay shared.
    pub fn dynamic(&mut self, ty: TypeId, loc: Location, value: &Value) -> Result<Value> {
        self.expect(ty, Kind::Dyn)?;
        self.validate(value.as_ref(), value.type_id())?;
        let heap = self.push_words(Table::Values, value.words().to_vec())?;
        self.pack(
            ty,
            loc,
            &[
                u64::from(value.type_id().raw()) | (1u64 << 32),
                u64::from(heap),
                0,
            ],
        )
    }

    pub fn dynamic_value<'a>(&'a self, value: &Value) -> Result<ValueRef<'a>> {
        self.validate(value.as_ref(), value.type_id())?;
        self.expect(value.type_id(), Kind::Dyn)?;
        if value.words[2] >> 32 != 1 || value.words[4] != 0 {
            return Err("unsupported native Dyn storage".into());
        }
        let ty = TypeId(value.words[2] as u32);
        let heap = u32::try_from(value.words[3]).map_err(|_| "invalid Dyn HeapId")?;
        let result = ValueRef {
            arena: self.identity,
            words: self.object_words(Table::Values, heap)?,
        };
        self.validate(result, ty)?;
        Ok(result)
    }
}
