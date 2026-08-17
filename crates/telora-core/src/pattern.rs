use crate::Location;
use crate::ast::{Pattern, PatternKind};
use crate::types::TypeDescriptor;
use std::collections::{BTreeSet, HashSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PatternCompatibility {
    Compatible,
    Incompatible,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PatternBinding {
    pub(crate) name: String,
    pub(crate) location: Location,
    pub(crate) ty: TypeDescriptor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DuplicatePatternBinding {
    pub(crate) name: String,
    pub(crate) location: Location,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PatternProblem {
    pub(crate) location: Location,
    pub(crate) message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PatternAnalysis {
    pub(crate) bindings: Vec<PatternBinding>,
    pub(crate) duplicates: Vec<DuplicatePatternBinding>,
    pub(crate) problems: Vec<PatternProblem>,
    pub(crate) compatibility: PatternCompatibility,
    pub(crate) irrefutable: bool,
    pub(crate) covered_variants: BTreeSet<String>,
    pub(crate) possible_variants: BTreeSet<String>,
}

pub(crate) fn analyze_pattern(pattern: &Pattern, matched: &TypeDescriptor) -> PatternAnalysis {
    let mut context = AnalysisContext::default();
    let shape = context.analyze(pattern, matched);
    PatternAnalysis {
        bindings: context.bindings,
        duplicates: context.duplicates,
        problems: context.problems,
        compatibility: shape.compatibility,
        irrefutable: shape.irrefutable,
        covered_variants: shape.covered_variants,
        possible_variants: shape.possible_variants,
    }
}

pub(crate) fn first_refutable_location(
    pattern: &Pattern,
    matched: &TypeDescriptor,
) -> Option<Location> {
    if let TypeDescriptor::Declared(declared) = matched {
        return first_refutable_location(pattern, declared.body());
    }
    if analyze_pattern(pattern, matched).irrefutable {
        return None;
    }
    match (&pattern.value, matched) {
        (PatternKind::Tuple(items), TypeDescriptor::Tuple(matched_items))
            if items.len() == matched_items.len() =>
        {
            items
                .iter()
                .zip(matched_items)
                .find_map(|(item, matched)| first_refutable_location(item, matched))
                .or(Some(pattern.location))
        }
        (PatternKind::Struct(fields), TypeDescriptor::Struct(matched_fields)) => fields
            .iter()
            .find_map(|field| {
                matched_fields
                    .get(&field.name.value)
                    .and_then(|matched| first_refutable_location(&field.pattern, matched))
                    .or_else(|| {
                        (!matched_fields.contains_key(&field.name.value))
                            .then_some(field.name.location)
                    })
            })
            .or(Some(pattern.location)),
        (
            PatternKind::Tagged { tag, payload },
            TypeDescriptor::Tagged {
                tag: matched_tag,
                payload: matched_payload,
            },
        ) if matched_tag.name() == tag => {
            first_refutable_location(payload, matched_payload).or(Some(pattern.location))
        }
        (PatternKind::Tagged { tag, payload }, TypeDescriptor::Enum(variants))
            if variants.len() == 1 =>
        {
            variants
                .get(tag)
                .and_then(Option::as_deref)
                .and_then(|matched| first_refutable_location(payload, matched))
                .or(Some(pattern.location))
        }
        _ => Some(pattern.location),
    }
}

pub(crate) fn first_incompatible_location(
    pattern: &Pattern,
    matched: &TypeDescriptor,
) -> Option<Location> {
    if let TypeDescriptor::Declared(declared) = matched {
        return first_incompatible_location(pattern, declared.body());
    }
    if analyze_pattern(pattern, matched).compatibility != PatternCompatibility::Incompatible {
        return None;
    }
    match (&pattern.value, matched) {
        (PatternKind::Tuple(items), TypeDescriptor::Tuple(matched_items))
            if items.len() == matched_items.len() =>
        {
            items
                .iter()
                .zip(matched_items)
                .find_map(|(item, matched)| first_incompatible_location(item, matched))
                .or(Some(pattern.location))
        }
        (PatternKind::Struct(fields), TypeDescriptor::Struct(matched_fields)) => fields
            .iter()
            .find_map(|field| {
                matched_fields
                    .get(&field.name.value)
                    .and_then(|matched| first_incompatible_location(&field.pattern, matched))
                    .or_else(|| {
                        (!matched_fields.contains_key(&field.name.value))
                            .then_some(field.name.location)
                    })
            })
            .or(Some(pattern.location)),
        (
            PatternKind::Tagged { tag, payload },
            TypeDescriptor::Tagged {
                tag: matched_tag,
                payload: matched_payload,
            },
        ) if matched_tag.name() == tag => {
            first_incompatible_location(payload, matched_payload).or(Some(pattern.location))
        }
        (PatternKind::Tagged { tag, payload }, TypeDescriptor::Enum(variants)) => variants
            .get(tag)
            .and_then(Option::as_deref)
            .and_then(|matched| first_incompatible_location(payload, matched))
            .or(Some(pattern.location)),
        _ => Some(pattern.location),
    }
}

#[derive(Default)]
struct AnalysisContext {
    names: HashSet<String>,
    bindings: Vec<PatternBinding>,
    duplicates: Vec<DuplicatePatternBinding>,
    problems: Vec<PatternProblem>,
}

struct PatternShape {
    compatibility: PatternCompatibility,
    irrefutable: bool,
    covered_variants: BTreeSet<String>,
    possible_variants: BTreeSet<String>,
}

impl PatternShape {
    fn new(compatibility: PatternCompatibility, irrefutable: bool) -> Self {
        Self {
            compatibility,
            irrefutable,
            covered_variants: BTreeSet::new(),
            possible_variants: BTreeSet::new(),
        }
    }
}

impl AnalysisContext {
    fn analyze(&mut self, pattern: &Pattern, matched: &TypeDescriptor) -> PatternShape {
        if !matches!(
            pattern.value,
            PatternKind::Wildcard | PatternKind::Binding(_)
        ) && let TypeDescriptor::Declared(declared) = matched
        {
            return self.analyze(pattern, declared.body());
        }
        match &pattern.value {
            PatternKind::Wildcard => self.catch_all(matched),
            PatternKind::Binding(name) => {
                if self.names.insert(name.value.clone()) {
                    self.bindings.push(PatternBinding {
                        name: name.value.clone(),
                        location: name.location,
                        ty: matched.clone(),
                    });
                } else {
                    self.duplicates.push(DuplicatePatternBinding {
                        name: name.value.clone(),
                        location: name.location,
                    });
                }
                self.catch_all(matched)
            }
            PatternKind::Int(_) => primitive_shape(matched, PrimitivePattern::Int),
            PatternKind::Float(_) => primitive_shape(matched, PrimitivePattern::Float),
            PatternKind::String(_) => primitive_shape(matched, PrimitivePattern::String),
            PatternKind::Atom(tag) => atom_shape(matched, tag),
            PatternKind::Tagged { tag, payload } => {
                let (payload_type, outer_compatibility, only_variant) = match matched {
                    TypeDescriptor::Tagged {
                        tag: matched_tag,
                        payload,
                    } if matched_tag.name() == tag => {
                        (payload.as_ref(), PatternCompatibility::Compatible, true)
                    }
                    TypeDescriptor::Tagged { .. } => (
                        &TypeDescriptor::Any,
                        PatternCompatibility::Incompatible,
                        false,
                    ),
                    TypeDescriptor::Enum(variants) => match variants.get(tag) {
                        Some(Some(payload)) => (
                            payload.as_ref(),
                            PatternCompatibility::Compatible,
                            variants.len() == 1,
                        ),
                        Some(None) => (
                            &TypeDescriptor::Any,
                            PatternCompatibility::Incompatible,
                            variants.len() == 1,
                        ),
                        None => (
                            &TypeDescriptor::Any,
                            PatternCompatibility::Incompatible,
                            false,
                        ),
                    },
                    matched if is_unknown(matched) => {
                        (&TypeDescriptor::Any, PatternCompatibility::Unknown, false)
                    }
                    _ => (
                        &TypeDescriptor::Any,
                        PatternCompatibility::Incompatible,
                        false,
                    ),
                };
                let payload = self.analyze(payload, payload_type);
                let compatibility = combine(outer_compatibility, payload.compatibility);
                let mut shape = PatternShape::new(
                    compatibility,
                    only_variant
                        && outer_compatibility == PatternCompatibility::Compatible
                        && payload.irrefutable,
                );
                if matches!(matched, TypeDescriptor::Enum(_))
                    && outer_compatibility == PatternCompatibility::Compatible
                {
                    shape.possible_variants.insert(tag.clone());
                    if payload.irrefutable {
                        shape.covered_variants.insert(tag.clone());
                    }
                }
                shape
            }
            PatternKind::Tuple(items) => {
                let matched_items = match matched {
                    TypeDescriptor::Tuple(matched_items) if matched_items.len() == items.len() => {
                        Some(matched_items.as_slice())
                    }
                    _ => None,
                };
                let outer = match matched {
                    TypeDescriptor::Tuple(matched_items) if matched_items.len() == items.len() => {
                        PatternCompatibility::Compatible
                    }
                    TypeDescriptor::Tuple(_) => PatternCompatibility::Incompatible,
                    matched if is_unknown(matched) => PatternCompatibility::Unknown,
                    _ => PatternCompatibility::Incompatible,
                };
                let mut compatibility = outer;
                let mut irrefutable = outer == PatternCompatibility::Compatible;
                for (index, item) in items.iter().enumerate() {
                    let item_type = matched_items
                        .and_then(|matched| matched.get(index))
                        .unwrap_or(&TypeDescriptor::Any);
                    let item = self.analyze(item, item_type);
                    compatibility = combine(compatibility, item.compatibility);
                    irrefutable &= item.irrefutable;
                }
                PatternShape::new(compatibility, irrefutable)
            }
            PatternKind::Struct(fields) => {
                let matched_fields = match matched {
                    TypeDescriptor::Struct(fields) => Some(fields),
                    _ => None,
                };
                let outer = match matched {
                    TypeDescriptor::Struct(_) => PatternCompatibility::Compatible,
                    matched if is_unknown(matched) => PatternCompatibility::Unknown,
                    _ => PatternCompatibility::Incompatible,
                };
                if outer == PatternCompatibility::Incompatible {
                    self.problems.push(PatternProblem {
                        location: pattern.location,
                        message: format!("Struct pattern cannot match {}", matched.display_name()),
                    });
                }
                let mut names = HashSet::new();
                let mut compatibility = outer;
                let mut irrefutable = outer == PatternCompatibility::Compatible;
                for field in fields {
                    if !names.insert(field.name.value.clone()) {
                        self.problems.push(PatternProblem {
                            location: field.name.location,
                            message: format!(
                                "duplicate Struct pattern field {:?}",
                                field.name.value
                            ),
                        });
                    }
                    let field_type =
                        matched_fields.and_then(|matched| matched.get(&field.name.value));
                    if matched_fields.is_some() && field_type.is_none() {
                        self.problems.push(PatternProblem {
                            location: field.name.location,
                            message: format!("Struct has no field {:?}", field.name.value),
                        });
                        compatibility = PatternCompatibility::Incompatible;
                        irrefutable = false;
                    }
                    let field =
                        self.analyze(&field.pattern, field_type.unwrap_or(&TypeDescriptor::Any));
                    compatibility = combine(compatibility, field.compatibility);
                    irrefutable &= field.irrefutable && field_type.is_some();
                }
                PatternShape::new(compatibility, irrefutable)
            }
        }
    }

    fn catch_all(&self, matched: &TypeDescriptor) -> PatternShape {
        let mut shape = PatternShape::new(PatternCompatibility::Compatible, true);
        if let TypeDescriptor::Enum(variants) = matched {
            shape.covered_variants.extend(variants.keys().cloned());
            shape.possible_variants.extend(variants.keys().cloned());
        }
        shape
    }
}

#[derive(Clone, Copy)]
enum PrimitivePattern {
    Int,
    Float,
    String,
}

fn primitive_shape(matched: &TypeDescriptor, pattern: PrimitivePattern) -> PatternShape {
    let compatible = matches!(
        (pattern, matched),
        (PrimitivePattern::Int, TypeDescriptor::Int)
            | (PrimitivePattern::Float, TypeDescriptor::Float)
            | (PrimitivePattern::String, TypeDescriptor::String)
    );
    let compatibility = if compatible {
        PatternCompatibility::Compatible
    } else if is_unknown(matched) {
        PatternCompatibility::Unknown
    } else {
        PatternCompatibility::Incompatible
    };
    PatternShape::new(compatibility, false)
}

fn atom_shape(matched: &TypeDescriptor, tag: &str) -> PatternShape {
    match matched {
        TypeDescriptor::Atom(atom) if atom.name() == tag => {
            PatternShape::new(PatternCompatibility::Compatible, true)
        }
        TypeDescriptor::Atom(_) => PatternShape::new(PatternCompatibility::Incompatible, false),
        TypeDescriptor::Enum(variants) => {
            let compatible = matches!(variants.get(tag), Some(None));
            let mut shape = PatternShape::new(
                if compatible {
                    PatternCompatibility::Compatible
                } else {
                    PatternCompatibility::Incompatible
                },
                compatible && variants.len() == 1,
            );
            if compatible {
                shape.covered_variants.insert(tag.to_owned());
                shape.possible_variants.insert(tag.to_owned());
            }
            shape
        }
        matched if is_unknown(matched) => PatternShape::new(PatternCompatibility::Unknown, false),
        _ => PatternShape::new(PatternCompatibility::Incompatible, false),
    }
}

fn is_unknown(matched: &TypeDescriptor) -> bool {
    matches!(
        matched,
        TypeDescriptor::Any
            | TypeDescriptor::Never
            | TypeDescriptor::Bound(_)
            | TypeDescriptor::Inference(_)
            | TypeDescriptor::Union(_)
    )
}

fn combine(left: PatternCompatibility, right: PatternCompatibility) -> PatternCompatibility {
    if left == PatternCompatibility::Incompatible || right == PatternCompatibility::Incompatible {
        PatternCompatibility::Incompatible
    } else if left == PatternCompatibility::Unknown || right == PatternCompatibility::Unknown {
        PatternCompatibility::Unknown
    } else {
        PatternCompatibility::Compatible
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::located;
    use crate::source::{Location, SourceDatabase, TextRange};
    use crate::value::Atom;
    use std::collections::BTreeMap;

    fn location(start: u32) -> Location {
        let mut sources = SourceDatabase::default();
        let source = sources.add("test.telora", "");
        Location::new(source, TextRange::new(start, start + 1).unwrap())
    }

    fn pattern(value: PatternKind, start: u32) -> Pattern {
        located(value, location(start))
    }

    fn binding(name: &str, start: u32) -> Pattern {
        pattern(
            PatternKind::Binding(located(name.to_owned(), location(start))),
            start,
        )
    }

    #[test]
    fn selects_nested_tuple_and_tagged_binding_types() {
        let input = pattern(
            PatternKind::Tagged {
                tag: "Some".into(),
                payload: Box::new(pattern(
                    PatternKind::Tuple(vec![binding("name", 2), binding("age", 3)]),
                    1,
                )),
            },
            0,
        );
        let matched = TypeDescriptor::Tagged {
            tag: Atom::named("Some"),
            payload: Box::new(TypeDescriptor::Tuple(vec![
                TypeDescriptor::String,
                TypeDescriptor::Int,
            ])),
        };
        let analysis = analyze_pattern(&input, &matched);
        assert_eq!(analysis.compatibility, PatternCompatibility::Compatible);
        assert!(analysis.irrefutable);
        assert_eq!(analysis.bindings[0].ty, TypeDescriptor::String);
        assert_eq!(analysis.bindings[1].ty, TypeDescriptor::Int);
    }

    #[test]
    fn unknown_tuple_shape_keeps_bindings_conservative() {
        let input = pattern(
            PatternKind::Tuple(vec![binding("left", 1), binding("right", 2)]),
            0,
        );
        let analysis = analyze_pattern(&input, &TypeDescriptor::Any);
        assert_eq!(analysis.compatibility, PatternCompatibility::Unknown);
        assert!(!analysis.irrefutable);
        assert!(
            analysis
                .bindings
                .iter()
                .all(|binding| binding.ty == TypeDescriptor::Any)
        );
    }

    #[test]
    fn enum_payload_coverage_requires_irrefutable_payload() {
        let matched = TypeDescriptor::Enum(BTreeMap::from([
            ("None".into(), None),
            ("Some".into(), Some(Box::new(TypeDescriptor::Int))),
        ]));
        let binding_payload = pattern(
            PatternKind::Tagged {
                tag: "Some".into(),
                payload: Box::new(binding("value", 1)),
            },
            0,
        );
        let literal_payload = pattern(
            PatternKind::Tagged {
                tag: "Some".into(),
                payload: Box::new(pattern(PatternKind::Int(1), 3)),
            },
            2,
        );
        assert!(
            analyze_pattern(&binding_payload, &matched)
                .covered_variants
                .contains("Some")
        );
        assert!(
            !analyze_pattern(&literal_payload, &matched)
                .covered_variants
                .contains("Some")
        );
    }

    #[test]
    fn duplicate_bindings_keep_the_first_fact() {
        let input = pattern(
            PatternKind::Tuple(vec![binding("item", 1), binding("item", 2)]),
            0,
        );
        let matched = TypeDescriptor::Tuple(vec![TypeDescriptor::Int, TypeDescriptor::String]);
        let analysis = analyze_pattern(&input, &matched);
        assert_eq!(analysis.bindings.len(), 1);
        assert_eq!(analysis.bindings[0].ty, TypeDescriptor::Int);
        assert_eq!(analysis.bindings[0].location, location(1));
        assert_eq!(analysis.duplicates.len(), 1);
        assert_eq!(analysis.duplicates[0].name, "item");
        assert_eq!(analysis.duplicates[0].location, location(2));
    }
}
