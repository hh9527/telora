use super::*;

impl Solver<'_> {
    pub(super) fn member(
        &mut self,
        node: HirId,
        receiver: TypeSlotId,
        name: String,
    ) -> Option<Task> {
        let Some(mut term) = self.term(receiver).cloned() else {
            return Some(Task::Member {
                node,
                receiver,
                name,
            });
        };
        let mut ty = receiver;
        let mut metadata = false;
        if term.constructor == TypeConstructor::Meta {
            ty = term.arguments[0];
            let Some(raw) = self.term(ty).cloned() else {
                return Some(Task::Member {
                    node,
                    receiver,
                    name,
                });
            };
            term = raw;
            metadata = true;
        } else if let TypeConstructor::TypeFunction(function) = term.constructor {
            let (cons, arity) = match function {
                TypeFunction::Option => (TypeConstructor::Option, 1),
                TypeFunction::Result => (TypeConstructor::Result, 2),
                TypeFunction::FoldControl => (TypeConstructor::FoldControl, 2),
                _ => {
                    self.bad_member(node, &name);
                    return None;
                }
            };
            let args = (0..arity).map(|_| self.fresh()).collect();
            ty = self.structure(cons, args);
            term = self.term(ty).unwrap().clone();
            metadata = true;
        }
        let payload = match &term.constructor {
            TypeConstructor::Record(fields) if !metadata => {
                if let Some(index) = fields.iter().position(|f| f == &name) {
                    self.same(node, term.arguments[index]);
                    self.mir.member_selections[node.index()] = Some(MemberSelection::RecordField);
                } else {
                    self.bad_member(node, &name);
                }
                return None;
            }
            TypeConstructor::Nominal(symbol) => {
                let Some((operation, members)) = self.nominal_members(*symbol, &term.arguments)
                else {
                    self.bad_member(node, &name);
                    return None;
                };
                let Some((index, (_, payload))) = members
                    .into_iter()
                    .enumerate()
                    .find(|(_, (n, _))| n == &name)
                else {
                    self.bad_member(node, &name);
                    return None;
                };
                if metadata && self.is_trait(*symbol) {
                    self.same(node, payload.unwrap());
                    let bound = self.structure(TypeConstructor::Meta, vec![ty]);
                    self.mir.bound_requirements.push(BoundRequirement {
                        subject: term.arguments[0],
                        bound,
                        reference: node,
                        state: BoundState::Pending,
                        evidence: None,
                    });
                    return None;
                }
                if operation == TypeOperation::Struct && !metadata {
                    self.same(node, payload.unwrap());
                    self.mir.member_selections[node.index()] = Some(MemberSelection::RecordField);
                    return None;
                }
                if operation != TypeOperation::Enum || !metadata {
                    self.bad_member(node, &name);
                    return None;
                }
                self.mir.member_selections[node.index()] = Some(MemberSelection::EnumVariant {
                    index: index as u32,
                });
                payload
            }
            TypeConstructor::Bool if metadata && matches!(name.as_str(), "True" | "False") => {
                self.mir.member_selections[node.index()] =
                    Some(MemberSelection::Boolean(name == "True"));
                None
            }
            TypeConstructor::PropertyTarget
                if metadata && matches!(name.as_str(), "Type" | "Field" | "Variant") =>
            {
                None
            }
            TypeConstructor::Option if metadata => match name.as_str() {
                "Some" => Some(term.arguments[0]),
                "None" => None,
                _ => {
                    self.bad_member(node, &name);
                    return None;
                }
            },
            TypeConstructor::Result if metadata => match name.as_str() {
                "Ok" => Some(term.arguments[0]),
                "Err" => Some(term.arguments[1]),
                _ => {
                    self.bad_member(node, &name);
                    return None;
                }
            },
            TypeConstructor::FoldControl if metadata => match name.as_str() {
                "Continue" => Some(term.arguments[0]),
                "Break" => Some(term.arguments[1]),
                _ => {
                    self.bad_member(node, &name);
                    return None;
                }
            },
            _ => {
                self.bad_member(node, &name);
                return None;
            }
        };
        if let Some(payload) = payload {
            self.assign(node, TypeConstructor::Function, vec![payload, ty]);
        } else {
            self.same(node, ty);
        }
        None
    }
    fn bad_member(&mut self, node: HirId, name: &str) {
        self.conflict(
            node.ty(),
            node.ty(),
            Some(self.mir.hir[node.index()].location),
            format!("type has no member {name:?}"),
        );
    }
}
