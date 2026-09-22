//! One encoding function per closed (source, Value) identity pair.
use crate::plan::{Key, Plan, Special};
use telora_core::mir::{
    EvidenceNode, Mir, SealedExecutable, SymbolId, TypeConstructor as T, TypeId,
};

fn display_symbol(mir: &Mir) -> Option<SymbolId> {
    mir.types.iter().find_map(|ty| {
        let T::Nominal(symbol) = ty.constructor else {
            return None;
        };
        let definition = &mir.symbols[symbol.index()];
        (definition.name == "Display"
            && definition.module.is_some_and(|module| {
                mir.modules[module.index()]
                    .native
                    .as_ref()
                    .is_some_and(|native| native.id == 20)
            }))
        .then_some(symbol)
    })
}

fn is_display_evidence(mir: &Mir, symbol: SymbolId, evidence: &EvidenceNode) -> bool {
    let bound = &mir.types[evidence.bound.index()];
    if bound.constructor != T::Meta || bound.arguments.len() != 1 {
        return false;
    }
    let raw = &mir.types[bound.arguments[0].index()];
    raw.constructor == T::Nominal(symbol) && raw.arguments == [evidence.subject]
}

fn bridge_subjects(mir: &Mir, capability_module: u32) -> Vec<TypeId> {
    mir.evidence
        .iter()
        .filter_map(|evidence| {
            let implementation = evidence.implementation?;
            let definition = &mir.symbols[implementation.index()];
            if !definition.module.is_some_and(|module| {
                mir.modules[module.index()]
                    .native
                    .as_ref()
                    .is_some_and(|native| native.id == 13)
            }) {
                return None;
            }
            let implementation = mir
                .trait_implementations
                .iter()
                .find(|candidate| candidate.symbol == implementation)?;
            implementation
                .requirements
                .iter()
                .any(|(_, bound)| {
                    let bound = &mir.types[bound.index()];
                    if bound.constructor != T::Meta || bound.arguments.len() != 1 {
                        return false;
                    }
                    let raw = &mir.types[bound.arguments[0].index()];
                    let T::Nominal(symbol) = raw.constructor else {
                        return false;
                    };
                    mir.symbols[symbol.index()].module.is_some_and(|module| {
                        mir.modules[module.index()]
                            .native
                            .as_ref()
                            .is_some_and(|native| native.id == capability_module)
                    })
                })
                .then_some(evidence.subject)
        })
        .collect()
}

impl Plan {
    pub(crate) fn plan_decoders(
        &mut self,
        executable: &SealedExecutable<'_>,
    ) -> Result<(), String> {
        let mir = executable.sealed_mir().mir();
        self.parse_codecs.extend(bridge_subjects(mir, 27));
        let mut pending = Vec::new();
        for root in executable.closure().nodes() {
            if crate::natives::identity(mir, root.node) != Some((13, "decode_with")) {
                continue;
            }
            let key = Key {
                node: root.node,
                instance: root.instance,
                callable: true,
                special: Special::Normal,
            };
            let args = &mir.types[key.ty(mir, root.node)?.index()].arguments;
            if args.len() != 4 || mir.types[args[1].index()].constructor != T::TypeOf {
                return Err("Wasm: decoder signature mismatch".into());
            }
            self.record_codec_property_type(mir, args[0])?;
            pending.push((args[2], mir.types[args[1].index()].arguments[0]));
        }
        while let Some((source, target)) = pending.pop() {
            let key = Key {
                special: Special::Decode(source, target),
                callable: true,
                ..self.root
            };
            if self.functions.contains_key(&key) {
                continue;
            }
            self.functions.insert(key, 0);
            if source == target {
                continue;
            }
            for (index, variant) in self.layouts[target.index()].variants.iter().enumerate() {
                if variant.type_id.is_some() {
                    self.functions.insert(
                        Key {
                            special: Special::DecodeVariant(source, target, index as u32),
                            callable: true,
                            ..self.root
                        },
                        0,
                    );
                }
            }
            if matches!(
                mir.types[target.index()].constructor,
                T::Array | T::Dict | T::Tuple | T::Option
            ) {
                pending.extend(
                    mir.types[target.index()]
                        .arguments
                        .iter()
                        .map(|&child| (source, child)),
                );
            }
            if let Some(object) = &self.layouts[target.index()].object {
                pending.extend(
                    object
                        .members
                        .iter()
                        .filter_map(|m| m.type_id)
                        .map(|id| (source, self.layouts[id].id())),
                );
            }
            pending.extend(
                self.layouts[target.index()]
                    .variants
                    .iter()
                    .filter_map(|m| m.type_id)
                    .map(|id| (source, self.layouts[id].id())),
            );
        }
        Ok(())
    }
    pub(crate) fn plan_encoders(
        &mut self,
        executable: &SealedExecutable<'_>,
    ) -> Result<(), String> {
        let mir = executable.sealed_mir().mir();
        self.display_codecs.extend(bridge_subjects(mir, 20));
        if let Some(symbol) = display_symbol(mir) {
            self.display_evidence.extend(
                mir.evidence
                    .iter()
                    .enumerate()
                    .filter(|(_, evidence)| is_display_evidence(mir, symbol, evidence))
                    .map(|(index, evidence)| (evidence.subject, index)),
            );
        }
        let mut pending = Vec::new();
        for root in executable.closure().nodes() {
            if crate::natives::identity(mir, root.node) != Some((13, "encode_with")) {
                continue;
            }
            let key = Key {
                node: root.node,
                instance: root.instance,
                callable: true,
                special: Special::Normal,
            };
            let args = &mir.types[key.ty(mir, root.node)?.index()].arguments;
            if args.len() != 4 {
                return Err("Wasm: encoder signature mismatch".into());
            }
            self.record_codec_property_type(mir, args[0])?;
            pending.push((args[2], args[3]));
        }
        while let Some((source, target)) = pending.pop() {
            let key = Key {
                special: Special::Encode(source, target),
                callable: true,
                ..self.root
            };
            if self.functions.contains_key(&key) {
                continue;
            }
            self.functions.insert(key, 0);
            if source == target {
                continue;
            }
            if matches!(
                mir.types[source.index()].constructor,
                T::Array | T::Dict | T::Tuple | T::Option
            ) {
                pending.extend(
                    mir.types[source.index()]
                        .arguments
                        .iter()
                        .map(|&child| (child, target)),
                );
            }
            if let Some(object) = &self.layouts[source.index()].object {
                pending.extend(
                    object
                        .members
                        .iter()
                        .filter_map(|member| member.type_id)
                        .map(|id| (self.layouts[id].id(), target)),
                );
            }
            pending.extend(
                self.layouts[source.index()]
                    .variants
                    .iter()
                    .filter_map(|member| member.type_id)
                    .map(|id| (self.layouts[id].id(), target)),
            );
        }
        Ok(())
    }

    fn record_codec_property_type(&mut self, mir: &Mir, witness: TypeId) -> Result<(), String> {
        let witness = self.layouts[witness.index()].id();
        let ty = &mir.types[witness.index()];
        if ty.constructor != T::TypeOf || ty.arguments.len() != 1 {
            return Err("Wasm: codec property witness must be TypeOf".into());
        }
        let found = ty.arguments[0];
        if self.codec_property_type.is_some_and(|known| known != found) {
            return Err("Wasm: inconsistent codec property identity".into());
        }
        self.codec_property_type = Some(found);
        Ok(())
    }
}
