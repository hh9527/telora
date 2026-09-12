use super::*;
use super::sha256::Context;

impl Runtime {
    pub(super) fn hash_state(&self, value: &Value) -> Result<&Context> {
        self.validate(value.as_ref(), value.type_id())?;
        self.expect(value.type_id(), Kind::Hash)?;
        let reference = HeapRef::from_raw(u32::try_from(value.words[2]).map_err(|_| "invalid HashState HeapId")?);
        self.tables(reference).hashes.get(reference.slot() as usize).ok_or_else(|| "invalid HashState HeapId".into())
    }
    pub(super) fn hash(&mut self, ty: TypeId, inputs: &[Value], loc: Location, operation: usize) -> Result<Value> {
        if operation == 0 {
            let output = sha256::hex(self.text(inputs[0].as_ref())?.as_str().as_bytes());
            return self.owned_string(ty, loc, output);
        }
        if (1..=4).contains(&operation) { self.charge_allocation(1, std::mem::size_of::<Context>(), 0)?; }
        let mut state = if operation == 1 {
            let mut state = Context::default();
            state.update(b"telora.hash\0\x01");
            state
        } else { self.hash_state(&inputs[0])?.clone() };
        match operation {
            1 => {},
            2 => {
                let bytes = self.bytes_data(&inputs[1])?;
                state.update(&[1]); state.update(&(bytes.len() as u64).to_be_bytes()); state.update(bytes);
            }
            3 => {
                let text = self.text(inputs[1].as_ref())?;
                state.update(&[2]); state.update(&(text.as_str().len() as u64).to_be_bytes()); state.update(text.as_str().as_bytes());
            }
            4 => { state.update(&[3]); state.update(&self.scalar_bits(inputs[1].as_ref())?.to_be_bytes()); },
            5 => return self.bytes(ty, loc, &state.finish()),
            _ => return Err("unknown native hash operation".into()),
        }
        self.expect(ty, Kind::Hash)?;
        let reference = HeapRef::new(World::Work, u32::try_from(self.work.hashes.len()).map_err(|_| "HashState table overflow")?)?;
        self.work.hashes.push(state);
        self.pack(ty, loc, &[u64::from(reference.raw())])
    }
}
