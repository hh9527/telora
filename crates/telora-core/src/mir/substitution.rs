use super::*;
use std::collections::BTreeMap;

#[derive(Debug)]
pub struct SubstitutionNode {
    pub source: TypeId,
    pub arguments: Vec<usize>,
    pub result: Option<TypeId>,
}

/// A single operation context, reused after its output has entered `types`.
/// One source type has one result slot within this substitution context.
#[derive(Default, Debug)]
pub struct TypeSubstitution {
    pub parameters: BTreeMap<SymbolId, TypeId>,
    pub nodes: Vec<SubstitutionNode>,
}

impl Mir {
    pub(crate) fn substitute_resolved_type(
        &mut self,
        source: TypeId,
        parameters: &BTreeMap<SymbolId, TypeId>,
        canonical: &mut BTreeMap<(TypeConstructor, Vec<TypeId>), TypeId>,
    ) -> TypeId {
        self.type_substitution.parameters.clone_from(parameters);
        self.type_substitution.nodes.clear();
        self.type_substitution.nodes.push(SubstitutionNode {
            source,
            arguments: vec![],
            result: None,
        });
        let mut indexed = BTreeMap::from([(source, 0)]);
        let mut pending = vec![(0, false)];
        while let Some((slot, finish)) = pending.pop() {
            if self.type_substitution.nodes[slot].result.is_some() {
                continue;
            }
            let source = self.type_substitution.nodes[slot].source;
            let constructor = self.types[source.index()].constructor.clone();
            if let TypeConstructor::Parameter(parameter) = constructor {
                self.type_substitution.nodes[slot].result =
                    Some(parameters.get(&parameter).copied().unwrap_or(source));
                continue;
            }
            if !finish {
                let mut children = vec![];
                for &source in &self.types[source.index()].arguments {
                    let child = *indexed.entry(source).or_insert_with(|| {
                        let id = self.type_substitution.nodes.len();
                        self.type_substitution.nodes.push(SubstitutionNode {
                            source,
                            arguments: vec![],
                            result: None,
                        });
                        id
                    });
                    children.push(child);
                }
                pending.push((slot, true));
                pending.extend(children.iter().rev().map(|&child| (child, false)));
                self.type_substitution.nodes[slot].arguments = children;
            } else {
                let arguments = self.type_substitution.nodes[slot]
                    .arguments
                    .iter()
                    .map(|&child| {
                        self.type_substitution.nodes[child]
                            .result
                            .expect("child type substitution completed")
                    })
                    .collect::<Vec<_>>();
                let result = if constructor == TypeConstructor::Unchecked
                    && arguments.len() == 1
                    && self.types[arguments[0].index()].constructor == TypeConstructor::Unchecked
                {
                    arguments[0]
                } else {
                    let key = (constructor, arguments);
                    *canonical.entry(key.clone()).or_insert_with(|| {
                        let id = TypeId(self.types.len() as u32);
                        self.types.push(ResolvedType {
                            constructor: key.0,
                            arguments: key.1,
                        });
                        id
                    })
                };
                self.type_substitution.nodes[slot].result = Some(result);
            }
        }
        self.type_substitution.nodes[0].result.unwrap()
    }
}
