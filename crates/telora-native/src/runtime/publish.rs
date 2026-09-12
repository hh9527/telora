use super::*;
use std::collections::BTreeMap;

#[derive(Default)]
struct Copies {
    objects: BTreeMap<(Table, u32), u32>,
    strings: BTreeMap<u32, u32>,
    bytes: BTreeMap<u32, u32>,
    regexes: BTreeMap<u32, u32>,
    hashes: BTreeMap<u32, u32>,
    tests: BTreeMap<u32, u32>,
    blames: BTreeMap<u32, u32>,
}
impl Runtime {
    /// Publish initialization atomically, preserving aliases in the whole root
    /// set. Only after all copies succeed do we retire the initialize world.
    pub fn publish(&mut self, roots: &[Value]) -> Result<Vec<Value>> {
        if self.published {
            return Err("main world is already sealed".into());
        }
        let mut target = Tables::default();
        let demand_roots = self.demand_roots()?;
        let mut copies = Copies::default();
        let mut result = Vec::with_capacity(roots.len());
        for root in roots.iter().chain(&demand_roots) {
            self.validate(root.as_ref(), root.type_id())?;
            let mut words = root.words.to_vec();
            self.copy_value(&mut words, &mut target, &mut copies, 0)?;
            result.push(Value {
                arena: 0,
                words: words.into_boxed_slice(),
            });
        }
        let identity = NEXT_ARENA
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map_err(|_| "arena identity overflow")?;
        for root in &mut result {
            root.arena = identity;
        }
        self.main = target;
        self.work = Tables::default();
        self.identity = identity; // all escaped initialize descriptors become stale
        self.published = true;
        let demands = result.split_off(roots.len());
        self.publish_demands(demands);
        Ok(result)
    }
    fn copy_value(
        &self,
        words: &mut [u64],
        target: &mut Tables,
        copies: &mut Copies,
        depth: usize,
    ) -> Result<()> {
        if depth > 512 {
            return Err("native publication nesting limit".into());
        }
        if words.len() < 2 {
            return Err("truncated published value".into());
        }
        let ty = TypeId((words[1] >> 32) as u32);
        let layout = self.layout(ty)?;
        self.validate(
            ValueRef {
                arena: self.identity,
                words,
            },
            ty,
        )?;
        match layout.kind {
            Kind::Scalar => {}
            Kind::Bytes => {
                let value = Value {
                    arena: self.identity,
                    words: words.to_vec().into_boxed_slice(),
                };
                self.bytes_data(&value)?;
                let old = words[2] as u32;
                let id = if let Some(&id) = copies.bytes.get(&old) {
                    id
                } else {
                    let reference = HeapRef::from_raw(old);
                    let bytes = self.tables(reference).bytes.get(reference.slot())?;
                    self.charge_allocation(bytes.len(), 1, std::mem::size_of::<RawStringItem>())?;
                    let id = HeapRef::new(World::Main, target.bytes.push(bytes)?)?.raw();
                    copies.bytes.insert(old, id);
                    id
                };
                words[2] = (words[2] & 0xffff_ffff_0000_0000) | u64::from(id);
            }
            Kind::Metadata => {
                self.represented_type(ValueRef {
                    arena: self.identity,
                    words,
                })?;
            }
            Kind::Function => {
                let environment = (words[2] >> 32) as u32;
                {
                    let raw = environment.checked_sub(1).ok_or("function value has no identity")?;
                    let id = self.copy_object(
                        Table::Environments,
                        raw,
                        None,
                        target,
                        copies,
                        depth + 1,
                    )?;
                    let encoded = id.checked_add(1).ok_or("environment ID overflow")?;
                    words[2] = (words[2] & 0xffff_ffff) | (u64::from(encoded) << 32);
                }
            }
            Kind::Tuple if layout.fields.is_empty() => {}
            Kind::String => {
                // Validate both inline and heap string encodings before copying.
                self.text(ValueRef {
                    arena: self.identity,
                    words,
                })?;
                if words[2] as u8 == 1 {
                    let old = (words[2] >> 32) as u32;
                    let id = if let Some(&id) = copies.strings.get(&old) {
                        id
                    } else {
                        let bytes = self.string_bytes(old)?;
                        self.charge_allocation(bytes.len(), 1, std::mem::size_of::<RawStringItem>())?;
                        let id = HeapRef::new(World::Main, target.strings.push(bytes)?)?.raw();
                        copies.strings.insert(old, id);
                        id
                    };
                    words[2] = (words[2] & 0xffff_ffff) | (u64::from(id) << 32);
                }
            }
            Kind::Tuple | Kind::Record | Kind::Newtype => {
                let id = self.copy_object(
                    if layout.kind == Kind::Newtype { Table::Newtypes } else { Table::Records },
                    words[2] as u32,
                    Some(ty),
                    target,
                    copies,
                    depth + 1,
                )?;
                words[2] = u64::from(id);
            }
            Kind::Array => {
                let array = Value {
                    arena: self.identity,
                    words: words.to_vec().into_boxed_slice(),
                };
                self.array_range(&array)?;
                let id = self.copy_object(
                    Table::Arrays,
                    words[2] as u32,
                    Some(layout.arguments[0]),
                    target,
                    copies,
                    depth + 1,
                )?;
                words[2] = (words[2] & 0xffff_ffff_0000_0000) | u64::from(id);
            }
            Kind::Dict => {
                let dict = Value {
                    arena: self.identity,
                    words: words.to_vec().into_boxed_slice(),
                };
                self.dict_parts(&dict)?;
                let keys = self.copy_object(
                    Table::Arrays,
                    words[2] as u32,
                    None,
                    target,
                    copies,
                    depth + 1,
                )?;
                let values = self.copy_object(
                    Table::Arrays,
                    words[3] as u32,
                    Some(layout.arguments[0]),
                    target,
                    copies,
                    depth + 1,
                )?;
                words[2] = (words[2] & 0xffff_ffff_0000_0000) | u64::from(keys);
                words[3] = u64::from(values);
            }
            Kind::Enum => {
                let value = Value {
                    arena: self.identity,
                    words: words.to_vec().into_boxed_slice(),
                };
                let tag = self.enum_tag(&value)?;
                self.enum_payload(&value)?;
                let variant = &layout.variants[tag as usize];
                if let Some(payload) = variant.payload {
                    match variant.storage {
                        "full_value" => self.copy_value(
                            &mut words[3..3 + self.layout(payload)?.words],
                            target,
                            copies,
                            depth + 1,
                        )?,
                        "heap_id" => {
                            words[3] = u64::from(self.copy_object(
                                Table::Values,
                                words[3] as u32,
                                Some(payload),
                                target,
                                copies,
                                depth + 1,
                            )?);
                        }
                        _ => return Err("enum payload is not materializable".into()),
                    }
                }
            }
            Kind::Dyn => {
                let value = Value {
                    arena: self.identity,
                    words: words.to_vec().into_boxed_slice(),
                };
                let payload = self.dynamic_value(&value)?.type_id();
                words[3] = u64::from(self.copy_object(
                    Table::Values,
                    words[3] as u32,
                    Some(payload),
                    target,
                    copies,
                    depth + 1,
                )?);
            }
            Kind::Test => {
                let value = Value { arena: self.identity, words: words.to_vec().into_boxed_slice() };
                let old = words[2] as u32;
                let description = self.test_description(&value)?;
                words[2] = u64::from(if let Some(&id) = copies.tests.get(&old) { id } else {
                    let mut description = description.clone();
                    for input in &mut description.inputs { self.copy_value(input, target, copies, depth + 1)?; }
                    let id = HeapRef::new(World::Main, u32::try_from(target.tests.len()).map_err(|_| "Test table overflow")?)?.raw();
                    target.tests.push(description);
                    copies.tests.insert(old, id);
                    id
                });
            }
            Kind::Blame => {
                let value = Value { arena: self.identity, words: words.to_vec().into_boxed_slice() };
                let old = words[2] as u32;
                let blame = self.blame_object(&value)?;
                words[2] = u64::from(if let Some(&id) = copies.blames.get(&old) { id } else {
                    let mut blame = blame.clone();
                    self.copy_value(&mut blame.message, target, copies, depth + 1)?;
                    let id = HeapRef::new(World::Main, u32::try_from(target.blames.len()).map_err(|_| "Blame table overflow")?)?.raw();
                    target.blames.push(blame);
                    copies.blames.insert(old, id);
                    id
                });
            }
            Kind::Hash => {
                let value = Value { arena: self.identity, words: words.to_vec().into_boxed_slice() };
                let state = self.hash_state(&value)?;
                let old = words[2] as u32;
                words[2] = u64::from(if let Some(&id) = copies.hashes.get(&old) { id } else {
                    let id = HeapRef::new(World::Main, u32::try_from(target.hashes.len()).map_err(|_| "HashState table overflow")?)?.raw();
                    target.hashes.push(state.clone());
                    copies.hashes.insert(old, id);
                    id
                });
            }
            Kind::Regex => {
                let value = Value {
                    arena: self.identity,
                    words: words.to_vec().into_boxed_slice(),
                };
                let regex = self.regex_object(&value)?;
                let old = words[2] as u32;
                words[2] = u64::from(if let Some(&id) = copies.regexes.get(&old) {
                    id
                } else {
                    let id = HeapRef::new(
                        World::Main,
                        u32::try_from(target.regexes.len()).map_err(|_| "Regex table overflow")?,
                    )?
                    .raw();
                    target.regexes.push(regex.clone());
                    copies.regexes.insert(old, id);
                    id
                });
            }
            Kind::Format => {
                words[2] = u64::from(self.copy_object(
                    Table::Formats,
                    words[2] as u32,
                    Some(ty),
                    target,
                    copies,
                    depth + 1,
                )?);
            }
            Kind::Other => {
                return Err(format!(
                    "native publication unsupported TypeId {}",
                    ty.index()
                ));
            }
        }
        Ok(())
    }
    fn copy_object(
        &self,
        table: Table,
        old: u32,
        ty: Option<TypeId>,
        target: &mut Tables,
        copies: &mut Copies,
        depth: usize,
    ) -> Result<u32> {
        if let Some(&id) = copies.objects.get(&(table, old)) {
            return Ok(id);
        }
        let original = self.object_words(table, old)?;
        self.charge_allocation(original.len(), 8, std::mem::size_of::<WordItem>())?;
        let mut words = original.to_vec();
        let slot = match table {
            Table::Records => target.records.push(vec![])?,
            Table::Newtypes => target.newtypes.push(vec![])?,
            Table::Arrays => target.arrays.push(vec![])?,
            Table::Values => target.values.push(vec![])?,
            Table::Environments => target.environments.push(vec![])?,
            Table::Formats => target.formats.push(vec![])?,
        };
        let id = HeapRef::new(World::Main, slot)?.raw();
        copies.objects.insert((table, old), id); // register before traversing cycles
        match table {
            Table::Environments | Table::Formats => {
                let mut remaining = if matches!(table, Table::Formats) {
                    words.get_mut(1..).ok_or("truncated format node")?
                } else {
                    words.as_mut_slice()
                };
                while !remaining.is_empty() {
                    let header = remaining.get(1).ok_or("truncated capture header")?;
                    let width = self.layout(TypeId((header >> 32) as u32))?.words;
                    if width > remaining.len() {
                        return Err("truncated capture value".into());
                    }
                    let (value, rest) = remaining.split_at_mut(width);
                    self.copy_value(value, target, copies, depth)?;
                    remaining = rest;
                }
            }
            Table::Values => {
                let ty = ty.ok_or("boxed value requires its solved type")?;
                self.validate(
                    ValueRef {
                        arena: self.identity,
                        words: &words,
                    },
                    ty,
                )?;
                self.copy_value(&mut words, target, copies, depth)?;
            }
            Table::Records | Table::Newtypes => {
                let layout = self.layout(ty.ok_or("record copy needs type")?)?;
                let mut end = 0;
                for &(field, offset) in &layout.fields {
                    let size = self.layout(field)?.words;
                    if offset != end {
                        return Err("non-contiguous record layout".into());
                    }
                    end = offset.checked_add(size).ok_or("record size overflow")?;
                    let value = words.get_mut(offset..end).ok_or("truncated record")?;
                    if (value[1] >> 32) as u32 != field.0 {
                        return Err("record field type mismatch".into());
                    }
                    self.copy_value(value, target, copies, depth)?;
                }
                if end != words.len() {
                    return Err("record extent mismatch".into());
                }
            }
            Table::Arrays if !words.is_empty() => {
                let stride = match ty {
                    Some(t) => self.layout(t)?.words,
                    None => 4, // Dict's String column
                };
                if words.len() % stride != 0 {
                    return Err("array extent mismatch".into());
                }
                for value in words.chunks_mut(stride) {
                    let actual = TypeId((value[1] >> 32) as u32);
                    if let Some(t) = ty {
                        if actual != t {
                            return Err("array element type mismatch".into());
                        }
                    } else {
                        self.expect(actual, Kind::String)?;
                    }
                    self.copy_value(value, target, copies, depth)?;
                }
            }
            Table::Arrays => {}
        }
        match table {
            Table::Records => target.records.entries[slot as usize].words = words,
            Table::Newtypes => target.newtypes.entries[slot as usize].words = words,
            Table::Arrays => target.arrays.entries[slot as usize].words = words,
            Table::Values => target.values.entries[slot as usize].words = words,
            Table::Environments => target.environments.entries[slot as usize].words = words,
            Table::Formats => target.formats.entries[slot as usize].words = words,
        }
        Ok(id)
    }
}
