use super::*;

// Hash algorithm is local to this experiment; the candidate bucket ABI only
// requires a stored u32 hash and string-content equality on collisions.
fn hash(text: &str) -> u32 {
    text.bytes()
        .fold(2166136261, |h, b| (h ^ u32::from(b)).wrapping_mul(16777619))
}
impl Arena {
    pub fn dict(&mut self, ty: TypeId, loc: Location, pairs: &[(Value, Value)]) -> Result<Value> {
        let element = self.expect(ty, Kind::Dict)?.arguments[0];
        let value_words = match self.layout(element) {
            Ok(l) => l.words,
            Err(_) if pairs.is_empty() => 0,
            Err(e) => return Err(e),
        };
        let capacity = if pairs.is_empty() {
            0
        } else {
            u32::try_from(pairs.len())
                .map_err(|_| "dictionary capacity overflow")?
                .checked_next_power_of_two()
                .ok_or("dictionary capacity overflow")?
        };
        let buckets = capacity
            .checked_mul(2)
            .ok_or("dictionary bucket overflow")?
            .max(1);
        let stride = 4 + value_words;
        let bytes = Storage::Dictionary {
            entry_stride: stride as u64 * 8,
            empty_only: value_words == 0,
        }
        .allocation_bytes(Extent::Dictionary {
            length: 0,
            capacity,
            buckets,
        })?;
        let mut words =
            vec![0; usize::try_from(bytes / 8).map_err(|_| "dictionary size overflow")?];
        let bucket_start = 2 + capacity as usize * stride;
        let mut length = 0u32;
        for (key, value) in pairs {
            self.validate(key.as_ref(), key.type_id())?;
            self.expect(key.type_id(), Kind::String)?;
            self.validate(value.as_ref(), element)?;
            let text = self.text(key.as_ref())?;
            let h = hash(text.as_str());
            let mut slot = h as usize & (buckets as usize - 1);
            loop {
                let bucket = words[bucket_start + slot];
                let entry = (bucket >> 32) as u32;
                if entry == 0 {
                    let offset = 2 + length as usize * stride;
                    words[offset..offset + 4].copy_from_slice(&key.words);
                    words[offset + 4..offset + stride].copy_from_slice(&value.words);
                    words[bucket_start + slot] = u64::from(h) | (u64::from(length + 1) << 32);
                    length += 1;
                    break;
                }
                if bucket as u32 == h {
                    let offset = 2 + (entry - 1) as usize * stride;
                    let stored = ValueRef {
                        arena: self.identity,
                        words: &words[offset..offset + 4],
                    };
                    if self.text(stored)?.as_str() == text.as_str() {
                        // Preserve original key/source and insertion order;
                        // last value wins, with the replacement value's origin.
                        words[offset + 4..offset + stride].copy_from_slice(&value.words);
                        break;
                    }
                }
                slot = (slot + 1) & (buckets as usize - 1);
            }
        }
        words[0] = u64::from(length) | (u64::from(capacity) << 32);
        words[1] = u64::from(buckets);
        let id = self.dicts.push(&words)?;
        self.pack(ty, loc, &[u64::from(id)])
    }
    fn dict_parts<'a>(&'a self, value: &Value) -> Result<(&'a [u64], usize, usize, usize, TypeId)> {
        self.validate(value.as_ref(), value.type_id())?;
        let element = self.expect(value.type_id(), Kind::Dict)?.arguments[0];
        let words = self.dicts.get(value.words[2] as u32)?;
        if words.len() < 2 {
            return Err("truncated dictionary".into());
        }
        let length = words[0] as u32;
        let capacity = (words[0] >> 32) as u32;
        let buckets = words[1] as u32;
        let value_words = match self.layout(element) {
            Ok(l) => l.words,
            Err(_) if length == 0 => 0,
            Err(e) => return Err(e),
        };
        let stride = 4 + value_words;
        let bytes = Storage::Dictionary {
            entry_stride: stride as u64 * 8,
            empty_only: value_words == 0,
        }
        .allocation_bytes(Extent::Dictionary {
            length,
            capacity,
            buckets,
        })?;
        if bytes / 8 != words.len() as u64 {
            return Err("dictionary storage extent mismatch".into());
        }
        Ok((
            words,
            length as usize,
            2 + capacity as usize * stride,
            stride,
            element,
        ))
    }
    pub fn dict_len(&self, value: &Value) -> Result<usize> {
        Ok(self.dict_parts(value)?.1)
    }
    pub fn dict_get<'a>(&'a self, value: &Value, key: &Value) -> Result<Option<ValueRef<'a>>> {
        self.validate(key.as_ref(), key.type_id())?;
        let text = self.text(key.as_ref())?;
        let (words, length, bucket_start, stride, element) = self.dict_parts(value)?;
        let buckets = words.len() - bucket_start;
        let h = hash(text.as_str());
        let mut slot = h as usize & (buckets - 1);
        for _ in 0..buckets {
            let bucket = words[bucket_start + slot];
            let entry = (bucket >> 32) as usize;
            if entry == 0 {
                return Ok(None);
            }
            if entry > length {
                return Err("invalid dictionary bucket index".into());
            }
            if bucket as u32 == h {
                let offset = 2 + (entry - 1) * stride;
                let stored = ValueRef {
                    arena: self.identity,
                    words: &words[offset..offset + 4],
                };
                if self.text(stored)?.as_str() == text.as_str() {
                    let result = ValueRef {
                        arena: self.identity,
                        words: &words[offset + 4..offset + stride],
                    };
                    self.validate(result, element)?;
                    return Ok(Some(result));
                }
            }
            slot = (slot + 1) & (buckets - 1);
        }
        Err("dictionary has no terminating empty bucket".into())
    }
    pub fn dict_entry<'a>(
        &'a self,
        value: &Value,
        index: usize,
    ) -> Result<(ValueRef<'a>, ValueRef<'a>)> {
        let (words, length, _, stride, element) = self.dict_parts(value)?;
        if index >= length {
            return Err("dictionary entry out of bounds".into());
        }
        let offset = 2 + index * stride;
        let key = ValueRef {
            arena: self.identity,
            words: &words[offset..offset + 4],
        };
        let value = ValueRef {
            arena: self.identity,
            words: &words[offset + 4..offset + stride],
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
