impl ExecutionWorld {
    /// Serialize the Value contract selected statically by the caller. No type
    /// witness materialization, descriptor inference or intermediate heap graph.
    pub(crate) fn solved_json(&self, expected: crate::mir::TypeId) -> Result<String, String> {
        enum Task {
            Value(Val),
            Text(String),
            Leave(Handle),
        }
        let view = HeapView {
            current: &self.work.heap,
            background: Some(&self.main),
        };
        let mut pending = vec![Task::Value(self.work.root)];
        let mut active = HashSet::new();
        let mut output = String::new();
        while let Some(task) = pending.pop() {
            let value = match task {
                Task::Text(text) => {
                    output.push_str(&text);
                    continue;
                }
                Task::Leave(handle) => {
                    active.remove(&handle);
                    continue;
                }
                Task::Value(value) => value,
            };
            if value.type_id().and_then(crate::TypeId::solved_id) != Some(expected) {
                return Err(
                    "JSON value does not have the statically selected Value identity".into(),
                );
            }
            let reference = ValueRef { value, view };
            if let Some(atom) = reference.as_atom() {
                output.push_str(match atom.as_str() {
                    "None" => "null",
                    "True" => "true",
                    "False" => "false",
                    _ => return Err("invalid std/value.Value nullary variant".into()),
                });
                continue;
            }
            let DecodedValue::Tagged(handle) = value.value() else {
                return Err("invalid std/value.Value representation".into());
            };
            if !active.insert(handle) {
                return Err("JSON cannot encode cyclic values".into());
            }
            pending.push(Task::Leave(handle));
            let (tag, payload) = view.tagged(handle).map_err(|e| e.to_string())?;
            let tag = (ValueRef { value: tag, view })
                .as_atom()
                .ok_or("invalid Value tag")?;
            let payload_ref = ValueRef {
                value: payload,
                view,
            };
            match tag.as_str() {
                "Int" => output.push_str(
                    &payload_ref
                        .as_int()
                        .ok_or("invalid Value.Int payload")?
                        .to_string(),
                ),
                "Float" => {
                    let number = payload_ref
                        .as_float()
                        .ok_or("invalid Value.Float payload")?;
                    if !number.is_finite() {
                        return Err("JSON cannot encode a non-finite Float".into());
                    }
                    output.push_str(&number.to_string());
                }
                "String" => output.push_str(
                    &serde_json::to_string(
                        payload_ref
                            .as_str()
                            .ok_or("invalid Value.String payload")?
                            .as_str(),
                    )
                    .map_err(|e| e.to_string())?,
                ),
                "Bytes" => return Err("JSON cannot encode Bytes".into()),
                "LocalDate" | "LocalTime" | "LocalDateTime" | "OffsetDateTime" => {
                    return Err("JSON cannot encode temporal values; use a codec first".into());
                }
                "Array" => {
                    let DecodedValue::Array(handle) = payload.value() else {
                        return Err("invalid Value.Array payload".into());
                    };
                    let values = view.sequence(handle, false).map_err(|e| e.to_string())?;
                    output.push('[');
                    pending.push(Task::Text("]".into()));
                    for (index, &value) in values.iter().enumerate().rev() {
                        pending.push(Task::Value(value));
                        if index != 0 {
                            pending.push(Task::Text(",".into()));
                        }
                    }
                }
                "Object" => {
                    let DecodedValue::Dict(handle) = payload.value() else {
                        return Err("invalid Value.Object payload".into());
                    };
                    let (fields, values) = view.dict_parts(handle).map_err(|e| e.to_string())?;
                    let mut fields = fields
                        .iter()
                        .zip(values)
                        .map(|(field, &value)| view.text(*field).map(|name| (name, value)))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|e| e.to_string())?;
                    fields.sort_by_key(|(name, _)| *name);
                    output.push('{');
                    pending.push(Task::Text("}".into()));
                    for (index, (name, value)) in fields.into_iter().enumerate().rev() {
                        pending.push(Task::Value(value));
                        pending.push(Task::Text(":".into()));
                        pending.push(Task::Text(
                            serde_json::to_string(name).map_err(|e| e.to_string())?,
                        ));
                        if index != 0 {
                            pending.push(Task::Text(",".into()));
                        }
                    }
                }
                _ => return Err("invalid std/value.Value variant".into()),
            }
        }
        Ok(output)
    }
}
