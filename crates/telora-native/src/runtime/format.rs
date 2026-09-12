use super::*;

impl Runtime {
    pub(crate) fn format_prepare(
        &mut self,
        output: TypeId,
        loc: Location,
        source: &Value,
    ) -> Result<Value> {
        let text = self.text(source.as_ref())?;
        let mut chars = text.as_str().chars().peekable();
        let mut strings = vec![String::new()];
        let mut fields = Vec::new();
        while let Some(ch) = chars.next() {
            match ch {
                '{' | '}' if chars.peek() == Some(&ch) => {
                    chars.next();
                    strings.last_mut().unwrap().push(ch);
                }
                '{' => {
                    let mut field = String::new();
                    loop {
                        match chars.next() {
                            Some('}') => break,
                            Some('{') => return Err("nested '{' in Display template field".into()),
                            Some(ch) => field.push(ch),
                            None => return Err("unclosed Display template field".into()),
                        }
                    }
                    if field.is_empty()
                        || field.starts_with(|ch: char| ch.is_ascii_digit())
                        || !field
                            .chars()
                            .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
                    {
                        return Err(format!("invalid Display template field {field:?}"));
                    }
                    fields.push(field);
                    strings.push(String::new());
                }
                '}' => return Err("unmatched '}' in Display template".into()),
                ch => strings.last_mut().unwrap().push(ch),
            }
        }
        let arguments = self.expect(output, Kind::Tuple)?.arguments.clone();
        let mut arrays = Vec::new();
        for (ty, texts) in arguments.into_iter().zip([strings, fields]) {
            let string = self.expect(ty, Kind::Array)?.arguments[0];
            let values = texts
                .iter()
                .map(|text| self.string(string, loc, text))
                .collect::<Result<Vec<_>>>()?;
            arrays.push(self.array(ty, loc, &values)?);
        }
        self.aggregate(output, loc, &arrays)
    }
    pub(crate) fn format_node(
        &mut self,
        output: TypeId,
        loc: Location,
        operation: u64,
        values: &[Value],
    ) -> Result<Value> {
        self.expect(output, Kind::Format)?;
        if operation == 4 {
            if values.len() != 2 {
                return Err("invalid format node arity".into());
            }
            let strings = self.array_len(&values[0])?;
            let items = self.array_len(&values[1])?;
            if strings != items.saturating_add(1) {
                return Err(format!(
                    "std/fmt.concat requires strings.len == items.len + 1, got {strings} and {items}"
                ));
            }
        } else if values.len() != 1 {
            return Err("invalid format node arity".into());
        }
        let mut words = vec![operation];
        for value in values {
            self.validate(value.as_ref(), value.type_id())?;
            words.extend_from_slice(value.words());
        }
        let heap = self.push_words(Table::Formats, words)?;
        self.pack(output, loc, &[u64::from(heap)])
    }
    pub(crate) fn format_render(&self, value: &Value) -> Result<String> {
        fn render(rt: &Runtime, value: &Value, output: &mut String, depth: usize) -> Result<()> {
            if depth >= 128 {
                return Err("std/fmt value exceeds the recursive rendering limit".into());
            }
            let (operation, arguments) = rt.format_parts(value)?;
            let first = arguments.first().ok_or("Fmt input missing")?;
            match operation {
                1 => output.push_str(rt.text(first.as_ref())?.as_str()),
                2 => output.push_str(&(rt.scalar_bits(first.as_ref())? as i64).to_string()),
                3 => output.push_str(&f64::from_bits(rt.scalar_bits(first.as_ref())?).to_string()),
                4 => {
                    let items = arguments.get(1).ok_or("Fmt concat items missing")?;
                    let count = rt.array_len(items)?;
                    if rt.array_len(first)? != count + 1 {
                        return Err("invalid Fmt concat lengths".into());
                    }
                    for index in 0..count {
                        output.push_str(rt.text(rt.array_get(first, index)?)?.as_str());
                        render(
                            rt,
                            &rt.array_get(items, index)?.to_owned(),
                            output,
                            depth + 1,
                        )?;
                    }
                    output.push_str(rt.text(rt.array_get(first, count)?)?.as_str());
                }
                _ => return Err("invalid Fmt operation".into()),
            }
            Ok(())
        }
        let mut output = String::new();
        render(self, value, &mut output, 0)?;
        Ok(output)
    }

    pub(super) fn format_parts(&self, value: &Value) -> Result<(u64, Vec<Value>)> {
        self.validate(value.as_ref(), value.type_id())?;
        self.expect(value.type_id(), Kind::Format)?;
        let words = self.object_words(Table::Formats, u32::try_from(value.words[2]).map_err(|_| "invalid Fmt HeapId")?)?;
        let operation = *words.first().ok_or("empty Fmt node")?;
        let mut rest = &words[1..];
        let mut values = vec![];
        while !rest.is_empty() {
            let header = *rest.get(1).ok_or("truncated Fmt input")?;
            let ty = TypeId((header >> 32) as u32);
            let width = self.layout(ty)?.words;
            let words = rest.get(..width).ok_or("truncated Fmt input")?;
            let value = ValueRef { arena: self.identity, words };
            self.validate(value, ty)?;
            values.push(value.to_owned());
            rest = &rest[width..];
        }
        if !(1..=4).contains(&operation) || values.len() != if operation == 4 { 2 } else { 1 } { return Err("invalid Fmt node shape".into()); }
        Ok((operation, values))
    }
}
