//! Bounded, field-sensitive abstract values; none imply exclusive storage.
use crate::plan::Key;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
use telora_core::mir::SymbolId;
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub(crate) struct Path {
    pub function: usize,
    pub root: String,
    pub fields: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Closure {
    pub key: Key,
    pub captures: Arc<BTreeMap<SymbolId, Value>>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Value {
    pub parameters: BTreeSet<usize>,
    pub paths: BTreeSet<Path>,
    pub fresh: bool,
    pub updated: bool,
    pub borrowed_view: bool,
    pub unknown: bool,
    pub members: BTreeMap<String, Value>,
    #[serde(skip)]
    pub callable: Option<Closure>,
}
impl Value {
    pub fn unknown() -> Self {
        Self {
            unknown: true,
            ..Self::default()
        }
    }
    pub fn fresh() -> Self {
        Self {
            fresh: true,
            ..Self::default()
        }
    }
    pub fn absorb(&mut self, other: &Self) {
        self.parameters.extend(other.parameters.iter().copied());
        self.paths.extend(other.paths.iter().cloned());
        self.unknown |= other.unknown;
    }
    pub fn join(mut self, other: Self) -> Self {
        self.absorb(&other);
        self.fresh |= other.fresh;
        self.updated |= other.updated;
        self.borrowed_view |= other.borrowed_view;
        self.members = std::mem::take(&mut self.members)
            .into_iter()
            .filter_map(|(name, value)| {
                other
                    .members
                    .get(&name)
                    .map(|v| (name, value.join(v.clone())))
            })
            .collect();
        self.callable = match (self.callable, other.callable) {
            (Some(a), Some(b)) if a.key == b.key => {
                let mut captures = BTreeMap::new();
                for (s, v) in a.captures.iter() {
                    captures.insert(
                        *s,
                        v.clone()
                            .join(b.captures.get(s).cloned().unwrap_or_else(Value::unknown)),
                    );
                }
                for (s, v) in b.captures.iter() {
                    captures
                        .entry(*s)
                        .or_insert_with(|| v.clone().join(Value::unknown()));
                }
                Some(Closure {
                    key: a.key,
                    captures: Arc::new(captures),
                })
            }
            _ => None,
        };
        self.bounded()
    }
    pub fn element(&self) -> Self {
        if let Some(v) = self.members.get("*") {
            return v.clone();
        }
        self.project("[*]")
    }
    pub fn sequence(item: Self) -> Self {
        let mut value = Self::fresh();
        value.absorb(&item);
        value.members.insert("*".into(), item);
        value.bounded()
    }
    pub fn project(&self, name: &str) -> Self {
        if let Some(v) = self.members.get(name) {
            return v.clone();
        }
        if self.paths.is_empty() || self.fresh {
            return Self::unknown();
        }
        let mut value = Self {
            parameters: self.parameters.clone(),
            borrowed_view: true,
            unknown: self.unknown,
            ..Self::default()
        };
        for p in &self.paths {
            if p.fields.len() >= 8 {
                value.unknown = true;
                continue;
            }
            let mut p = p.clone();
            p.fields.push(name.into());
            value.paths.insert(p);
        }
        value.bounded()
    }
    pub fn bounded(mut self) -> Self {
        self.bound(&mut 256, 0);
        self
    }
    fn bound(&mut self, budget: &mut usize, depth: usize) {
        if *budget == 0 || depth >= 8 {
            if !self.members.is_empty() {
                self.members.clear();
                self.unknown = true;
            }
            return;
        }
        *budget -= 1;
        if self.paths.len() > 64 {
            self.paths = self.paths.iter().take(64).cloned().collect();
            self.unknown = true;
        }
        if self.members.len() > 64 {
            self.members = self
                .members
                .iter()
                .take(64)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            self.unknown = true;
        }
        for v in self.members.values_mut() {
            v.bound(budget, depth + 1);
        }
    }
}
