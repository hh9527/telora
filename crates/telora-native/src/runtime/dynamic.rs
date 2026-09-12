use super::*;

impl Runtime {
    pub fn dynamic_field(&self, value: &Value, name: &Value) -> Result<ValueRef<'_>> {
        let payload = self.dynamic_value(value)?.to_owned();
        let layout = self.layout(payload.type_id())?;
        let text = self.text(name.as_ref())?;
        let found = if layout.kind == Kind::Dict {
            self.dict_get(&payload, name)?
        } else if layout.kind == Kind::Record && layout.dynamic_kind == Some("Dict") {
            layout
                .field_names
                .iter()
                .position(|n| n == text.as_str())
                .map(|index| self.field(&payload, index))
                .transpose()?
        } else {
            return Err("Dyn field access expects Struct".into());
        };
        found.ok_or_else(|| format!("Dyn record has no field {:?}", text.as_str()))
    }
    pub fn dynamic_kind(&self, value: &Value) -> Result<&'static str> {
        let payload = self.dynamic_value(value)?;
        let layout = self.layout(payload.type_id())?;
        if let Some(kind) = layout.dynamic_kind {
            return Ok(kind);
        }
        if layout.kind == Kind::Enum {
            return Ok(if self.enum_payload_ref(payload)?.is_some() {
                "Tagged"
            } else {
                "Atom"
            });
        }
        Err("Dyn witness is not an executable value type".into())
    }
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
