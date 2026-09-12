use super::*;

impl Runtime {
    pub fn variant_name(&self, ty: TypeId, index: u32) -> Result<&str> {
        Ok(&self
            .expect(ty, Kind::Enum)?
            .variants
            .get(index as usize)
            .ok_or("enum tag out of bounds")?
            .name)
    }
    pub fn enum_value(
        &mut self,
        ty: TypeId,
        loc: Location,
        index: u32,
        payload: Option<&Value>,
    ) -> Result<Value> {
        let layout = self.expect(ty, Kind::Enum)?;
        let variant = layout
            .variants
            .get(index as usize)
            .ok_or("enum tag out of bounds")?;
        let mut data = vec![0; layout.words - 2];
        data[0] = u64::from(index);
        match (variant.payload, payload) {
            (None, None) if variant.storage == "nullary" => {}
            (Some(expected), Some(payload)) => {
                self.validate(payload.as_ref(), expected)?;
                match variant.storage {
                    "full_value" => {
                        data.get_mut(1..1 + payload.words.len())
                            .ok_or("enum payload width mismatch")?
                            .copy_from_slice(&payload.words);
                    }
                    "heap_id" => {
                        data[1] = self.push_words(Table::Values, payload.words.to_vec())? as u64;
                    }
                    _ => return Err("enum branch has no materializable payload".into()),
                }
            }
            _ => return Err("enum payload arity mismatch".into()),
        }
        self.pack(ty, loc, &data)
    }
    pub fn enum_tag(&self, value: &Value) -> Result<u32> {
        self.validate(value.as_ref(), value.type_key())?;
        let index = u32::try_from(value.words[2]).map_err(|_| "invalid enum tag word")?;
        self.variant_name(value.type_key(), index)?;
        Ok(index)
    }
    pub fn enum_payload<'a>(&'a self, value: &'a Value) -> Result<Option<ValueRef<'a>>> {
        let index = self.enum_tag(value)?;
        let variant = &self.layout(value.type_key())?.variants[index as usize];
        let Some(ty) = variant.payload else {
            return Ok(None);
        };
        let words = match variant.storage {
            "full_value" => value
                .words
                .get(3..3 + self.layout(ty)?.words)
                .ok_or("truncated enum payload")?,
            "heap_id" => self.object_words(Table::Values, value.words[3] as u32)?,
            _ => return Err("enum branch has no materializable payload".into()),
        };
        let result = ValueRef {
            arena: self.identity,
            words,
        };
        self.validate(result, ty)?;
        Ok(Some(result))
    }
}
