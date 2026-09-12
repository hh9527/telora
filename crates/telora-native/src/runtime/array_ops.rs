use super::*;

impl Runtime {
    // Admit the final backing before building any elements. Empty arrays may
    // carry Never, whose value layout must not be requested.
    pub(super) fn array_buffer(&self, element: TypeId, length: usize) -> Result<Vec<u64>> {
        u32::try_from(length).map_err(|_| "array length overflow")?;
        let stride = if length == 0 { 0 } else { self.layout(element)?.words };
        let capacity = length.checked_mul(stride).ok_or("array word count overflow")?;
        self.charge_allocation(capacity, 8, std::mem::size_of::<WordItem>())?;
        Ok(Vec::with_capacity(capacity))
    }

    pub(crate) fn array_concat(&mut self, ty: TypeId, loc: Location, arrays: &[Value]) -> Result<Value> {
        let element = self.expect(ty, Kind::Array)?.arguments[0];
        let mut length = 0u32;
        for array in arrays {
            self.validate(array.as_ref(), ty)?;
            let (_, start, end, actual) = self.array_range(array)?;
            if actual != element { return Err("array spread element differs from its sealed target".into()); }
            length = length.checked_add(end - start).ok_or("array spread length overflow")?;
        }
        let stride = if length == 0 { 0 } else { self.layout(element)?.words };
        let mut words = self.array_buffer(element, length as usize)?;
        for array in arrays {
            let (id, start, end, _) = self.array_range(array)?;
            let backing = self.object_words(Table::Arrays, id)?;
            words.extend_from_slice(&backing[start as usize * stride..end as usize * stride]);
        }
        let id = self.push_precharged_words(Table::Arrays, words)?;
        self.pack(ty, loc, &[u64::from(id), u64::from(length)])
    }

    pub(crate) fn array_operation(&mut self, ty: TypeId, loc: Location, operation: usize, inputs: &[Value]) -> Result<Value> {
        let input = &inputs[0];
        let count = self.array_len(input)?;
        let output = if operation == 3 { self.layout(ty)?.arguments[0] } else { ty };
        if operation == 3 && count != self.array_len(&inputs[1])? {
            return self.named_variant(ty, loc, "None", None);
        }
        let element = self.expect(output, Kind::Array)?.arguments[0];
        let length = match operation {
            0 | 3 => count,
            1 => {
                self.validate(inputs[1].as_ref(), element)?;
                count.checked_add(1).ok_or("array length overflow")?
            }
            2 => {
                let mut length = 0usize;
                for index in 0..count {
                    let value = self.array_get(input, index)?.to_owned();
                    length = length.checked_add(self.array_len(&value)?).ok_or("array length overflow")?;
                }
                length
            }
            _ => return Err("unknown array construction operation".into()),
        };
        let mut words = self.array_buffer(element, length)?;
        for index in 0..count {
            let value = self.array_get(input, index)?.to_owned();
            match operation {
                0 => {
                    let index_type = self.expect(element, Kind::Tuple)?.arguments[0];
                    let index = self.scalar(index_type, loc, index as u64)?;
                    words.extend_from_slice(self.aggregate(element, loc, &[index, value])?.words());
                }
                1 => words.extend_from_slice(value.words()),
                2 => {
                    for index in 0..self.array_len(&value)? {
                        words.extend_from_slice(self.array_get(&value, index)?.words());
                    }
                }
                3 => {
                    let right = self.array_get(&inputs[1], index)?.to_owned();
                    words.extend_from_slice(self.aggregate(element, loc, &[value, right])?.words());
                }
                _ => return Err("unknown array construction operation".into()),
            }
        }
        if operation == 1 { words.extend_from_slice(inputs[1].words()); }
        let id = self.push_precharged_words(Table::Arrays, words)?;
        let result = self.pack(output, loc, &[u64::from(id), length as u64])?;
        if operation == 3 { self.named_variant(ty, loc, "Some", Some(&result)) } else { Ok(result) }
    }
}
