//! Static trait obligations introduced by sealed standard-library adapters.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

impl Solver<'_> {
    pub(super) fn prepare_parse_adapters(&mut self) {
        let Some(from_str) = self.builtin_symbol(27, "FromStr", BindingKind::Trait) else {
            return;
        };
        let regex = self.provider_property(19, "parse_by");
        let mut targets = BTreeSet::new();
        for (property, record) in self.mir.properties.iter().enumerate() {
            if !record.concrete {
                continue;
            }
            if Some(record.property) == regex {
                let Some(layout) = self
                    .mir
                    .type_layouts
                    .get(record.owner.index())
                    .and_then(Option::as_ref)
                else {
                    continue;
                };
                targets.extend(
                    layout
                        .members
                        .iter()
                        .flatten()
                        .map(|&target| (property, target)),
                );
            }
        }
        let mut canonical = self
            .mir
            .types
            .iter()
            .enumerate()
            .map(|(index, ty)| {
                (
                    (ty.constructor.clone(), ty.arguments.clone()),
                    TypeId(index as u32),
                )
            })
            .collect::<BTreeMap<_, _>>();
        for (property, target) in targets {
            let raw = Self::canonical_type(
                self.mir,
                &mut canonical,
                TypeConstructor::Nominal(from_str),
                vec![target],
            );
            let bound =
                Self::canonical_type(self.mir, &mut canonical, TypeConstructor::Meta, vec![raw]);
            let subject = self.known_slot(target);
            let bound = self.known_slot(bound);
            let reference = self.mir.properties[property].providers[0];
            let requirement = self.mir.bound_requirements.len();
            self.mir.bound_requirements.push(BoundRequirement {
                subject,
                bound,
                reference,
                state: BoundState::Pending,
                evidence: None,
            });
            self.mir.parse_adapters.push(ParseAdapter {
                target,
                property,
                requirement,
            });
        }
    }

    fn known_slot(&mut self, ty: TypeId) -> TypeSlotId {
        let slot = self.fresh();
        self.mir.ty_slots[slot.index()] = TypeState::Known(ty);
        slot
    }

    fn canonical_type(
        mir: &mut Mir,
        canonical: &mut BTreeMap<(TypeConstructor, Vec<TypeId>), TypeId>,
        constructor: TypeConstructor,
        arguments: Vec<TypeId>,
    ) -> TypeId {
        let key = (constructor, arguments);
        *canonical.entry(key.clone()).or_insert_with(|| {
            let id = TypeId(mir.types.len() as u32);
            mir.types.push(ResolvedType {
                constructor: key.0,
                arguments: key.1,
            });
            id
        })
    }

    fn builtin_symbol(&self, module: u32, name: &str, kind: BindingKind) -> Option<SymbolId> {
        self.mir
            .symbols
            .iter()
            .enumerate()
            .find_map(|(index, symbol)| {
                (symbol.name == name
                    && symbol.kind == SymbolKind::Declaration(kind)
                    && symbol.module.is_some_and(|id| {
                        self.mir.modules[id.index()]
                            .native
                            .as_ref()
                            .is_some_and(|native| native.id == module)
                    }))
                .then_some(SymbolId(index as u32))
            })
    }

    fn provider_property(&self, module: u32, name: &str) -> Option<TypeId> {
        let symbol = self.mir.symbols.iter().find(|symbol| {
            symbol.name == name
                && symbol.module.is_some_and(|id| {
                    self.mir.modules[id.index()]
                        .native
                        .as_ref()
                        .is_some_and(|native| native.id == module)
                })
        })?;
        let &node = symbol.declarations.first()?;
        let TypeState::Known(mut ty) = self.mir.ty_slots[node.ty().index()] else {
            return None;
        };
        loop {
            let shape = &self.mir.types[ty.index()];
            if shape.constructor != TypeConstructor::Function {
                return Some(ty);
            }
            ty = *shape.arguments.last()?;
        }
    }
}
