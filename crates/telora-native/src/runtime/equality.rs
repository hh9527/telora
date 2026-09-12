use super::*;
use std::collections::BTreeSet;

impl Runtime {
    /// Compare the sealed value graph. Only descriptors enter the work list;
    /// objects and byte buffers remain borrowed from their original world.
    pub(super) fn equal(&self, left: &Value, right: &Value) -> Result<bool> {
        let mut pending = vec![(left.clone(), right.clone())];
        let mut visited = BTreeSet::new();
        while let Some((left, right)) = pending.pop() {
            self.validate(left.as_ref(), left.type_id())?;
            self.validate(right.as_ref(), right.type_id())?;
            if left.type_id() != right.type_id() { return Ok(false); }
            let ty = left.type_id();
            let layout = self.layout(ty)?;
            match layout.kind {
                Kind::Scalar => {
                    let a = self.scalar_bits(left.as_ref())?;
                    let b = self.scalar_bits(right.as_ref())?;
                    let equal = if self.type_info[ty.index()].kind == Some("Float") { f64::from_bits(a) == f64::from_bits(b) } else { a == b };
                    if !equal { return Ok(false); }
                }
                Kind::Metadata => { if self.represented_type(left.as_ref())? != self.represented_type(right.as_ref())? { return Ok(false); } }
                Kind::String => { if self.text(left.as_ref())?.as_str() != self.text(right.as_ref())?.as_str() { return Ok(false); } }
                Kind::Bytes => { if self.bytes_data(&left)? != self.bytes_data(&right)? { return Ok(false); } }
                Kind::Dyn => {
                    self.dynamic_value(&left)?;
                    self.dynamic_value(&right)?;
                    if left.words()[3] != right.words()[3] { return Ok(false); }
                }
                Kind::Function => {
                    self.function_id(&left)?;
                    self.function_id(&right)?;
                    for value in [&left, &right] {
                        let environment = (value.words()[2] >> 32) as u32;
                        self.object_words(Table::Environments, environment.checked_sub(1).ok_or("function value has no identity")?)?;
                    }
                    if left.words()[2] != right.words()[2] { return Ok(false); }
                }
                Kind::Array | Kind::Tuple | Kind::Record | Kind::Newtype | Kind::Dict | Kind::Enum => {
                    let identity = (ty, left.words()[2..].to_vec(), right.words()[2..].to_vec());
                    if !visited.insert(identity) { continue; }
                    match layout.kind {
                        Kind::Array => {
                            let len = self.array_len(&left)?;
                            if len != self.array_len(&right)? { return Ok(false); }
                            for index in (0..len).rev() { pending.push((self.array_get(&left, index)?.to_owned(), self.array_get(&right, index)?.to_owned())); }
                        }
                        Kind::Dict => {
                            let len = self.dict_len(&left)?;
                            if len != self.dict_len(&right)? { return Ok(false); }
                            for index in (0..len).rev() {
                                let (a, av) = self.dict_entry(&left, index)?;
                                let (b, bv) = self.dict_entry(&right, index)?;
                                if self.text(a)?.as_str() != self.text(b)?.as_str() { return Ok(false); }
                                pending.push((av.to_owned(), bv.to_owned()));
                            }
                        }
                        Kind::Enum => {
                            if self.enum_tag(&left)? != self.enum_tag(&right)? { return Ok(false); }
                            match (self.enum_payload(&left)?, self.enum_payload(&right)?) {
                                (Some(a), Some(b)) => pending.push((a.to_owned(), b.to_owned())),
                                (None, None) => {},
                                _ => return Err("sealed enum payload mismatch".into()),
                            }
                        }
                        _ => for index in (0..layout.fields.len()).rev() { pending.push((self.field(&left, index)?.to_owned(), self.field(&right, index)?.to_owned())); },
                    }
                }
                // These require their explicit identity/resource contracts.
                _ => return Err("native equality is not linked for this resource type".into()),
            }
        }
        Ok(true)
    }
}
