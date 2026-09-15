use super::*;

pub(super) struct FlowParser<'a> {
    depth: usize,
    source_id: SourceId,
    offset: usize,
    text: &'a str,
    pos: usize,
    plan: &'a mut ValidatedDataPlan,
    anchors: &'a BTreeMap<String, DataNodeId>,
}
impl<'a> FlowParser<'a> {
    pub(super) fn new(
        source_id: SourceId,
        offset: usize,
        text: &'a str,
        plan: &'a mut ValidatedDataPlan,
        anchors: &'a BTreeMap<String, DataNodeId>,
        depth: usize,
    ) -> Self {
        Self {
            depth,
            source_id,
            offset,
            text,
            pos: 0,
            plan,
            anchors,
        }
    }
    pub(super) fn parse(mut self) -> Result<DataNodeId, Diagnostic> {
        let value = self.value()?;
        self.ws();
        if self.pos != self.text.len() {
            return Err(self.error("unexpected YAML flow content"));
        }
        Ok(value)
    }
    fn value(&mut self) -> Result<DataNodeId, Diagnostic> {
        if self.depth == MAX_NESTING {
            return Err(self.error(format!(
                "data syntax nesting exceeds parser limit ({MAX_NESTING})"
            )));
        }
        self.depth += 1;
        let result = self.value_inner();
        self.depth -= 1;
        result
    }

    fn value_inner(&mut self) -> Result<DataNodeId, Diagnostic> {
        self.ws();
        let start = self.pos;
        match self.peek() {
            Some('[') => {
                self.bump();
                let mut values = Vec::new();
                loop {
                    self.ws();
                    if self.take(']') {
                        break;
                    }
                    values.push(self.value()?);
                    self.ws();
                    if self.take(']') {
                        break;
                    }
                    self.expect(',')?;
                }
                Ok(self.plan.array(values, self.loc(start, self.pos)))
            }
            Some('{') => {
                self.bump();
                let mut entries = Vec::new();
                let mut merged = Vec::new();
                let mut seen = BTreeMap::new();
                loop {
                    self.ws();
                    if self.take('}') {
                        break;
                    }
                    let key_start = self.pos;
                    let key_text = self.scalar_text(&[':'])?;
                    let key = parse_key(key_text.trim()).map_err(|m| self.error(m))?;
                    let key_loc = self.loc(key_start, self.pos);
                    if key != "<<"
                        && let Some(previous) = seen.insert(key.clone(), key_loc)
                    {
                        return Err(Diagnostic::error(
                            format!("duplicate YAML key {key:?}"),
                            key_loc,
                        )
                        .with_secondary("first defined here", previous));
                    }
                    self.expect(':')?;
                    let value = self.value()?;
                    if key == "<<" {
                        collect_merge_entries(self.plan, value, &mut merged)
                            .map_err(|message| self.error(message))?;
                    } else {
                        entries.push((key, key_loc, value));
                    }
                    self.ws();
                    if self.take('}') {
                        break;
                    }
                    self.expect(',')?;
                }
                let explicit = entries
                    .iter()
                    .map(|(key, _, _)| key.as_str())
                    .collect::<alloc::collections::BTreeSet<_>>();
                let mut effective = Vec::new();
                let mut merged_seen = BTreeMap::new();
                for (key, location, value) in merged {
                    if explicit.contains(key.as_str()) {
                        continue;
                    }
                    if let Some(previous) = merged_seen.insert(key.clone(), location) {
                        return Err(Diagnostic::error(
                            format!("duplicate effective YAML merge key {key:?}"),
                            location,
                        )
                        .with_secondary("first merged here", previous));
                    }
                    effective.push((key, location, value));
                }
                effective.extend(entries);
                let fields = effective
                    .into_iter()
                    .map(|(name, key_location, value)| {
                        (
                            name,
                            DataField {
                                key_location,
                                value,
                            },
                        )
                    })
                    .collect();
                Ok(self.plan.object(fields, self.loc(start, self.pos)))
            }
            _ => {
                let raw = self.scalar_text(&[',', ']', '}'])?.trim();
                let loc = self.loc(start, self.pos);
                if let Some(encoded) = raw.strip_prefix("!!binary") {
                    let bytes =
                        decode_base64(encoded.trim()).map_err(|message| self.error(message))?;
                    return Ok(self.plan.scalar(DataScalar::Bytes(bytes), loc));
                }
                if raw.starts_with('!') {
                    return Err(self.error("custom YAML tags are not supported"));
                }
                if let Some(name) = raw.strip_prefix('*') {
                    let anchored = self
                        .anchors
                        .get(name)
                        .copied()
                        .ok_or_else(|| self.error(format!("unknown YAML alias {name:?}")));
                    let anchored = anchored?;
                    return Ok(self.plan.clone_root_at(anchored, loc));
                }
                parse_scalar(self.plan, raw, loc).map_err(|m| self.error(m))
            }
        }
    }
    fn scalar_text(&mut self, stops: &[char]) -> Result<&'a str, Diagnostic> {
        let start = self.pos;
        let mut quote = None;
        while let Some(ch) = self.peek() {
            if let Some(q) = quote {
                self.bump();
                if ch == q {
                    quote = None;
                } else if q == '"' && ch == '\\' {
                    self.bump();
                }
            } else if matches!(ch, '\'' | '"') {
                quote = Some(ch);
                self.bump();
            } else if stops.contains(&ch) {
                break;
            } else {
                self.bump();
            }
        }
        if self.pos == start {
            Err(self.error("expected YAML flow scalar"))
        } else {
            Ok(&self.text[start..self.pos])
        }
    }
    fn ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }
    fn peek(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }
    fn bump(&mut self) {
        if let Some(ch) = self.peek() {
            self.pos += ch.len_utf8();
        }
    }
    fn take(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn expect(&mut self, expected: char) -> Result<(), Diagnostic> {
        self.ws();
        if self.take(expected) {
            Ok(())
        } else {
            Err(self.error(format!("expected {expected:?} in YAML flow value")))
        }
    }
    fn loc(&self, start: usize, end: usize) -> Location {
        Location::from_usize(self.source_id, self.offset + start..self.offset + end)
            .expect("flow range fits")
    }
    fn error(&self, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(message, self.loc(self.pos, self.pos))
    }
}
