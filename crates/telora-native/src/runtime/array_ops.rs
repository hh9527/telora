use super::*;

impl Runtime {
    pub(crate) fn array_concat(&mut self, ty: TypeId, loc: Location, arrays: &[Value]) -> Result<Value> {
        let element = self.expect(ty, Kind::Array)?.arguments[0];
        let mut length = 0u32;
        let mut ranges = vec![];
        for array in arrays {
            self.validate(array.as_ref(), ty)?;
            let (id, start, end, actual) = self.array_range(array)?;
            if actual != element { return Err("array spread element differs from its sealed target".into()); }
            length = length.checked_add(end - start).ok_or("array spread length overflow")?;
            if start != end { ranges.push((id, start as usize, end as usize)); }
        }
        let stride = if length == 0 { 0 } else { self.layout(element)?.words };
        let capacity = (length as usize).checked_mul(stride).ok_or("array spread word count overflow")?;
        self.charge_allocation(capacity, 8, std::mem::size_of::<WordItem>())?;
        let mut words = Vec::with_capacity(capacity);
        for (id, start, end) in ranges {
            let backing = self.object_words(Table::Arrays, id)?;
            words.extend_from_slice(&backing[start * stride..end * stride]);
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
        let mut values = Vec::new();
        for index in 0..count {
            let value = self.array_get(input, index)?.to_owned();
            match operation {
                0 => {
                    let index_type = self.expect(element, Kind::Tuple)?.arguments[0];
                    let index = self.scalar(index_type, loc, index as u64)?;
                    values.push(self.aggregate(element, loc, &[index, value])?);
                }
                1 => values.push(value),
                2 => {
                    for index in 0..self.array_len(&value)? {
                        values.push(self.array_get(&value, index)?.to_owned());
                    }
                }
                3 => {
                    let right = self.array_get(&inputs[1], index)?.to_owned();
                    values.push(self.aggregate(element, loc, &[value, right])?);
                }
                _ => return Err("unknown array construction operation".into()),
            }
        }
        if operation == 1 { values.push(inputs[1].clone()); }
        let result = self.array(output, loc, &values)?;
        if operation == 3 { self.named_variant(ty, loc, "Some", Some(&result)) } else { Ok(result) }
    }
}
