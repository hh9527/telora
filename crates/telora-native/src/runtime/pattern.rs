use super::*;
use std::collections::BTreeSet;

#[derive(Clone)]
pub(super) struct CompiledRegex {
    regex: regex::Regex,
    captures: BTreeSet<String>,
    required: BTreeSet<String>,
}

fn required(hir: &regex_syntax::hir::Hir) -> BTreeSet<String> {
    use regex_syntax::hir::HirKind;
    match hir.kind() {
        HirKind::Capture(capture) => {
            let mut names = required(&capture.sub);
            if let Some(name) = &capture.name {
                names.insert(name.to_string());
            }
            names
        }
        HirKind::Concat(items) => items.iter().flat_map(required).collect(),
        HirKind::Alternation(items) => {
            let mut items = items.iter();
            let Some(first) = items.next() else {
                return BTreeSet::new();
            };
            items.fold(required(first), |names, item| {
                names.intersection(&required(item)).cloned().collect()
            })
        }
        HirKind::Repetition(repetition) if repetition.min != 0 => required(&repetition.sub),
        _ => BTreeSet::new(),
    }
}

impl Runtime {
    pub(super) fn regex_equal(&self, left: &Value, right: &Value) -> Result<bool> {
        Ok(self.regex_object(left)?.regex.as_str() == self.regex_object(right)?.regex.as_str())
    }
    pub(super) fn regex_object(&self, value: &Value) -> Result<&CompiledRegex> {
        self.validate(value.as_ref(), value.type_id())?;
        self.expect(value.type_id(), Kind::Regex)?;
        let raw = u32::try_from(value.words[2]).map_err(|_| "invalid Regex HeapId")?;
        let reference = HeapRef::from_raw(raw);
        self.tables(reference)
            .regexes
            .get(reference.slot() as usize)
            .ok_or_else(|| "invalid Regex HeapId".into())
    }
    pub(crate) fn regex_compile(
        &mut self,
        ty: TypeId,
        loc: Location,
        pattern: &Value,
    ) -> Result<Value> {
        self.expect(ty, Kind::Regex)?;
        let text = self.text(pattern.as_ref())?;
        let hir = regex_syntax::Parser::new()
            .parse(text.as_str())
            .map_err(|e| format!("invalid regular expression: {e}"))?;
        let regex = regex::Regex::new(text.as_str())
            .map_err(|e| format!("invalid regular expression: {e}"))?;
        let captures = regex
            .capture_names()
            .enumerate()
            .skip(1)
            .map(|(index, name)| {
                name.map(str::to_owned)
                    .ok_or_else(|| format!("capture group {index} must have a name"))
            })
            .collect::<Result<BTreeSet<_>>>()?;
        let required = required(&hir);
        let heap = HeapRef::new(
            World::Work,
            u32::try_from(self.work.regexes.len()).map_err(|_| "Regex table overflow")?,
        )?;
        self.work.regexes.push(CompiledRegex {
            regex,
            captures,
            required,
        });
        self.pack(ty, loc, &[u64::from(heap.raw())])
    }
    pub(crate) fn regex_matches(&self, pattern: &Value, input: &Value) -> Result<bool> {
        Ok(self
            .regex_object(pattern)?
            .regex
            .is_match(self.text(input.as_ref())?.as_str()))
    }
    pub(crate) fn regex_prepare(
        &self,
        pattern: &Value,
        property: &Value,
        owner: &Value,
    ) -> Result<Value> {
        let compiled = self.regex_object(pattern)?;
        let property = self.represented_type(property.as_ref())?;
        let owner = self.represented_type(owner.as_ref())?;
        self.regex_contract(compiled, property, owner)?;
        Ok(pattern.clone())
    }
    fn regex_contract(&self, compiled: &CompiledRegex, property: TypeId, owner: TypeId) -> Result<()> {
        let layout = self.layout(owner)?;
        if layout.kind != Kind::Record || layout.dynamic_kind != Some("Dict") {
            return Err("std/regex.parse_by requires a struct type".into());
        }
        let expected = layout.field_names.iter().cloned().collect::<BTreeSet<_>>();
        if expected != compiled.captures {
            let missing = expected.difference(&compiled.captures).collect::<Vec<_>>();
            let extra = compiled.captures.difference(&expected).collect::<Vec<_>>();
            return Err(format!(
                "regex captures must match struct fields; missing captures {missing:?}, extra captures {extra:?}"
            ));
        }
        for (name, &(ty, _)) in layout.field_names.iter().zip(&layout.fields) {
            let field = self.layout(ty)?;
            let optional = field.optional;
            let inner = if optional { field.arguments[0] } else { ty };
            if !matches!(
                self.layout(inner)?.dynamic_kind,
                Some("Int" | "Float" | "String")
            ) && !self.property_presence.contains(&(inner, property))
            {
                return Err(format!("regex field {name:?} is not string-parsable"));
            }
            if optional == compiled.required.contains(name) {
                return Err(format!(
                    "regex capture {name:?} is {}, but its field is {}",
                    if optional { "required" } else { "optional" },
                    if optional { "optional" } else { "required" }
                ));
            }
        }
        Ok(())
    }
    pub(super) fn regex_captures(&self, pattern: &Value, input: &str, owner: TypeId, property: TypeId) -> Result<Vec<(String, TypeId, Option<std::ops::Range<usize>>)>> {
        let compiled = self.regex_object(pattern)?;
        self.regex_contract(compiled, property, owner)?;
        let captures = compiled.regex.captures(input).ok_or("input does not match regular expression")?;
        let layout = self.layout(owner)?;
        Ok(layout.field_names.iter().zip(&layout.fields).map(|(name, &(ty, _))| {
            (name.clone(), ty, captures.name(name).map(|capture| capture.range()))
        }).collect())
    }
}
