use super::*;

impl Runtime {
    /// FunctionId is a code-plan identity, never a machine address. Environment
    /// zero means no captures; other IDs encode HeapRef + 1 in this table only.
    pub fn closure(
        &mut self,
        ty: TypeId,
        loc: Location,
        function: u32,
        captures: &[Value],
    ) -> Result<Value> {
        self.expect(ty, Kind::Function)?;
        let environment = if captures.is_empty() {
            0
        } else {
            let mut words = Vec::new();
            for capture in captures {
                self.validate(capture.as_ref(), capture.type_id())?;
                words.extend_from_slice(capture.words());
            }
            self.push_words(Table::Environments, words)?
                .checked_add(1)
                .ok_or("environment ID overflow")?
        };
        self.pack(
            ty,
            loc,
            &[u64::from(function) | (u64::from(environment) << 32)],
        )
    }
    pub fn function_id(&self, value: &Value) -> Result<u32> {
        self.validate(value.as_ref(), value.type_id())?;
        self.expect(value.type_id(), Kind::Function)?;
        Ok(value.words[2] as u32)
    }
    pub fn capture(&self, value: &Value, index: usize) -> Result<ValueRef<'_>> {
        self.function_id(value)?;
        let environment = (value.words[2] >> 32) as u32;
        let raw = environment
            .checked_sub(1)
            .ok_or("closure has no captures")?;
        let mut words = self.object_words(Table::Environments, raw)?;
        for current in 0..=index {
            let header = words.get(1).ok_or("capture index outside environment")?;
            let ty = TypeId((header >> 32) as u32);
            let width = self.layout(ty)?.words;
            let capture = words.get(..width).ok_or("truncated closure capture")?;
            if current == index {
                return Ok(ValueRef {
                    arena: self.identity,
                    words: capture,
                });
            }
            words = &words[width..];
        }
        unreachable!()
    }
}
