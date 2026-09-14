//! Coverage uses closed pattern types and constructor selections. No legacy
//! descriptor tree, alternate inference, constant evaluation or VM is involved.
use super::*;

#[derive(Clone, Copy)]
struct Coverage {
    irrefutable: bool,
    variant: Option<u32>,
    covers_variant: bool,
}

impl Solver<'_> {
    fn valid_pattern_shape(&self, node: HirId, mut ty: TypeId) -> bool {
        if self.mir.types[ty.index()].constructor == TypeConstructor::Unchecked {
            ty = self.mir.types[ty.index()].arguments[0];
        }
        match self.mir.hir[node.index()].kind {
            HirKind::TuplePattern => {
                self.mir.types[ty.index()].constructor == TypeConstructor::Tuple
            }
            HirKind::StructPattern => match self.mir.types[ty.index()].constructor {
                TypeConstructor::Record(_) => true,
                TypeConstructor::Nominal(symbol) => {
                    self.nominal_index[symbol.index()].is_some_and(|index| {
                        self.mir.type_definitions[index].operation == TypeOperation::Struct
                    })
                }
                _ => false,
            },
            _ => true,
        }
    }

    fn pattern_variants(&self, ty: TypeId) -> Option<Vec<(String, bool)>> {
        let constructor = &self.mir.types[ty.index()].constructor;
        let count = match constructor {
            TypeConstructor::Bool => {
                return Some(vec![("False".into(), false), ("True".into(), false)]);
            }
            TypeConstructor::Option | TypeConstructor::Result | TypeConstructor::FoldControl => 2,
            TypeConstructor::PropertyTarget => 6,
            TypeConstructor::Nominal(symbol) => {
                let definition = &self.mir.type_definitions[self.nominal_index[symbol.index()]?];
                return (definition.operation == TypeOperation::Enum).then(|| {
                    definition
                        .members
                        .iter()
                        .map(|member| (member.name.clone(), member.payload.is_some()))
                        .collect()
                });
            }
            _ => return None,
        };
        Some(
            (0..count)
                .map(|index| {
                    let (name, payload) = crate::type_image::builtin_variant(constructor, index)
                        .expect("native variant index");
                    (name.to_owned(), payload)
                })
                .collect(),
        )
    }

    fn pattern_coverage(&self, node: HirId, facts: &[Option<Coverage>]) -> Option<Coverage> {
        let ty = self.known(node.ty())?;
        if !self.valid_pattern_shape(node, ty) {
            return None;
        }
        let child_covers = |role| -> Option<bool> {
            let children = self.children(node, role);
            children
                .into_iter()
                .map(|child| facts[child.index()].map(|fact| fact.irrefutable))
                .collect::<Option<Vec<_>>>()
                .map(|children| children.into_iter().all(|covered| covered))
        };
        let mut fact = Coverage {
            irrefutable: false,
            variant: None,
            covers_variant: false,
        };
        match self.mir.hir[node.index()].kind {
            HirKind::Wildcard => fact.irrefutable = true,
            HirKind::PatternName(_)
                if self.mir.hir_symbols[node.index()].is_some_and(|symbol| {
                    self.mir.symbols[symbol.index()].resolution == ResolveState::Bound(symbol)
                }) =>
            {
                fact.irrefutable = true
            }
            HirKind::TuplePattern => fact.irrefutable = child_covers(Role::Item)?,
            HirKind::StructPattern => fact.irrefutable = child_covers(Role::Field)?,
            HirKind::PatternField => return facts[self.child(node, Role::Pattern)?.index()],
            HirKind::ConstructorPattern | HirKind::PatternName(_) => {
                let payload_covers = self
                    .child(node, Role::Pattern)
                    .map(|payload| facts[payload.index()].map(|fact| fact.irrefutable))
                    .unwrap_or(Some(true))?;
                match self.mir.member_selections[node.index()]? {
                    MemberSelection::NewtypePattern => fact.irrefutable = payload_covers,
                    MemberSelection::Boolean(value) => {
                        fact.variant = Some(u32::from(value));
                        fact.covers_variant = true;
                    }
                    MemberSelection::EnumVariant { index } => {
                        let variants = self.pattern_variants(ty)?;
                        variants.get(index as usize)?;
                        fact.variant = Some(index);
                        fact.covers_variant = payload_covers;
                        fact.irrefutable = variants.len() == 1 && payload_covers;
                    }
                    _ => return None,
                }
            }
            HirKind::Int(_) | HirKind::Float(_) | HirKind::String(_) => {}
            _ => return None,
        }
        Some(fact)
    }

    pub(super) fn validate_patterns(&mut self) {
        // HIR children precede their parents; this scan visits each pattern
        // once, retaining only small facts indexed by its existing HirId.
        let mut facts = vec![None; self.mir.hir.len()];
        for index in 0..self.mir.hir.len() {
            let node = HirId(index as u32);
            if let Some(ty) = self.known(node.ty())
                && !self.valid_pattern_shape(node, ty)
            {
                let kind = if matches!(self.mir.hir[index].kind, HirKind::StructPattern) {
                    "Struct"
                } else {
                    "Tuple"
                };
                self.mir.diagnostics.push(Diagnostic::error(
                    format!(
                        "{kind} pattern cannot match {:?}",
                        self.mir.types[ty.index()].constructor
                    ),
                    self.mir.hir[index].location,
                ));
            }
            facts[index] = self.pattern_coverage(node, &facts);
        }
        for index in 0..self.mir.hir.len() {
            let node = HirId(index as u32);
            if matches!(self.mir.hir[index].kind, HirKind::LetElse) {
                let pattern = self.child(node, Role::Pattern).expect("let else pattern");
                if facts[pattern.index()].is_some_and(|fact| fact.irrefutable) {
                    self.mir.diagnostics.push(Diagnostic::error(
                        "let else pattern is irrefutable",
                        self.mir.hir[pattern.index()].location,
                    ));
                }
                continue;
            }
            if !matches!(self.mir.hir[index].kind, HirKind::Match) {
                continue;
            }
            let value = self.child(node, Role::Value).expect("match value");
            let Some(ty) = self.known(value.ty()) else {
                continue;
            };
            let variants = self.pattern_variants(ty);
            let mut covered = BTreeSet::new();
            let mut all = false;
            let mut complete_facts = true;
            let mut let_pattern = false;
            for arm in self.children(node, Role::Arm) {
                let pattern = self.child(arm, Role::Pattern).expect("arm pattern");
                let Some(fact) = facts[pattern.index()] else {
                    complete_facts = false;
                    continue;
                };
                if matches!(
                    self.mir.hir[arm.index()].kind,
                    HirKind::MatchArm { irrefutable: true }
                ) {
                    let_pattern = true;
                    if !fact.irrefutable {
                        self.mir.diagnostics.push(Diagnostic::error(
                            "refutable let pattern",
                            self.mir.hir[pattern.index()].location,
                        ));
                    }
                }
                if all {
                    self.mir.diagnostics.push(Diagnostic::error(
                        "unreachable match arm; prior arms cover every value",
                        self.mir.hir[pattern.index()].location,
                    ));
                } else if let Some(variant) =
                    fact.variant.filter(|variant| covered.contains(variant))
                {
                    let name = &variants.as_ref().expect("variant family")[variant as usize].0;
                    self.mir.diagnostics.push(Diagnostic::error(
                        format!("unreachable match arm; prior arms cover {name}"),
                        self.mir.hir[pattern.index()].location,
                    ));
                }
                if self.child(arm, Role::Guard).is_none() {
                    if fact.covers_variant {
                        covered.extend(fact.variant);
                    }
                    all |= fact.irrefutable
                        || variants
                            .as_ref()
                            .is_some_and(|variants| covered.len() == variants.len());
                }
            }
            if !all
                && complete_facts
                && !let_pattern
                && let Some(variants) = variants
            {
                let missing = variants
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| !covered.contains(&(*index as u32)))
                    .map(|(_, (name, payload))| {
                        if *payload {
                            format!("{name}(_)")
                        } else {
                            name.clone()
                        }
                    })
                    .collect::<Vec<_>>();
                if !missing.is_empty() {
                    self.mir.diagnostics.push(Diagnostic::error(
                        format!("non-exhaustive match; missing {}", missing.join(", ")),
                        self.mir.hir[index].location,
                    ));
                }
            }
        }
    }
}
