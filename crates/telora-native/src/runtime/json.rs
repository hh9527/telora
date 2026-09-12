use super::*;
use std::collections::BTreeSet;

impl Runtime {
    /// Read the original native object graph and build only the output text
    /// and traversal stack. No host Value tree or replacement heap is built.
    pub fn semantic_json(&self, contract: &DataContract, root: &Value) -> Result<String> {
        self.semantic_json_indented(contract, root, None)
    }

    pub fn semantic_json_indented(&self, contract: &DataContract, root: &Value, indent: Option<usize>) -> Result<String> {
        if indent.is_some_and(|width| width > 16) {
            return Err("std/json.stringify_pretty indent must be between 0 and 16".into());
        }
        type Identity = (u8, u32, u64);
        enum Task<'a> {
            Value(ValueRef<'a>, usize),
            Array(Value, usize, usize, usize),
            Object(Value, usize, usize, usize),
            Character(char),
            Newline(usize),
            Leave(Identity),
        }
        let mut pending = vec![Task::Value(root.as_ref(), 0)];
        let mut active = BTreeSet::new();
        let mut output = output::Output::new(self);
        while let Some(task) = pending.pop() {
            let (value, depth) = match task {
                Task::Newline(depth) => {
                    if let Some(width) = indent {
                        output.push('\n')?;
                        for _ in 0..width * depth { output.push(' ')?; }
                    }
                    continue;
                }
                Task::Character(c) => {
                    output.push(c)?;
                    continue;
                }
                Task::Leave(id) => {
                    active.remove(&id);
                    continue;
                }
                Task::Array(array, index, len, depth) => {
                    if index != 0 { output.push(',')?; }
                    let value = self.array_get(&array, index)?;
                    if index + 1 < len { pending.push(Task::Array(array, index + 1, len, depth)); }
                    pending.push(Task::Value(value, depth + 1));
                    pending.push(Task::Newline(depth + 1));
                    continue;
                }
                Task::Object(dict, index, len, depth) => {
                    if index != 0 { output.push(',')?; }
                    if let Some(width) = indent {
                        output.push('\n')?;
                        for _ in 0..width * (depth + 1) { output.push(' ')?; }
                    }
                    let (key, value) = self.dict_entry(&dict, index)?;
                    serde_json::to_writer(&mut output, self.text(key)?.as_str()).map_err(|e| e.to_string())?;
                    output.push(':')?;
                    if indent.is_some() { output.push(' ')?; }
                    if index + 1 < len { pending.push(Task::Object(dict, index + 1, len, depth)); }
                    pending.push(Task::Value(value, depth + 1));
                    continue;
                }
                Task::Value(value, depth) => (value, depth),
            };
            self.validate(value, contract.value_type())?;
            let tag = self.variant_name(value.type_id(), self.enum_tag_ref(value)?)?;
            if let Some(text) = match tag {
                "None" => Some("null"),
                "True" => Some("true"),
                "False" => Some("false"),
                _ => None,
            } {
                output.push_str(text)?;
                continue;
            }
            let payload = self
                .enum_payload_ref(value)?
                .ok_or("semantic Value payload missing")?;
            match tag {
                "Int" => output.push_str(&(self.scalar_bits(payload)? as i64).to_string())?,
                "Float" => {
                    let number = f64::from_bits(self.scalar_bits(payload)?);
                    if !number.is_finite() {
                        return Err("JSON cannot encode a non-finite Float".into());
                    }
                    output.push_str(&number.to_string())?;
                }
                "String" => serde_json::to_writer(&mut output, self.text(payload)?.as_str()).map_err(|e| e.to_string())?,
                "Bytes" => return Err("JSON cannot encode Bytes".into()),
                "LocalDate" | "LocalTime" | "LocalDateTime" | "OffsetDateTime" => {
                    return Err("JSON cannot encode temporal values; use a codec first".into());
                }
                "Array" => {
                    let array = payload.to_owned(); // fixed-width descriptor only
                    let (heap, start, end, _) = self.array_range(&array)?;
                    let id = (0, heap, u64::from(start) | (u64::from(end) << 32));
                    if !active.insert(id) {
                        return Err("JSON cannot encode cyclic values".into());
                    }
                    pending.push(Task::Leave(id));
                    pending.push(Task::Character(']'));
                    let len = self.array_len(&array)?;
                    if len > 0 {
                        pending.push(Task::Newline(depth));
                        pending.push(Task::Array(array, 0, len, depth));
                    }
                    output.push('[')?;
                }
                "Object" => {
                    let dict = payload.to_owned(); // columns remain borrowed
                    let id = (1, dict.words[3] as u32, dict.words[2]);
                    if !active.insert(id) {
                        return Err("JSON cannot encode cyclic values".into());
                    }
                    pending.push(Task::Leave(id));
                    pending.push(Task::Character('}'));
                    let len = self.dict_len(&dict)?;
                    if len > 0 {
                        pending.push(Task::Newline(depth));
                        pending.push(Task::Object(dict, 0, len, depth));
                    }
                    output.push('{')?;
                }
                _ => return Err("invalid semantic Value variant".into()),
            }
        }
        output.finish()
    }
}
