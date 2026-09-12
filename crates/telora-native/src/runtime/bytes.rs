use super::*;

impl Runtime {
    pub fn bytes(&mut self, ty: TypeId, loc: Location, bytes: &[u8]) -> Result<Value> {
        self.expect(ty, Kind::Bytes)?;
        let length = u32::try_from(bytes.len()).map_err(|_| "native Bytes length overflow")?;
        let slot = self.work.bytes.push(bytes)?;
        let heap = HeapRef::new(World::Work, slot)?.raw();
        self.pack(ty, loc, &[u64::from(heap), u64::from(length)])
    }
    pub fn bytes_data<'a>(&'a self, value: &Value) -> Result<&'a [u8]> {
        self.validate(value.as_ref(), value.type_id())?;
        self.expect(value.type_id(), Kind::Bytes)?;
        let heap = HeapRef::from_raw(value.words[2] as u32);
        let start = (value.words[2] >> 32) as usize;
        let end =
            usize::try_from(u32::try_from(value.words[3]).map_err(|_| "invalid Bytes end word")?)
                .map_err(|_| "Bytes end overflow")?;
        self.tables(heap)
            .bytes
            .get(heap.slot())?
            .get(start..end)
            .ok_or_else(|| "Bytes slice outside backing buffer".into())
    }
    pub fn bytes_slice(
        &self,
        value: &Value,
        start: usize,
        end: usize,
        loc: Location,
    ) -> Result<Value> {
        let bytes = self.bytes_data(value)?;
        if start > end || end > bytes.len() {
            return Err("Bytes slice out of bounds".into());
        }
        let base = value.words[2] >> 32;
        self.pack(
            value.type_id(),
            loc,
            &[
                (value.words[2] & 0xffff_ffff) | ((base + start as u64) << 32),
                base + end as u64,
            ],
        )
    }
}
