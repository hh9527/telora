//! Apply nominal member skeletons before publishing the static graph. Runtime
//! consumers index this table by TypeId; they never substitute type parameters.
use super::*;
use std::collections::BTreeMap;

impl Solver<'_> {
    pub(super) fn materialize_layouts(&mut self) {
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
        let mut index = 0;
        while index < self.mir.types.len() {
            self.mir.type_layouts.push(None);
            let owner = self.mir.types[index].clone();
            index += 1;
            let TypeConstructor::Nominal(symbol) = owner.constructor else {
                continue;
            };
            let Some(definition) = self.nominal_index[symbol.index()] else {
                continue;
            };
            let definition = &self.mir.type_definitions[definition];
            let substitutions = definition
                .parameters
                .iter()
                .copied()
                .zip(owner.arguments)
                .collect::<BTreeMap<_, _>>();
            let members = definition
                .members
                .iter()
                .map(|member| (member.syntax, member.payload))
                .collect::<Vec<_>>();
            let mut applied = vec![];
            let mut valid = true;
            for (syntax, slot) in members {
                let payload = if let Some(slot) = slot {
                    let TypeState::Known(ty) = self.mir.ty_slots[slot.index()] else {
                        valid = false;
                        if self.mir.ty_slots[slot.index()] == TypeState::Unknown
                            && !self.mir.type_unknowns.contains(&slot)
                        {
                            self.mir.type_unknowns.push(slot);
                            self.mir.diagnostics.push(Diagnostic::error(
                                "unknown nominal member type",
                                self.mir.hir[syntax.index()].location,
                            ));
                        }
                        continue;
                    };
                    Some(self.substitute_resolved(ty, &substitutions, &mut canonical))
                } else {
                    None
                };
                applied.push(payload);
            }
            if valid {
                self.mir.type_layouts[index - 1] = Some(TypeLayout { members: applied });
            }
            if self.mir.types.len() > 65_536 {
                let declaration = self.mir.symbols[symbol.index()].declarations[0];
                self.mir.diagnostics.push(Diagnostic::error(
                    "nominal member graph exceeds static expansion limit",
                    self.mir.hir[declaration.index()].location,
                ));
                break;
            }
        }
    }
}
