use super::*;
use std::collections::BTreeSet;

impl Runtime {
    /// Read the original native object graph and build only the output text
    /// and traversal stack. No host Value tree or replacement heap is built.
    pub fn semantic_json(&self, contract: &DataContract, root: &Value) -> Result<String> {
        type Identity = (u8, u32, u64);
        enum Task<'a> {
            Value(ValueRef<'a>),
            Key(ValueRef<'a>),
            Character(char),
            Leave(Identity),
        }
        let mut pending = vec![Task::Value(root.as_ref())];
        let mut active = BTreeSet::new();
        let mut output = String::new();
        while let Some(task) = pending.pop() {
            let value = match task {
                Task::Character(c) => {
                    output.push(c);
                    continue;
                }
                Task::Leave(id) => {
                    active.remove(&id);
                    continue;
                }
                Task::Key(key) => {
                    output.push_str(
                        &serde_json::to_string(self.text(key)?.as_str())
                            .map_err(|e| e.to_string())?,
                    );
                    continue;
                }
                Task::Value(value) => value,
            };
            self.validate(value, contract.value_type())?;
            let tag = self.variant_name(value.type_id(), self.enum_tag_ref(value)?)?;
            if let Some(text) = match tag {
                "None" => Some("null"),
                "True" => Some("true"),
                "False" => Some("false"),
                _ => None,
            } {
                output.push_str(text);
                continue;
            }
            let payload = self
                .enum_payload_ref(value)?
                .ok_or("semantic Value payload missing")?;
            match tag {
                "Int" => output.push_str(&(self.scalar_bits(payload)? as i64).to_string()),
                "Float" => {
                    let number = f64::from_bits(self.scalar_bits(payload)?);
                    if !number.is_finite() {
                        return Err("JSON cannot encode a non-finite Float".into());
                    }
                    output.push_str(&number.to_string());
                }
                "String" => output.push_str(
                    &serde_json::to_string(self.text(payload)?.as_str())
                        .map_err(|e| e.to_string())?,
                ),
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
                    for index in (0..self.array_len(&array)?).rev() {
                        pending.push(Task::Value(self.array_get(&array, index)?));
                        if index != 0 {
                            pending.push(Task::Character(','));
                        }
                    }
                    output.push('[');
                }
                "Object" => {
                    let dict = payload.to_owned(); // columns remain borrowed
                    let id = (1, dict.words[3] as u32, dict.words[2]);
                    if !active.insert(id) {
                        return Err("JSON cannot encode cyclic values".into());
                    }
                    pending.push(Task::Leave(id));
                    pending.push(Task::Character('}'));
                    for index in (0..self.dict_len(&dict)?).rev() {
                        let (key, value) = self.dict_entry(&dict, index)?;
                        pending.push(Task::Value(value));
                        pending.push(Task::Character(':'));
                        pending.push(Task::Key(key));
                        if index != 0 {
                            pending.push(Task::Character(','));
                        }
                    }
                    output.push('{');
                }
                _ => return Err("invalid semantic Value variant".into()),
            }
        }
        Ok(output)
    }
}
