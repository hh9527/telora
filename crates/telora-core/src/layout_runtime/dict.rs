use super::*;

impl Arena {
    pub fn dict(&mut self, ty: TypeId, loc: Location, pairs: &[(Value, Value)]) -> Result<Value> {
        let element = self.expect(ty, Kind::Dict)?.arguments[0];
        let mut sorted = Vec::with_capacity(pairs.len());
        for (key, value) in pairs {
            self.validate(key.as_ref(), key.type_id())?;
            self.expect(key.type_id(), Kind::String)?;
            self.validate(value.as_ref(), element)?;
            sorted.push((self.text(key.as_ref())?, key, value));
        }
        // Stable sorting: first key/source and last value win for duplicate keys.
        sorted.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        let mut keys = Vec::new();
        let mut values = Vec::new();
        let mut previous: Option<&str> = None;
        for (text, key, value) in &sorted {
            if previous == Some(text.as_str()) {
                let start = values.len() - value.words.len();
                values[start..].copy_from_slice(&value.words);
            } else {
                keys.extend_from_slice(&key.words);
                values.extend_from_slice(&value.words);
            }
            previous = Some(text.as_str());
        }
        let len = u32::try_from(keys.len() / 4).map_err(|_| "dictionary length overflow")?;
        let keys_id = self.arrays.push(keys)?;
        let values_id = self.arrays.push(values)?;
        self.pack(
            ty,
            loc,
            &[
                u64::from(keys_id) | (u64::from(len) << 32),
                u64::from(values_id),
            ],
        )
    }
    fn dict_parts<'a>(
        &'a self,
        value: &Value,
    ) -> Result<(&'a [u64], &'a [u64], usize, usize, TypeId)> {
        self.validate(value.as_ref(), value.type_id())?;
        let element = self.expect(value.type_id(), Kind::Dict)?.arguments[0];
        let length = (value.words[2] >> 32) as usize;
        if value.words[3] >> 32 != 0 {
            return Err("invalid dictionary reserved bits".into());
        }
        let keys = self.arrays.get(value.words[2] as u32)?;
        let values = self.arrays.get(value.words[3] as u32)?;
        let stride = match self.layout(element) {
            Ok(l) => l.words,
            Err(_) if length == 0 => 0,
            Err(e) => return Err(e),
        };
        if keys.len() != length.checked_mul(4).ok_or("dictionary size overflow")?
            || values.len()
                != length
                    .checked_mul(stride)
                    .ok_or("dictionary size overflow")?
        {
            return Err("dictionary column length mismatch".into());
        }
        Ok((keys, values, length, stride, element))
    }
    pub fn dict_len(&self, value: &Value) -> Result<usize> {
        Ok(self.dict_parts(value)?.2)
    }
    pub fn dict_get<'a>(&'a self, value: &Value, key: &Value) -> Result<Option<ValueRef<'a>>> {
        self.validate(key.as_ref(), key.type_id())?;
        let text = self.text(key.as_ref())?;
        let (keys, values, length, stride, element) = self.dict_parts(value)?;
        let (mut low, mut high) = (0, length);
        while low < high {
            let mid = low + (high - low) / 2;
            let stored = ValueRef {
                arena: self.identity,
                words: &keys[mid * 4..(mid + 1) * 4],
            };
            match self.text(stored)?.as_str().cmp(text.as_str()) {
                std::cmp::Ordering::Less => low = mid + 1,
                std::cmp::Ordering::Greater => high = mid,
                std::cmp::Ordering::Equal => {
                    let result = ValueRef {
                        arena: self.identity,
                        words: &values[mid * stride..(mid + 1) * stride],
                    };
                    self.validate(result, element)?;
                    return Ok(Some(result));
                }
            }
        }
        Ok(None)
    }
    pub fn dict_entry<'a>(
        &'a self,
        value: &Value,
        index: usize,
    ) -> Result<(ValueRef<'a>, ValueRef<'a>)> {
        let (keys, values, length, stride, element) = self.dict_parts(value)?;
        if index >= length {
            return Err("dictionary entry out of bounds".into());
        }
        let key = ValueRef {
            arena: self.identity,
            words: &keys[index * 4..(index + 1) * 4],
        };
        let value = ValueRef {
            arena: self.identity,
            words: &values[index * stride..(index + 1) * stride],
        };
        self.validate(key, key.type_id())?;
        self.expect(key.type_id(), Kind::String)?;
        self.validate(value, element)?;
        Ok((key, value))
    }
    pub fn dict_insert(
        &mut self,
        dict: &Value,
        key: &Value,
        value: &Value,
        loc: Location,
    ) -> Result<Value> {
        let mut pairs = (0..self.dict_len(dict)?)
            .map(|i| {
                self.dict_entry(dict, i)
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
            })
            .collect::<Result<Vec<_>>>()?;
        pairs.push((key.clone(), value.clone()));
        self.dict(dict.type_id(), loc, &pairs)
    }
    pub fn dict_remove(&mut self, dict: &Value, key: &Value, loc: Location) -> Result<Value> {
        let remove = self.text(key.as_ref())?;
        let pairs = (0..self.dict_len(dict)?)
            .filter_map(|i| {
                let result = (|| {
                    let (k, v) = self.dict_entry(dict, i)?;
                    Ok((self.text(k)?.as_str() != remove.as_str())
                        .then(|| (k.to_owned(), v.to_owned())))
                })();
                match result {
                    Ok(None) => None,
                    Ok(Some(pair)) => Some(Ok(pair)),
                    Err(e) => Some(Err(e)),
                }
            })
            .collect::<Result<Vec<_>>>()?;
        self.dict(dict.type_id(), loc, &pairs)
    }
}
