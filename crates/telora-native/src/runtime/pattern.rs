use super::*;
use std::collections::BTreeSet;
use std::cell::RefCell;
use regex_automata::{meta::{Regex, Cache}, Input, PatternID};

#[derive(Clone)]
pub(super) struct CompiledRegex {
    regex: Regex,
    pattern: String,
    cache: RefCell<Cache>,
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
    pub(super) fn regex_equal(&self, left: &Value, right: &Value, charge: &mut dyn FnMut(u64) -> Result<()>) -> Result<bool> {
        let left = &self.regex_object(left)?.pattern;
        let right = &self.regex_object(right)?.pattern;
        charge((left.len() as u64).checked_add(right.len() as u64).ok_or("regex comparison work overflow")?)?;
        Ok(left == right)
    }
    pub(super) fn charge_regex_copy(&self, compiled: &CompiledRegex) -> Result<()> {
        self.charge_allocation(compiled.pattern.len(), 1, std::mem::size_of::<CompiledRegex>())?;
        self.charge_allocation(compiled.cache.borrow().memory_usage(), 1, 0)?;
        for name in compiled.captures.iter().chain(&compiled.required) {
            self.charge_allocation(name.len(), 1, std::mem::size_of::<String>())?;
        }
        Ok(())
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
        self.charge_allocation(0, 1, 0)?;
        let text = self.text(pattern.as_ref())?;
        let hir = regex_syntax::Parser::new()
            .parse(text.as_str())
            .map_err(|e| format!("invalid regular expression: {e}"))?;
        // Retain the language engine's default NFA limit, and tighten it when
        // the session has less space left. This bounds each NFA, not compiler
        // scratch memory or the sum of the engine's automata.
        const NFA_LIMIT: usize = 10 * 1024 * 1024;
        let limit = self.remaining_allocation_bytes().min(NFA_LIMIT as u64) as usize;
        let regex = Regex::builder().configure(Regex::config().nfa_size_limit(Some(limit)))
            .build(text.as_str()).map_err(|e| {
                if limit < NFA_LIMIT && e.size_limit().is_some() {
                    return self.charge_allocation(limit + 1, 1, 0).unwrap_err();
                }
                format!("invalid regular expression: {e}")
            })?;
        let captures = regex
            .group_info().pattern_names(PatternID::ZERO)
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
        let compiled = CompiledRegex {
            pattern: text.as_str().to_owned(),
            cache: RefCell::new(regex.create_cache()),
            regex,
            captures,
            required,
        };
        self.charge_allocation(compiled.regex.memory_usage(), 1, 0)?;
        self.charge_regex_copy(&compiled)?;
        self.work.regexes.push(compiled);
        self.pack(ty, loc, &[u64::from(heap.raw())])
    }
    pub(crate) fn regex_matches(&self, pattern: &Value, input: &Value) -> Result<bool> {
        let compiled = self.regex_object(pattern)?;
        let mut cache = compiled.cache.borrow_mut();
        let before = cache.memory_usage();
        let matched = compiled.regex.search_with(&mut cache, &Input::new(self.text(input.as_ref())?.as_str()).earliest(true)).is_some();
        self.charge_allocation(cache.memory_usage().saturating_sub(before), 1, 0)?;
        Ok(matched)
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
        // Captures::all allocates exactly group_info.slot_len() slots. Admit
        // that known storage before asking the engine to allocate it.
        self.charge_allocation(compiled.regex.group_info().slot_len(), std::mem::size_of::<Option<regex_automata::util::primitives::NonMaxUsize>>(), 0)?;
        let mut captures = compiled.regex.create_captures();
        let mut cache = compiled.cache.borrow_mut();
        let before = cache.memory_usage();
        compiled.regex.search_captures_with(&mut cache, &Input::new(input), &mut captures);
        self.charge_allocation(cache.memory_usage().saturating_sub(before), 1, 0)?;
        if !captures.is_match() { return Err("input does not match regular expression".into()); }
        let layout = self.layout(owner)?;
        Ok(layout.field_names.iter().zip(&layout.fields).map(|(name, &(ty, _))| {
            (name.clone(), ty, captures.get_group_by_name(name).map(|capture| capture.range()))
        }).collect())
    }
}
