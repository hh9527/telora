use super::*;
use std::collections::BTreeSet;
enum Task {
    Compare(Value, Value),
    Children { left: Value, right: Value, kind: Kind, index: usize, length: usize },
}

impl Runtime {
    /// Compare the sealed value graph. Only descriptors enter the work list;
    /// objects and byte buffers remain borrowed from their original world.
    #[cfg(test)]
    pub(super) fn equal(&self, left: &Value, right: &Value) -> Result<bool> {
        self.equal_metered(left, right, &mut |_| Ok(()))
    }
    pub(super) fn equal_metered(&self, left: &Value, right: &Value, charge: &mut dyn FnMut(u64) -> Result<()>) -> Result<bool> {
        let mut pending = vec![Task::Compare(left.clone(), right.clone())];
        let mut visited = BTreeSet::new();
        while let Some(task) = pending.pop() {
            charge(1)?;
            let (left, right) = match task {
                Task::Compare(left, right) => (left, right),
                Task::Children { left, right, kind, index, length } => {
                    if index == length { continue; }
                    let (a, b) = match kind {
                        Kind::Array => (self.array_get(&left, index)?, self.array_get(&right, index)?),
                        Kind::Dict => {
                            let (ak, av) = self.dict_entry(&left, index)?;
                            let (bk, bv) = self.dict_entry(&right, index)?;
                            charge((self.byte_span_len(ak)? as u64).checked_add(self.byte_span_len(bk)? as u64).ok_or("equality byte count overflow")?)?;
                            if self.text(ak)?.as_str() != self.text(bk)?.as_str() { return Ok(false); }
                            (av, bv)
                        }
                        _ => (self.field(&left, index)?, self.field(&right, index)?),
                    };
                    let next = Task::Compare(a.to_owned(), b.to_owned());
                    pending.push(Task::Children { left, right, kind, index: index + 1, length });
                    pending.push(next);
                    continue;
                }
            };
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
                Kind::String | Kind::Bytes => {
                    charge((self.byte_span_len(left.as_ref())? as u64).checked_add(self.byte_span_len(right.as_ref())? as u64).ok_or("equality byte count overflow")?)?;
                    let equal = if layout.kind == Kind::String { self.text(left.as_ref())?.as_str() == self.text(right.as_ref())?.as_str() }
                        else { self.bytes_data(&left)? == self.bytes_data(&right)? };
                    if !equal { return Ok(false); }
                }
                Kind::Regex => { if !self.regex_equal(&left, &right, charge)? { return Ok(false); } }
                Kind::Hash => { if self.hash_state(&left)? != self.hash_state(&right)? { return Ok(false); } }
                Kind::Blame => {
                    self.blame_object(&left)?;
                    self.blame_object(&right)?;
                    if left.words()[2] != right.words()[2] { return Ok(false); }
                }
                Kind::Test => {
                    self.test_description(&left)?;
                    self.test_description(&right)?;
                    if left.words()[2] != right.words()[2] { return Ok(false); }
                }
                Kind::Format => {
                    if !visited.insert((ty, left.words()[2..].to_vec(), right.words()[2..].to_vec())) { continue; }
                    let (a, av) = self.format_parts(&left)?;
                    let (b, bv) = self.format_parts(&right)?;
                    if a != b { return Ok(false); }
                    if a == 3 {
                        if self.scalar_bits(av[0].as_ref())? != self.scalar_bits(bv[0].as_ref())? { return Ok(false); }
                    } else { pending.extend(av.into_iter().zip(bv).map(|(a, b)| Task::Compare(a, b))); }
                }
                Kind::Dyn => {
                    self.dynamic_value(&left)?;
                    self.dynamic_value(&right)?;
                    if left.words()[3] != right.words()[3] { return Ok(false); }
                }
                Kind::Function => {
                    let left = self.resolve_function_metered(left.as_ref(), charge)?;
                    let right = self.resolve_function_metered(right.as_ref(), charge)?;
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
                            pending.push(Task::Children { left, right, kind: Kind::Array, index: 0, length: len });
                        }
                        Kind::Dict => {
                            let len = self.dict_len(&left)?;
                            if len != self.dict_len(&right)? { return Ok(false); }
                            pending.push(Task::Children { left, right, kind: Kind::Dict, index: 0, length: len });
                        }
                        Kind::Enum => {
                            if self.enum_tag(&left)? != self.enum_tag(&right)? { return Ok(false); }
                            match (self.enum_payload(&left)?, self.enum_payload(&right)?) {
                                (Some(a), Some(b)) => pending.push(Task::Compare(a.to_owned(), b.to_owned())),
                                (None, None) => {},
                                _ => return Err("sealed enum payload mismatch".into()),
                            }
                        }
                        _ => pending.push(Task::Children { left, right, kind: layout.kind, index: 0, length: layout.fields.len() }),
                    }
                }
                // These require their explicit identity/resource contracts.
                _ => return Err("native equality is not linked for this resource type".into()),
            }
        }
        Ok(true)
    }
}
