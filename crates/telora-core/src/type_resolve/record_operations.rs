//! Static field selection, spread contributions and nominal updates.
use super::*;
use std::collections::BTreeMap;

impl Solver<'_> {
    pub(super) fn record_spread(&mut self, node: HirId) -> Option<Task> {
        let mut fields = BTreeMap::new();
        let mut explicit = std::collections::BTreeSet::new();
        let mut dictionary = None;
        let mut named = false;
        let mut items = vec![];
        for field in self.children(node, Role::Field) {
            let value = self.child(field, Role::Value).unwrap();
            if let Some(name) = self.child(field, Role::Name) {
                let HirKind::Name(name) = &self.mir.hir[name.index()].kind else { unreachable!() };
                if !explicit.insert(name.clone()) {
                    self.projection_error(node, format!("duplicate update field {name:?}"));
                    return None;
                }
                fields.insert(name.clone(), value.ty());
                items.push(value.ty());
            } else {
                let Some(term) = self.term(value.ty()).cloned() else {
                    if matches!(self.mir.ty_slots[self.root(value.ty()).index()], TypeState::Conflicted(_)) {
                        self.same(node, value.ty());
                        return None;
                    }
                    return Some(Task::RecordSpread { node });
                };
                if matches!(term.constructor, TypeConstructor::Record(_)) {
                    if self.term(node.ty()).is_some_and(|target| target.constructor == TypeConstructor::Dict) {
                        self.equal(value.ty(), node.ty(), Some(self.mir.hir[value.index()].location));
                    }
                    return Some(Task::RecordSpread { node });
                }
                if term.constructor == TypeConstructor::Dict {
                    if named {
                        self.projection_error(node, "cannot mix Dict and named struct spreads".into());
                        return None;
                    }
                    dictionary = Some(term.arguments[0]);
                    items.push(term.arguments[0]);
                    continue;
                }
                if dictionary.is_some() {
                    self.projection_error(node, "cannot mix Dict and named struct spreads".into());
                    return None;
                }
                let members = if let TypeConstructor::Nominal(symbol) = term.constructor {
                    self.nominal_members(symbol, &term.arguments)
                } else { None };
                let Some((TypeOperation::Struct, members)) = members else {
                    self.projection_error(node, "record spread requires a named struct operand".into());
                    return None;
                };
                named = true;
                for (name, payload) in members { fields.insert(name, payload.unwrap()); }
            }
        }
        if let Some(element) = dictionary {
            for actual in items {
                if actual.index() < self.mir.hir.len() { self.fit(HirId(actual.0), element, actual); }
                else { self.equal(element, actual, Some(self.mir.hir[node.index()].location)); }
            }
            self.assign(node, TypeConstructor::Dict, vec![element]);
            return None;
        }
        // Only winners receive the contextual field requirements. All authored
        // expressions retain their own slots and are still evaluated by codegen.
        self.assign(node, TypeConstructor::Record(fields.keys().cloned().collect()), fields.into_values().collect());
        None
    }

    pub(super) fn struct_update(&mut self, node: HirId, left: TypeSlotId, right: TypeSlotId) -> Option<Task> {
        let (Some(target), Some(patch)) = (self.term(left).cloned(), self.term(right).cloned()) else {
            for slot in [left, right] {
                if matches!(self.mir.ty_slots[self.root(slot).index()], TypeState::Conflicted(_)) {
                    self.same(node, slot);
                    return None;
                }
            }
            return Some(Task::StructUpdate { node, left, right });
        };
        if matches!(target.constructor, TypeConstructor::Record(_)) {
            return Some(Task::StructUpdate { node, left, right });
        }
        let target = if let TypeConstructor::Nominal(symbol) = target.constructor {
            self.nominal_members(symbol, &target.arguments)
        } else { None };
        let Some((TypeOperation::Struct, target)) = target else {
            self.projection_error(node, "struct update requires a named struct operand".into());
            return None;
        };
        let fields = match patch.constructor {
            TypeConstructor::Record(names) => names.into_iter().zip(patch.arguments).collect::<Vec<_>>(),
            TypeConstructor::Nominal(symbol) => {
                let Some((TypeOperation::Struct, members)) = self.nominal_members(symbol, &patch.arguments) else {
                    self.projection_error(node, "struct update requires a named struct operand".into());
                    return None;
                };
                members.into_iter().map(|(name, payload)| (name, payload.unwrap())).collect()
            }
            _ => {
                self.projection_error(node, "struct update requires a named struct operand".into());
                return None;
            }
        };
        for (name, actual) in fields {
            let Some((_, Some(expected))) = target.iter().find(|(field, _)| field == &name) else {
                self.projection_error(node, format!("unknown struct update field {name:?}"));
                return None;
            };
            if actual.index() < self.mir.hir.len() {
                self.fit(HirId(actual.0), *expected, actual);
            } else {
                self.equal(*expected, actual, Some(self.mir.hir[node.index()].location));
            }
        }
        None
    }

    pub(super) fn field_projection(&mut self, node: HirId, receiver: TypeSlotId) -> Option<Task> {
        let Some(term) = self.term(receiver).cloned() else {
            if matches!(self.mir.ty_slots[self.root(receiver).index()], TypeState::Conflicted(_)) {
                self.same(node, receiver);
                return None;
            }
            return Some(Task::FieldProjection { node, receiver });
        };
        if matches!(term.constructor, TypeConstructor::Record(_)) {
            return Some(Task::FieldProjection { node, receiver });
        }
        let members = if let TypeConstructor::Nominal(symbol) = term.constructor {
            self.nominal_members(symbol, &term.arguments)
        } else { None };
        let Some((TypeOperation::Struct, members)) = members else {
            self.projection_error(node, "field projection requires a named struct source".into());
            return None;
        };
        let mut fields = BTreeMap::new();
        for (source, target) in self.children(node, Role::Name).into_iter().zip(self.children(node, Role::Target)) {
            let HirKind::Name(source) = &self.mir.hir[source.index()].kind else { unreachable!() };
            let HirKind::Name(target) = &self.mir.hir[target.index()].kind else { unreachable!() };
            let Some((_, Some(payload))) = members.iter().find(|(name, _)| name == source) else {
                self.projection_error(node, format!("unknown projection source field {source:?}"));
                return None;
            };
            if fields.insert(target.clone(), *payload).is_some() {
                self.projection_error(node, format!("duplicate projection destination {target:?}"));
                return None;
            }
        }
        self.assign(node, TypeConstructor::Record(fields.keys().cloned().collect()), fields.into_values().collect());
        None
    }

    fn projection_error(&mut self, node: HirId, message: String) {
        self.conflict(node.ty(), node.ty(), Some(self.mir.hir[node.index()].location), message);
    }

    pub(super) fn validate_field_projections(&mut self) {
        let contributions = self.mir.hir.iter().enumerate().filter(|(_, node)| matches!(node.kind, HirKind::Binary(BinaryOperator::StructUpdate)))
            .filter_map(|(index, _)| self.child(HirId(index as u32), Role::Right)).collect::<std::collections::BTreeSet<_>>();
        for index in 0..self.mir.hir.len() {
            if matches!(self.mir.hir[index].kind, HirKind::Dict) {
                let node = HirId(index as u32);
                if self.children(node, Role::Field).iter().any(|&field| self.child(field, Role::Name).is_none()) {
                    for field in self.children(node, Role::Field) {
                        if self.child(field, Role::Name).is_none() {
                            let value = self.child(field, Role::Value).unwrap();
                            if self.term(value.ty()).is_some_and(|term| matches!(term.constructor, TypeConstructor::Record(_))) {
                                self.projection_error(node, "record spread requires a named struct operand".into());
                            }
                        }
                    }
                    if !contributions.contains(&node) && self.term(node.ty()).is_some_and(|term| matches!(term.constructor, TypeConstructor::Record(_))) {
                        self.projection_error(node, "record spread requires a named struct target context".into());
                    }
                }
            }
            if matches!(self.mir.hir[index].kind, HirKind::Binary(BinaryOperator::StructUpdate)) {
                let node = HirId(index as u32);
                let left = self.child(node, Role::Left).unwrap();
                if self.term(left.ty()).is_some_and(|term| matches!(term.constructor, TypeConstructor::Record(_))) {
                    self.projection_error(node, "struct update requires a named struct operand".into());
                }
            }
            if !matches!(self.mir.hir[index].kind, HirKind::FieldProjection) { continue; }
            let node = HirId(index as u32);
            let receiver = self.child(node, Role::Receiver).unwrap();
            if self.term(receiver.ty()).is_some_and(|term| matches!(term.constructor, TypeConstructor::Record(_))) {
                self.projection_error(node, "field projection requires a named struct source".into());
                continue;
            }
            if contributions.contains(&node) { continue; }
            let Some(term) = self.term(node.ty()) else { continue; };
            if !matches!(term.constructor, TypeConstructor::Nominal(_)) {
                self.projection_error(node, "field projection requires a named struct target context".into());
            }
        }
    }
}
