use super::*;

impl Runtime {
    pub fn metadata(&self, ty: TypeId, loc: Location, represented: TypeId) -> Result<Value> {
        self.expect(ty, Kind::Metadata)?;
        let value = self.pack(ty, loc, &[u64::from(represented.raw())])?;
        self.represented_type(value.as_ref())?;
        Ok(value)
    }
    pub fn represented_type(&self, value: ValueRef<'_>) -> Result<TypeId> {
        self.validate(value, value.type_id())?;
        let layout = self.expect(value.type_id(), Kind::Metadata)?;
        let represented =
            TypeId(u32::try_from(value.words[2]).map_err(|_| "invalid represented TypeId width")?);
        if represented.index() >= self.layouts.len() {
            return Err("represented TypeId outside sealed graph".into());
        }
        if let Some(&expected) = layout.arguments.first() {
            if represented != expected {
                return Err("metadata contradicts solved TypeOf witness".into());
            }
        }
        Ok(represented)
    }
}
