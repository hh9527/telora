//! Explicit checked casts validate the complete representation before invoking
//! any user checker. All source and target identities come from sealed layouts.
use super::*;
use super::codec::{Codec, DecodeFailure};

struct Visit { value: Value, target: TypeId, path: String }
enum Task {
    Visit(Visit),
    Build { original: Value, target: TypeId, children: Vec<Value>, names: Vec<(String, Option<Value>)>, tag: Option<u32> },
}

impl Codec<'_> {
    pub(super) fn checked_cast(&mut self, result: TypeId, input: &Value, origin: crate::abi::Origin) -> Result<Option<Value>> {
        let shape = self.runtime()?.layout(result)?;
        if !shape.result || shape.arguments.len() != 2 { return Err("cast has no sealed Result signature".into()); }
        let target = shape.arguments[0];
        let string = shape.arguments[1];
        self.runtime()?.expect(string, Kind::String)?;
        let outcome = self.cast_pass(target, input, string, true)
            .and_then(|_| self.cast_pass(target, input, string, false));
        match outcome {
            Ok(value) => self.runtime_mut()?.named_variant(result, input.location(), "Ok", Some(&value)).map(Some),
            Err(DecodeFailure::Rejected(message, subjects)) => {
                let loc = subjects.first().map(|origin| origin.words()).unwrap_or(input.location());
                let message = self.runtime_mut()?.owned_string(string, loc, message)?;
                self.runtime_mut()?.named_variant(result, input.location(), "Err", Some(&message)).map(Some)
            }
            Err(DecodeFailure::Blame(blame)) => {
                let (message, subjects) = self.runtime()?.blame_diagnostic(&blame)?;
                self.context.fail_with_subjects(message, origin, subjects);
                Ok(None)
            }
            Err(DecodeFailure::Failed) => Ok(None),
            Err(DecodeFailure::Runtime(message)) => Err(message),
        }
    }

    fn cast_pass(&mut self, target: TypeId, input: &Value, string: TypeId, validating: bool) -> std::result::Result<Value, DecodeFailure> {
        let mut pending = vec![Task::Visit(Visit { value: input.clone(), target, path: "value".into() })];
        let mut output: Vec<Value> = vec![];
        while let Some(task) = pending.pop() {
            if self.context.consume_fuel(1, input.origin()) != crate::abi::Status::Success { return Err(DecodeFailure::Failed); }
            let Visit { value, target, path } = match task {
                Task::Visit(visit) => visit,
                Task::Build { original, target, children, names, tag } => {
                    let values = output.split_off(output.len() - children.len());
                    if validating { output.push(original); continue; }
                    let rt = self.runtime()?;
                    let shape = rt.layout(target)?;
                    let source = rt.layout(original.type_id())?;
                    let kind = shape.kind;
                    let check = shape.nominal;
                    let reusable = source.kind == kind && original.words.len() == shape.words
                        && tag.is_none_or(|tag| source.variants[tag as usize].storage == shape.variants[tag as usize].storage)
                        && values.iter().zip(&children).all(|(left, right)| left.words == right.words);
                    let converted = if reusable {
                        let mut value = original.clone();
                        value.words[1] = (value.words[1] & 0xffff_ffff) | (u64::from(target.raw()) << 32);
                        value
                    } else {
                        let rt = self.runtime_mut()?;
                        match kind {
                            Kind::Array => rt.array(target, original.location(), &values)?,
                            Kind::Tuple | Kind::Record | Kind::Newtype => rt.aggregate(target, original.location(), &values)?,
                            Kind::Dict => {
                                let mut pairs = vec![];
                                for ((name, key), value) in names.into_iter().zip(values) {
                                    let key = match key { Some(key) => key, None => rt.string(string, original.location(), &name)? };
                                    pairs.push((key, value));
                                }
                                rt.dict(target, original.location(), &pairs)?
                            }
                            Kind::Enum => rt.enum_value(target, original.location(), tag.ok_or("cast enum has no tag")?, values.first())?,
                            _ => return Err("cast build has no sealed representation".into()),
                        }
                    };
                    if check {
                        let argument = if kind == Kind::Newtype { self.runtime()?.field(&converted, 0)?.to_owned() } else { converted.clone() };
                        self.check(target, 0, &argument)?;
                    }
                    output.push(converted);
                    continue;
                }
            };
            let rt = self.runtime()?;
            rt.validate(value.as_ref(), value.type_id())?;
            if value.type_id() == target { output.push(value); continue; }
            if rt.type_info.get(target.index()).is_some_and(|info| info.kind == Some("Never")) {
                return Err(DecodeFailure::Rejected(format!("{path}: representation does not match cast target"), vec![value.origin()]));
            }
            let source = rt.layout(value.type_id())?;
            let target_shape = rt.layout(target)?;
            let blocked_identity = source.nominal || source.unchecked.is_some_and(|owner| owner != target);
            let mut mismatch = blocked_identity || target_shape.unchecked.is_some();
            let mut visits = vec![];
            let mut names = vec![];
            let mut tag = None;
            if !mismatch {
                match (source.kind, target_shape.kind) {
                    (Kind::Record | Kind::Dict, Kind::Record) => {
                        let count = if source.kind == Kind::Dict { rt.dict_len(&value)? } else { source.fields.len() };
                        mismatch = count != target_shape.fields.len();
                        if !mismatch {
                            for (index, name) in target_shape.field_names.iter().enumerate() {
                                let child = if source.kind == Kind::Record {
                                    source.field_names.iter().position(|field| field == name).map(|index| rt.field(&value, index)).transpose()?
                                } else {
                                    let mut found = None;
                                    for index in 0..count {
                                        let (key, child) = rt.dict_entry(&value, index)?;
                                        if rt.text(key)?.as_str() == name { found = Some(child); break; }
                                    }
                                    found
                                };
                                let Some(child) = child else { mismatch = true; break; };
                                visits.push(Visit { value: child.to_owned(), target: target_shape.fields[index].0, path: format!("{path}.{name}") });
                            }
                        }
                    }
                    (Kind::Record | Kind::Dict, Kind::Dict) => {
                        let count = if source.kind == Kind::Dict { rt.dict_len(&value)? } else { source.fields.len() };
                        for index in 0..count {
                            let (name, key, child) = if source.kind == Kind::Dict {
                                let (key, child) = rt.dict_entry(&value, index)?;
                                (rt.text(key)?.as_str().to_owned(), Some(key.to_owned()), child)
                            } else { (source.field_names[index].clone(), None, rt.field(&value, index)?) };
                            visits.push(Visit { value: child.to_owned(), target: target_shape.arguments[0], path: format!("{path}.{name}") });
                            names.push((name, key));
                        }
                    }
                    (Kind::Array, Kind::Array) | (Kind::Tuple, Kind::Tuple | Kind::Newtype) => {
                        let array = source.kind == Kind::Array;
                        let count = if array { rt.array_len(&value)? } else { source.fields.len() };
                        mismatch = !array && count != target_shape.fields.len();
                        if !mismatch {
                            for index in 0..count {
                                let child = if array { rt.array_get(&value, index)? } else { rt.field(&value, index)? };
                                let target = if array { target_shape.arguments[0] } else { target_shape.fields[index].0 };
                                visits.push(Visit { value: child.to_owned(), target, path: format!("{path}[{index}]") });
                            }
                        }
                    }
                    (Kind::Enum, Kind::Enum) if (source.optional && target_shape.optional) || (source.result && target_shape.result) => {
                        let index = rt.enum_tag(&value)?;
                        tag = Some(index);
                        if let Some(child) = rt.enum_payload(&value)? {
                            visits.push(Visit { value: child.to_owned(), target: target_shape.variants[index as usize].payload.ok_or("cast payload absent from sealed variant")?, path: format!("{path}.payload") });
                        }
                    }
                    _ => mismatch = true,
                }
            }
            if mismatch {
                let actual = source.dynamic_kind.unwrap_or(match source.kind { Kind::Record | Kind::Dict => "Dict", Kind::Newtype | Kind::Tuple => "Tuple", Kind::Enum if rt.enum_payload(&value)?.is_some() => "Tagged", Kind::Enum => "Atom", _ => "Opaque" });
                let message = if target_shape.nominal && blocked_identity { format!("{path} has a different declared type identity") }
                    else if matches!(target_shape.dynamic_kind, Some("Int" | "Float" | "String" | "Bytes" | "Dyn")) {
                        format!("{path} must be {}, got {actual}", target_shape.dynamic_kind.unwrap())
                    } else { format!("{path}: representation does not match cast target") };
                return Err(DecodeFailure::Rejected(message, vec![value.origin()]));
            }
            pending.push(Task::Build { original: value, target, children: visits.iter().map(|visit| visit.value.clone()).collect(), names, tag });
            pending.extend(visits.into_iter().rev().map(Task::Visit));
        }
        output.pop().ok_or_else(|| "cast produced no result".into())
    }
}
