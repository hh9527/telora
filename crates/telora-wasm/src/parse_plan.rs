//! Finite type-specialized parsers, including recursive record graphs.
use crate::plan::{Key, Plan, Special};
use telora_core::mir::{EvidenceNode, Mir, SealedExecutable, SymbolId, TypeConstructor as T};

fn from_str_symbol(mir: &Mir) -> Option<SymbolId> {
    mir.types.iter().find_map(|ty| {
        let T::Nominal(symbol) = ty.constructor else {
            return None;
        };
        let definition = &mir.symbols[symbol.index()];
        (definition.name == "FromStr"
            && definition.module.is_some_and(|module| {
                mir.modules[module.index()]
                    .native
                    .as_ref()
                    .is_some_and(|native| native.id == 27)
            }))
        .then_some(symbol)
    })
}

fn is_from_str_evidence(mir: &Mir, symbol: SymbolId, evidence: &EvidenceNode) -> bool {
    let bound = &mir.types[evidence.bound.index()];
    if bound.constructor != T::Meta || bound.arguments.len() != 1 {
        return false;
    }
    let raw = &mir.types[bound.arguments[0].index()];
    raw.constructor == T::Nominal(symbol) && raw.arguments == [evidence.subject]
}

pub(crate) fn regex_fallback(mir: &Mir, evidence: usize) -> bool {
    let Some(symbol) = mir.evidence[evidence].implementation else {
        return false;
    };
    let Some(implementation) = mir
        .trait_implementations
        .iter()
        .find(|implementation| implementation.symbol == symbol)
    else {
        return false;
    };
    let Some(property) = crate::regex_property::property_type(mir) else {
        return false;
    };
    implementation.requirements.iter().any(|(_, bound)| {
        let bound = &mir.types[bound.index()];
        bound.constructor == T::Meta
            && bound.arguments.len() == 1
            && {
                let raw = &mir.types[bound.arguments[0].index()];
                raw.constructor == T::PropertyBound && raw.arguments == [property]
            }
    })
}

impl Plan {
    pub(crate) fn plan_parsers(&mut self, executable: &SealedExecutable<'_>) -> Result<(), String> {
        let mir = executable.sealed_mir().mir();
        if let Some(symbol) = from_str_symbol(mir) {
            self.parser_evidence.extend(
                mir.evidence
                    .iter()
                    .enumerate()
                    .filter(|(_, evidence)| is_from_str_evidence(mir, symbol, evidence))
                    .map(|(index, evidence)| (evidence.subject, index)),
            );
        }
        let decode_by_parse = crate::codec_properties::decode_by_parse_property(mir);
        let mut pending = Vec::new();
        pending.extend(self.functions.keys().filter_map(|key| match key.special {
            Special::Decode(_, target)
                if matches!(
                    mir.types[target.index()].constructor,
                    T::Nominal(_) | T::Record(_)
                ) && decode_by_parse.is_some_and(|property| {
                    mir.properties.iter().any(|record| {
                        record.owner == target
                            && record.property == property
                            && record.site == telora_core::mir::PropertySite::Type
                    })
                }) =>
            {
                Some(target)
            }
            _ => None,
        }));
        for root in executable.closure().nodes() {
            if crate::natives::identity(mir, root.node) != Some((27, "parse_with")) {
                continue;
            }
            let key = Key {
                node: root.node,
                instance: root.instance,
                callable: true,
                special: Special::Normal,
            };
            let signature = &mir.types[key.ty(mir, root.node)?.index()];
            let metadata = *signature
                .arguments
                .first()
                .ok_or("Wasm: parse signature missing metadata")?;
            pending.push(
                *mir.types[metadata.index()]
                    .arguments
                    .first()
                    .ok_or("Wasm: parse target missing")?,
            );
        }
        while let Some(ty) = pending.pop() {
            if self.parsers.contains_key(&ty) {
                continue;
            }
            let key = Key {
                special: Special::Parse(ty),
                callable: true,
                ..self.root
            };
            self.parsers.insert(ty, key);
            self.functions.insert(key, 0);
            if mir.types[ty.index()].constructor == T::Option {
                pending.extend(&mir.types[ty.index()].arguments);
            } else if self
                .parser_evidence
                .get(&ty)
                .is_some_and(|&evidence| regex_fallback(mir, evidence))
                && let Some(object) = &self.layouts[ty.index()].object
            {
                pending.extend(
                    object
                        .members
                        .iter()
                        .filter_map(|m| m.type_id)
                        .map(|id| self.layouts[id].id()),
                );
            }
        }
        Ok(())
    }
}
