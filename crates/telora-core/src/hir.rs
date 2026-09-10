use crate::ast::{
    Binding, BindingKind, Block, Expr, ExprKind, MatchArm, Pattern, PatternKind, Program,
    StringPartKind, TypeArgumentKind,
};
use crate::parser::RecoveredProgram;
use crate::source::Location;
use std::collections::{HashMap, HashSet};

macro_rules! hir_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(u32);

        impl $name {
            pub const fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

hir_id!(HirDefinitionId);
hir_id!(HirReferenceId);
hir_id!(HirExpressionId);
hir_id!(HirConflictId);

#[derive(Clone, Debug)]
pub(crate) enum HirResolveConflict {
    DuplicateDefinition { name: String, definitions: Vec<HirDefinitionId> },
    AmbiguousImport { name: String, candidates: Vec<HirImportOrigin> },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HirDefinitionKind {
    Let,
    DefinitionSlot,
    Type,
    Import,
    Native,
    NativeType,
    Parameter,
    TypeParameter,
    Pattern,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HirResolution {
    Definition(HirDefinitionId),
    External,
    Unresolved,
    Conflicted(HirConflictId),
}

impl HirResolution {
    pub(crate) fn is_unresolved(self) -> bool {
        matches!(self, Self::Unresolved | Self::Conflicted(_))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum HirImportOrigin {
    Bootstrap(u32),
    Host { module: crate::ModuleId, index: u32 },
    Definition { module: crate::ModuleId, definition: HirDefinitionId },
    Export { module: crate::ModuleId, index: u32 },
    Namespace(crate::ModuleId),
}

#[derive(Clone, Copy, Default)]
pub(crate) struct HirExternalName {
    pub(crate) declared: bool,
    pub(crate) member: bool,
}

fn named_external_lookup(
    names: impl IntoIterator<Item = String>,
    members: HashSet<String>,
) -> impl FnMut(&str) -> HirExternalName {
    let names = names.into_iter().collect::<HashSet<_>>();
    move |name| HirExternalName { declared: names.contains(name), member: members.contains(name) }
}

#[derive(Clone, Debug)]
pub struct HirTypeParameter {
    pub id: HirDefinitionId,
    pub name: String,
    pub location: Location,
}

#[derive(Clone, Debug)]
pub struct HirDefinition {
    pub id: HirDefinitionId,
    pub name: String,
    pub kind: HirDefinitionKind,
    pub location: Location,
    pub additional_locations: Vec<Location>,
    pub type_parameters: Vec<HirTypeParameter>,
    pub top_level: bool,
    pub value: Option<HirExpressionId>,
    pub(crate) member_import: Option<Expr>,
}

#[derive(Clone, Debug)]
pub struct HirReference {
    pub id: HirReferenceId,
    pub name: String,
    pub location: Location,
    pub resolution: HirResolution,
}

#[derive(Clone, Debug)]
pub struct HirExpression {
    pub id: HirExpressionId,
    pub location: Location,
    pub parent: Option<HirExpressionId>,
    pub reference: Option<HirReferenceId>,
}

#[derive(Clone, Debug)]
pub(crate) struct HirMemberAccess {
    pub(crate) expression: HirExpressionId,
    pub(crate) receiver: HirExpressionId,
    pub(crate) field: String,
    pub(crate) location: Location,
}

#[derive(Clone, Debug, Default)]
pub struct HirProgram {
    conflicts: Vec<HirResolveConflict>,
    definition_conflicts: Vec<Option<HirConflictId>>,
    exports: Vec<String>,
    definitions: Vec<HirDefinition>,
    definition_locations: Vec<(Location, HirDefinitionId)>,
    definition_dependencies: Vec<Vec<HirDefinitionId>>,
    references: Vec<HirReference>,
    expressions: Vec<HirExpression>,
    expression_children: Vec<Vec<HirExpressionId>>,
    member_patterns: HashSet<Location>,
    tool_roots: HashSet<Location>,
    property_roots: HashSet<Location>,
    definition_import_origins: Vec<Option<HirImportOrigin>>,
    reference_import_origins: Vec<Option<HirImportOrigin>>,
    member_accesses: Vec<HirMemberAccess>,
    expression_import_origins: Vec<Option<HirImportOrigin>>,
}

impl HirProgram {
    pub(crate) fn member_accesses(&self) -> &[HirMemberAccess] { &self.member_accesses }

    pub(crate) fn set_expression_import_origins(&mut self, origins: Vec<Option<HirImportOrigin>>) {
        assert_eq!(origins.len(), self.expressions.len());
        self.expression_import_origins = origins;
    }

    pub(crate) fn expression_import_origin_at(&self, location: Location) -> Option<HirImportOrigin> {
        self.expression_ids_at(location).find_map(|id|
            self.expression_import_origins.get(id.index()).copied().flatten())
    }

    pub(crate) fn set_import_origins(&mut self, origins: &std::collections::BTreeMap<String, HirImportOrigin>) {
        self.definition_import_origins = self.definitions.iter().map(|definition|
            (definition.kind == HirDefinitionKind::Import).then(|| origins.get(&definition.name).copied()).flatten())
            .collect();
        self.reference_import_origins = self.references.iter().map(|reference| match reference.resolution {
            HirResolution::Definition(id) => self.definition_import_origins[id.index()],
            HirResolution::External => origins.get(&reference.name).copied(),
            HirResolution::Unresolved | HirResolution::Conflicted(_) => None,
        }).collect();
    }

    pub(crate) fn definition_import_origin(&self, id: HirDefinitionId) -> Option<HirImportOrigin> {
        self.definition_import_origins.get(id.index()).copied().flatten()
    }

    pub(crate) fn reference_import_origin(&self, id: HirReferenceId) -> Option<HirImportOrigin> {
        self.reference_import_origins.get(id.index()).copied().flatten()
    }

    pub fn resolve(program: &Program, external_names: impl IntoIterator<Item = String>) -> Self {
        Self::resolve_with_member_constructors(program, external_names, HashSet::new())
    }

    pub(crate) fn resolve_with_member_constructors(
        program: &Program,
        external_names: impl IntoIterator<Item = String>,
        external_member_names: HashSet<String>,
    ) -> Self {
        let mut lookup = named_external_lookup(external_names, external_member_names);
        Self::resolve_with_lookup(program, &mut lookup)
    }

    pub(crate) fn resolve_with_lookup(
        program: &Program,
        lookup: &mut dyn FnMut(&str) -> HirExternalName,
    ) -> Self {
        let mut resolver = Resolver::new(lookup, true);
        let mut scopes = Vec::new();
        resolver.index_block(&program.value.body, &mut scopes, true);
        resolver.hir.normalize_order();
        resolver.hir
    }

    pub(crate) fn is_member_pattern(&self, location: Location) -> bool {
        self.member_patterns.contains(&location)
    }

    pub fn resolve_expression(
        expression: &Expr,
        external_names: impl IntoIterator<Item = String>,
    ) -> Self {
        let mut lookup = named_external_lookup(external_names, HashSet::new());
        let mut resolver = Resolver::new(&mut lookup, true);
        resolver.index_expr(expression, &mut Vec::new());
        resolver.hir.normalize_order();
        resolver.hir
    }

    pub fn resolve_recovered(
        program: &RecoveredProgram,
        external_names: impl IntoIterator<Item = String>,
    ) -> Self {
        Self::resolve_recovered_with_member_constructors(program, external_names, HashSet::new())
    }

    pub(crate) fn resolve_recovered_with_member_constructors(
        program: &RecoveredProgram,
        external_names: impl IntoIterator<Item = String>,
        external_member_names: HashSet<String>,
    ) -> Self {
        let mut lookup = named_external_lookup(external_names, external_member_names);
        Self::resolve_recovered_with_lookup(program, &mut lookup)
    }

    pub(crate) fn resolve_recovered_with_lookup(
        program: &RecoveredProgram,
        lookup: &mut dyn FnMut(&str) -> HirExternalName,
    ) -> Self {
        let mut resolver = Resolver::new(lookup, true);
        resolver.index_block_parts(
            &program.bindings,
            program.result.as_ref(),
            &mut Vec::new(),
            true,
        );
        resolver.hir.normalize_order();
        resolver.hir
    }

    pub fn definitions(&self) -> &[HirDefinition] {
        &self.definitions
    }

    pub fn exports(&self) -> &[String] {
        &self.exports
    }

    pub(crate) fn conflict_external(&mut self, name: &str, candidates: Vec<HirImportOrigin>) {
        let id = HirConflictId(self.conflicts.len() as u32);
        self.conflicts.push(HirResolveConflict::AmbiguousImport { name: name.into(), candidates });
        for reference in &mut self.references {
            if reference.name == name && reference.resolution == HirResolution::External {
                reference.resolution = HirResolution::Conflicted(id);
            }
        }
    }

    pub(crate) fn conflicts(&self) -> &[HirResolveConflict] { &self.conflicts }

    pub(crate) fn conflict(&self, id: HirConflictId) -> &HirResolveConflict { &self.conflicts[id.index()] }

    pub(crate) fn resolution_diagnostic(&self, reference: &HirReference) -> crate::source::Diagnostic {
        use crate::source::Diagnostic;
        match reference.resolution {
            HirResolution::Conflicted(id) => match self.conflict(id) {
                HirResolveConflict::DuplicateDefinition { name, definitions } => {
                    let mut diagnostic = Diagnostic::error(format!("duplicate definition {name:?}"),
                        self.definition(definitions[1]).unwrap().location)
                        .with_secondary("first bound here", self.definition(definitions[0]).unwrap().location);
                    for definition in &definitions[2..] {
                        diagnostic = diagnostic.with_secondary("also bound here", self.definition(*definition).unwrap().location);
                    }
                    diagnostic
                }
                HirResolveConflict::AmbiguousImport { name, candidates } => Diagnostic::error(
                    format!("ambiguous import {name:?}: {} candidate sources", candidates.len()), reference.location),
            },
            _ => Diagnostic::error(format!("unknown binding {:?}", reference.name), reference.location),
        }
    }

    pub fn definition(&self, id: HirDefinitionId) -> Option<&HirDefinition> {
        self.definitions.get(id.index())
    }

    pub(crate) fn definition_dependencies(&self, id: HirDefinitionId) -> &[HirDefinitionId] {
        &self.definition_dependencies[id.index()]
    }

    pub(crate) fn definition_at(&self, location: Location, name: &str) -> Option<&HirDefinition> {
        let key = (location.source, location.start, location.end);
        let start = self.definition_locations.partition_point(|(location, _)|
            (location.source, location.start, location.end) < key);
        self.definition_locations[start..].iter()
            .take_while(|(candidate, _)| *candidate == location)
            .map(|(_, id)| &self.definitions[id.index()])
            .find(|definition| definition.name == name)
    }

    pub fn references(&self) -> &[HirReference] {
        &self.references
    }

    pub fn reference(&self, id: HirReferenceId) -> Option<&HirReference> {
        self.references.get(id.index())
    }

    pub(crate) fn reference_at(&self, location: Location, name: &str) -> Option<&HirReference> {
        let key = (location.source, location.start, location.end);
        let start = self.references.partition_point(|reference| {
            let location = reference.location;
            (location.source, location.start, location.end) < key
        });
        self.references[start..].iter()
            .take_while(|reference| reference.location == location)
            .find(|reference| reference.name == name)
    }

    pub fn expressions(&self) -> &[HirExpression] {
        &self.expressions
    }

    pub(crate) fn is_tool_root(&self, location: Location) -> bool {
        self.tool_roots.contains(&location)
    }

    pub(crate) fn is_property_root(&self, location: Location) -> bool {
        self.property_roots.contains(&location)
    }

    pub fn expression(&self, id: HirExpressionId) -> Option<&HirExpression> {
        self.expressions.get(id.index())
    }

    pub fn expression_ids_at(&self, location: Location) -> impl Iterator<Item = HirExpressionId> {
        let key = (location.source, location.start, location.end);
        let start = self.expressions.partition_point(|expression| {
            let location = expression.location;
            (location.source, location.start, location.end) < key
        });
        self.expressions[start..].iter()
            .take_while(move |expression| expression.location == location)
            .map(|expression| expression.id)
    }

    pub(crate) fn expression_children(&self, id: HirExpressionId) -> &[HirExpressionId] {
        &self.expression_children[id.index()]
    }

    pub fn unresolved(&self) -> impl Iterator<Item = &HirReference> {
        self.references
            .iter()
            .filter(|reference| reference.resolution.is_unresolved())
    }

    fn normalize_order(&mut self) {
        self.definitions.sort_by_key(|definition| {
            (
                definition.location.source,
                definition.location.start,
                definition.location.end,
            )
        });
        let mut definitions = vec![HirDefinitionId(0); self.definitions.len()];
        for (index, definition) in self.definitions.iter_mut().enumerate() {
            let old = definition.id;
            let new = HirDefinitionId(index as u32);
            definition.id = new;
            definitions[old.index()] = new;
        }
        for reference in &mut self.references {
            if let HirResolution::Definition(definition) = reference.resolution {
                reference.resolution = self.definition_conflicts[definition.index()]
                    .map(HirResolution::Conflicted)
                    .unwrap_or(HirResolution::Definition(definitions[definition.index()]));
            }
        }
        for conflict in &mut self.conflicts {
            if let HirResolveConflict::DuplicateDefinition { definitions: candidates, .. } = conflict {
                for candidate in candidates { *candidate = definitions[candidate.index()]; }
            }
        }
        let mut definition_conflicts = vec![None; self.definition_conflicts.len()];
        for (old, new) in definitions.iter().enumerate() {
            definition_conflicts[new.index()] = self.definition_conflicts[old];
        }
        self.definition_conflicts = definition_conflicts;
        for definition in &mut self.definitions {
            for parameter in &mut definition.type_parameters {
                parameter.id = definitions[parameter.id.index()];
            }
        }
        self.definition_locations = self.definitions.iter().flat_map(|definition|
            std::iter::once(definition.location).chain(definition.additional_locations.iter().copied())
                .map(|location| (location, definition.id))).collect();
        self.definition_locations.sort_by_key(|(location, _)|
            (location.source, location.start, location.end));

        self.references.sort_by_key(|reference| {
            (
                reference.location.source,
                reference.location.start,
                reference.location.end,
            )
        });
        let mut references = vec![HirReferenceId(0); self.references.len()];
        for (index, reference) in self.references.iter_mut().enumerate() {
            let old = reference.id;
            let new = HirReferenceId(index as u32);
            reference.id = new;
            references[old.index()] = new;
        }
        for expression in &mut self.expressions {
            expression.reference = expression.reference.map(|id| references[id.index()]);
        }

        self.expressions.sort_by_key(|expression| {
            (
                expression.location.source,
                expression.location.start,
                expression.location.end,
            )
        });
        let mut expressions = vec![HirExpressionId(0); self.expressions.len()];
        for (index, expression) in self.expressions.iter_mut().enumerate() {
            let old = expression.id;
            expression.id = HirExpressionId(index as u32);
            expressions[old.index()] = expression.id;
        }
        for definition in &mut self.definitions {
            definition.value = definition.value.map(|id| expressions[id.index()]);
        }
        for expression in &mut self.expressions {
            expression.parent = expression.parent.map(|id| expressions[id.index()]);
        }
        for member in &mut self.member_accesses {
            member.expression = expressions[member.expression.index()];
            member.receiver = expressions[member.receiver.index()];
        }
        self.expression_children = vec![Vec::new(); self.expressions.len()];
        for expression in &self.expressions {
            if let Some(parent) = expression.parent {
                self.expression_children[parent.index()].push(expression.id);
            }
        }
        let mut owners = vec![Vec::new(); self.expressions.len()];
        for definition in &self.definitions {
            if let Some(value) = definition.value {
                owners[value.index()].push(definition.id);
            }
        }
        self.definition_dependencies = vec![Vec::new(); self.definitions.len()];
        for expression in &self.expressions {
            let Some(reference) = expression.reference else { continue; };
            let HirResolution::Definition(target) = self.references[reference.index()].resolution else { continue; };
            let mut current = Some(expression.id);
            while let Some(id) = current {
                for owner in &owners[id.index()] {
                    self.definition_dependencies[owner.index()].push(target);
                }
                current = self.expressions[id.index()].parent;
            }
        }
        for dependencies in &mut self.definition_dependencies {
            dependencies.sort_unstable();
            dependencies.dedup();
        }
    }
}

type Scope = HashMap<String, HirDefinitionId>;

struct Resolver<'a> {
    hir: HirProgram,
    parameter_definitions: HashMap<Location, HirDefinitionId>,
    external_lookup: &'a mut dyn FnMut(&str) -> HirExternalName,
    expression_stack: Vec<HirExpressionId>,
    static_expressions: bool,
}

impl<'a> Resolver<'a> {
    fn new(external_lookup: &'a mut dyn FnMut(&str) -> HirExternalName, static_expressions: bool) -> Self {
        Self { hir: HirProgram::default(), parameter_definitions: HashMap::new(), external_lookup,
            expression_stack: Vec::new(), static_expressions }
    }

    fn external_member(&mut self, name: &str) -> bool {
        (self.external_lookup)(name).member
    }

    fn define(&mut self, binding: &Binding, scope: &mut Scope, top_level: bool) -> HirDefinitionId {
        let name = binding.value.name.value.as_str();
        let kind = match binding.value.kind {
            BindingKind::Let => HirDefinitionKind::Let,
            BindingKind::Decl | BindingKind::Def => HirDefinitionKind::DefinitionSlot,
            BindingKind::Type => HirDefinitionKind::Type,
            BindingKind::Trait => HirDefinitionKind::Type,
            BindingKind::Impl => HirDefinitionKind::DefinitionSlot,
            BindingKind::Import => HirDefinitionKind::Import,
            BindingKind::OpenImport => HirDefinitionKind::Import,
            BindingKind::Export => HirDefinitionKind::Import,
            BindingKind::Native => HirDefinitionKind::Native,
            BindingKind::NativeType => HirDefinitionKind::NativeType,
        };
        let id = self.define_name(name, kind, binding.value.name.location, scope, top_level);
        self.hir.definitions[id.index()].member_import = binding.value.is_member_import()
            .then(|| binding.value.value.clone());
        id
    }

    fn define_name(
        &mut self,
        name: &str,
        kind: HirDefinitionKind,
        location: Location,
        scope: &mut Scope,
        top_level: bool,
    ) -> HirDefinitionId {
        let id = HirDefinitionId(self.hir.definitions.len() as u32);
        self.hir.definitions.push(HirDefinition {
            id,
            name: name.into(),
            kind,
            location,
            additional_locations: Vec::new(),
            type_parameters: Vec::new(),
            top_level,
            value: None,
            member_import: None,
        });
        self.hir.definition_conflicts.push(None);
        if kind != HirDefinitionKind::Let && let Some(previous) = scope.get(name).copied() {
            let conflict = if let Some(conflict) = self.hir.definition_conflicts[previous.index()] {
                let HirResolveConflict::DuplicateDefinition { definitions, .. } = &mut self.hir.conflicts[conflict.index()] else { unreachable!() };
                definitions.push(id);
                conflict
            } else {
                let conflict = HirConflictId(self.hir.conflicts.len() as u32);
                self.hir.conflicts.push(HirResolveConflict::DuplicateDefinition {
                    name: name.into(), definitions: vec![previous, id],
                });
                self.hir.definition_conflicts[previous.index()] = Some(conflict);
                conflict
            };
            self.hir.definition_conflicts[id.index()] = Some(conflict);
        }
        scope.insert(name.into(), id);
        id
    }

    fn index_block(&mut self, block: &Block, scopes: &mut Vec<Scope>, top_level: bool) {
        self.index_block_parts(
            &block.value.bindings,
            Some(&block.value.result),
            scopes,
            top_level,
        );
    }

    fn index_block_parts(
        &mut self,
        bindings: &[Binding],
        result: Option<&Expr>,
        scopes: &mut Vec<Scope>,
        top_level: bool,
    ) {
        if top_level {
            self.hir.exports = match result.map(|result| &result.value) {
                Some(ExprKind::Dict(fields)) => fields.iter()
                    .filter_map(|field| field.value.name.as_ref().map(|name| name.value.clone())).collect(),
                _ => bindings.iter().filter(|binding| binding.value.kind == BindingKind::Export)
                    .map(|binding| binding.value.name.value.clone()).collect(),
            };
        }
        scopes.push(Scope::new());
        for binding in bindings {
            if matches!(
                binding.value.kind,
                BindingKind::Decl
                    | BindingKind::Native
                    | BindingKind::NativeType
                    | BindingKind::Type
                    | BindingKind::Trait
                    | BindingKind::Impl
            ) || binding.value.kind == BindingKind::Def && binding.value.annotation.is_some()
                || binding.value.kind == BindingKind::Def
                    && matches!(binding.value.value.value, ExprKind::Closure { .. })
                    && resolve_name(scopes, &binding.value.name.value).is_none()
            {
                self.define(
                    binding,
                    scopes.last_mut().expect("block has a scope"),
                    top_level,
                );
            }
        }
        for binding in bindings {
            if matches!(
                binding.value.kind,
                BindingKind::OpenImport | BindingKind::Export
            ) {
                continue;
            }
            if self.static_expressions
                && let Some(annotation) = &binding.value.annotation
            {
                self.index_binding_expr(binding, annotation, scopes);
            }
            match binding.value.kind {
                BindingKind::Let | BindingKind::Import => {
                    let value = self.index_expr(&binding.value.value, scopes);
                    let definition = self.define(
                        binding,
                        scopes.last_mut().expect("block has a scope"),
                        top_level,
                    );
                    self.hir.definitions[definition.index()].value = Some(value);
                }
                BindingKind::Def | BindingKind::Impl => {
                    let definition =
                        if let Some(id) = self.hir.definitions.iter()
                            .find(|definition| definition.location == binding.value.name.location).map(|definition| definition.id)
                            .or_else(|| scopes.last().unwrap().get(&binding.value.name.value).copied()
                                .filter(|id| self.hir.definitions[id.index()].kind == HirDefinitionKind::DefinitionSlot
                                    && (self.hir.definitions[id.index()].value.is_none()
                                        || self.hir.definitions[id.index()].additional_locations.is_empty()
                                            && bindings.iter().any(|binding| binding.value.kind == BindingKind::Decl
                                                && binding.value.name.location == self.hir.definitions[id.index()].location)))) {
                            if binding.value.annotation.is_none()
                                && self.hir.definitions[id.index()].location
                                    != binding.value.name.location
                            {
                                self.hir.definitions[id.index()]
                                    .additional_locations
                                    .push(binding.value.name.location);
                            }
                            id
                        } else {
                            self.define(
                                binding,
                                scopes.last_mut().expect("block has a scope"),
                                top_level,
                            )
                        };
                    let value = if binding.value.annotation.is_some() {
                        self.index_binding_expr(binding, &binding.value.value, scopes)
                    } else {
                        self.index_expr(&binding.value.value, scopes)
                    };
                    self.hir.definitions[definition.index()].value = Some(value);
                }
                BindingKind::Decl
                | BindingKind::Native
                | BindingKind::NativeType
                | BindingKind::Type
                | BindingKind::Trait => {
                    let value = self.index_binding_expr(binding, &binding.value.value, scopes);
                    let definition = self
                        .hir
                        .definitions
                        .iter()
                        .find(|definition| definition.location == binding.value.name.location)
                        .map(|definition| definition.id)
                        .expect("predeclared binding has a definition");
                    self.hir.definitions[definition.index()].value = Some(value);
                }
                BindingKind::OpenImport => unreachable!("open imports are dependency edges"),
                BindingKind::Export => unreachable!("exports are module interface edges"),
            }
        }
        if let Some(result) = result {
            self.index_expr(result, scopes);
        }
        scopes.pop();
    }

    fn index_binding_expr(
        &mut self,
        binding: &Binding,
        expression: &Expr,
        scopes: &mut Vec<Scope>,
    ) -> HirExpressionId {
        if self.static_expressions && (matches!(binding.value.kind, BindingKind::Type | BindingKind::Trait)
            || binding.value.annotation.as_ref().is_some_and(|annotation| annotation.location == expression.location))
        {
            self.hir.tool_roots.insert(expression.location);
        }
        let mut parameter_scope = Scope::new();
        let parameters = binding.value.type_parameters.iter().map(|parameter| {
            // Annotation and body indexing can visit the same authored binder.
            // Its source identity is allocated once, independent of either visit.
            let id = if let Some(id) = self.parameter_definitions.get(&parameter.location).copied() {
                parameter_scope.insert(parameter.value.clone(), id);
                id
            } else {
                let id = self.define_name(&parameter.value, HirDefinitionKind::TypeParameter,
                    parameter.location, &mut parameter_scope, false);
                self.parameter_definitions.insert(parameter.location, id);
                id
            };
            HirTypeParameter { id, name: parameter.value.clone(), location: parameter.location }
        }).collect::<Vec<_>>();
        if !parameters.is_empty() {
            let owner = self.hir.definitions.iter_mut().find(|definition|
                definition.location == binding.value.name.location
                    || definition.additional_locations.contains(&binding.value.name.location))
                .expect("type parameters belong to a declared binding");
            for parameter in parameters {
                if !owner.type_parameters.iter().any(|existing| existing.id == parameter.id) {
                    owner.type_parameters.push(parameter);
                }
            }
        }
        scopes.push(parameter_scope);
        let expression = self.index_expr(expression, scopes);
        if self.static_expressions {
            self.expression_stack.push(expression);
            for bounds in &binding.value.type_parameter_bounds {
                for bound in bounds {
                    if let ExprKind::Call { callee, arguments } = &bound.value
                        && matches!(&callee.value, ExprKind::Variable(name) if name.value == "Property")
                    {
                        for argument in arguments { self.index_tool_expr(argument, scopes); }
                    } else { self.index_tool_expr(bound, scopes); }
                }
            }
            self.expression_stack.pop();
        }
        if self.static_expressions && binding.value.kind == BindingKind::Type {
            self.expression_stack.push(expression);
            for decorator in &binding.value.decorators {
                if matches!(&decorator.value.callee.value,
                    ExprKind::Variable(name) if name.value == "property")
                {
                    // The intrinsic creates a static capability record. Make
                    // its type dependency explicit at the keyword's location.
                    let location = decorator.value.callee.location;
                    let name = crate::ast::located("PropertyAttr".to_owned(), location);
                    self.index_tool_expr(&crate::ast::located(ExprKind::Variable(name), location), scopes);
                }
                let intrinsic_property = matches!(
                    &decorator.value.callee.value,
                    ExprKind::Variable(name) if matches!(name.value.as_str(), "property" | "check")
                );
                if !intrinsic_property {
                    self.hir.property_roots.insert(decorator.value.callee.location);
                    self.index_expr(&decorator.value.callee, scopes);
                }
                for argument in &decorator.value.arguments {
                    if !matches!(&decorator.value.callee.value,
                        ExprKind::Variable(name) if name.value == "check")
                    {
                        self.hir.property_roots.insert(argument.location);
                    }
                    self.index_expr(argument, scopes);
                }
            }
            self.expression_stack.pop();
        }
        scopes.pop();
        expression
    }

    fn index_tool_expr(&mut self, expression: &Expr, scopes: &mut Vec<Scope>) -> HirExpressionId {
        if self.static_expressions {
            self.hir.tool_roots.insert(expression.location);
        }
        self.index_expr(expression, scopes)
    }

    fn index_expr(&mut self, expression: &Expr, scopes: &mut Vec<Scope>) -> HirExpressionId {
        let expression_id = HirExpressionId(self.hir.expressions.len() as u32);
        self.hir.expressions.push(HirExpression {
            id: expression_id,
            location: expression.location,
            parent: self.expression_stack.last().copied(),
            reference: None,
        });
        self.expression_stack.push(expression_id);
        let reference = match &expression.value {
            ExprKind::Variable(name) => {
                let resolution = resolve_name(scopes, &name.value).map_or_else(
                    || {
                        if (self.external_lookup)(&name.value).declared {
                            HirResolution::External
                        } else {
                            HirResolution::Unresolved
                        }
                    },
                    HirResolution::Definition,
                );
                let id = HirReferenceId(self.hir.references.len() as u32);
                self.hir.references.push(HirReference {
                    id,
                    name: name.value.clone(),
                    location: name.location,
                    resolution,
                });
                Some(id)
            }
            ExprKind::InterpolatedString(parts) => {
                for part in parts {
                    if let StringPartKind::Expression(expression) = &part.value {
                        self.index_expr(expression, scopes);
                    }
                }
                None
            }
            ExprKind::Array(items) | ExprKind::Tuple(items) => {
                for item in items {
                    self.index_expr(item, scopes);
                }
                None
            }
            ExprKind::TypeSyntax(operand) | ExprKind::TypeMetadata(operand) => {
                self.index_tool_expr(operand, scopes);
                None
            }
            ExprKind::Spread(operand) => {
                self.index_expr(operand, scopes);
                None
            }
            ExprKind::Dict(fields) => {
                for field in fields {
                    if self.static_expressions {
                        for decorator in &field.value.decorators {
                            let intrinsic_property = matches!(
                                &decorator.value.callee.value,
                                ExprKind::Variable(name) if matches!(name.value.as_str(), "property" | "check")
                            );
                            if !intrinsic_property {
                                self.hir.property_roots.insert(decorator.value.callee.location);
                                self.index_expr(&decorator.value.callee, scopes);
                            }
                            for argument in &decorator.value.arguments {
                                if !matches!(&decorator.value.callee.value,
                                    ExprKind::Variable(name) if name.value == "check")
                                {
                                    self.hir.property_roots.insert(argument.location);
                                }
                                self.index_expr(argument, scopes);
                            }
                        }
                    }
                    self.index_expr(&field.value.value, scopes);
                }
                None
            }
            ExprKind::Block(block) => {
                self.index_block(block, scopes, false);
                None
            }
            ExprKind::Unary { operand, .. } | ExprKind::Propagate { operand } => {
                self.index_expr(operand, scopes);
                None
            }
            ExprKind::Return { value } => {
                self.index_expr(value, scopes);
                None
            }
            ExprKind::Panic { message } => {
                self.index_expr(message, scopes);
                None
            }
            ExprKind::Raise { message, subjects, .. } => {
                self.index_expr(message, scopes);
                for subject in subjects { self.index_expr(subject, scopes); }
                None
            }
            ExprKind::Debug { value, .. } => {
                self.index_expr(value, scopes);
                None
            }
            ExprKind::Binary { left, right, .. } => {
                self.index_expr(left, scopes);
                self.index_expr(right, scopes);
                None
            }
            ExprKind::Field { receiver, field } => {
                let receiver = self.index_expr(receiver, scopes);
                self.hir.member_accesses.push(HirMemberAccess { expression: expression_id,
                    receiver, field: field.value.clone(), location: field.location });
                None
            }
            ExprKind::FieldProjection { receiver, .. } => {
                self.index_expr(receiver, scopes);
                None
            }
            ExprKind::Index { receiver, index } => {
                self.index_expr(receiver, scopes);
                self.index_expr(index, scopes);
                None
            }
            ExprKind::TupleProjection { receiver, .. } => {
                self.index_expr(receiver, scopes);
                None
            }
            ExprKind::TypeAscription { value, target }
            | ExprKind::CheckedCast { value, target } => {
                self.index_expr(value, scopes);
                self.index_tool_expr(target, scopes);
                None
            }
            ExprKind::DynProject {
                namespace,
                target,
                value,
            } => {
                self.index_expr(namespace, scopes);
                self.index_tool_expr(target, scopes);
                self.index_expr(value, scopes);
                None
            }
            ExprKind::Call { callee, arguments } => {
                self.index_expr(callee, scopes);
                for argument in arguments {
                    self.index_expr(argument, scopes);
                }
                None
            }
            ExprKind::TypeApply { callee, arguments } => {
                self.index_expr(callee, scopes);
                if self.static_expressions {
                    for argument in arguments {
                        match &argument.value {
                            TypeArgumentKind::Explicit(argument) => {
                                self.index_tool_expr(argument, scopes);
                            }
                            TypeArgumentKind::Infer => {
                                let id = HirExpressionId(self.hir.expressions.len() as u32);
                                self.hir.expressions.push(HirExpression {
                                    id,
                                    location: argument.location,
                                    parent: self.expression_stack.last().copied(),
                                    reference: None,
                                });
                            }
                        }
                    }
                }
                None
            }
            ExprKind::Interpreter { operand, .. } => {
                self.index_expr(operand, scopes);
                None
            }
            ExprKind::Closure {
                parameters,
                result_annotation,
                body,
            } => {
                if self.static_expressions {
                    for parameter in parameters {
                        if let Some(annotation) = &parameter.annotation {
                            self.index_tool_expr(annotation, scopes);
                        }
                    }
                    if let Some(annotation) = result_annotation {
                        self.index_tool_expr(annotation, scopes);
                    }
                }
                scopes.push(Scope::new());
                for parameter in parameters {
                    self.define_name(
                        &parameter.name.value,
                        HirDefinitionKind::Parameter,
                        parameter.name.location,
                        scopes.last_mut().expect("closure has a scope"),
                        false,
                    );
                }
                self.index_block(body, scopes, false);
                scopes.pop();
                None
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.index_expr(condition, scopes);
                self.index_block(then_branch, scopes, false);
                self.index_block(else_branch, scopes, false);
                None
            }
            ExprKind::IfLet {
                pattern,
                value,
                then_branch,
                else_branch,
            } => {
                self.index_expr(value, scopes);
                scopes.push(Scope::new());
                self.index_pattern_constructors(pattern, scopes);
                self.index_pattern(pattern, scopes.last_mut().expect("if let has a scope"));
                self.index_block(then_branch, scopes, false);
                scopes.pop();
                self.index_block(else_branch, scopes, false);
                None
            }
            ExprKind::LetElse {
                pattern,
                value,
                else_branch,
                body,
            } => {
                self.index_expr(value, scopes);
                self.index_block(else_branch, scopes, false);
                scopes.push(Scope::new());
                self.index_pattern_constructors(pattern, scopes);
                self.index_pattern(pattern, scopes.last_mut().expect("let else has a scope"));
                self.index_block(body, scopes, false);
                scopes.pop();
                None
            }
            ExprKind::Match { value, arms } => {
                self.index_expr(value, scopes);
                for arm in arms {
                    self.index_arm(arm, scopes);
                }
                None
            }
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::String(_)
            | ExprKind::Bytes(_)
            | ExprKind::Atom(_) => None,
        };
        self.expression_stack.pop();
        self.hir.expressions[expression_id.index()].reference = reference;
        expression_id
    }

    fn index_arm(&mut self, arm: &MatchArm, scopes: &mut Vec<Scope>) {
        scopes.push(Scope::new());
        self.index_pattern_constructors(&arm.value.pattern, scopes);
        self.index_pattern(
            &arm.value.pattern,
            scopes.last_mut().expect("arm has a scope"),
        );
        if let Some(guard) = &arm.value.guard {
            self.index_expr(guard, scopes);
        }
        self.index_expr(&arm.value.value, scopes);
        scopes.pop();
    }

    fn index_pattern(&mut self, pattern: &Pattern, scope: &mut Scope) {
        match &pattern.value {
            PatternKind::Binding(name) => {
                if self.hir.is_member_pattern(name.location) { return; }
                // Pattern analysis retains the first binding when diagnosing
                // duplicates. References must resolve to that same definition.
                if let Some(id) = scope.get(&name.value).copied() {
                    self.hir.definitions[id.index()].additional_locations.push(name.location);
                    return;
                }
                self.define_name(
                    &name.value,
                    HirDefinitionKind::Pattern,
                    name.location,
                    scope,
                    false,
                );
            }
            PatternKind::Tuple(items) => {
                for item in items {
                    self.index_pattern(item, scope);
                }
            }
            PatternKind::Tagged { payload, .. } | PatternKind::Constructor { payload: Some(payload), .. } => self.index_pattern(payload, scope),
            PatternKind::Struct(fields) => {
                for field in fields {
                    self.index_pattern(&field.pattern, scope);
                }
            }
            _ => {}
        }
    }

    fn index_pattern_constructors(&mut self, pattern: &Pattern, scopes: &mut Vec<Scope>) {
        match &pattern.value {
            PatternKind::Binding(name) => {
                let member = match resolve_name(scopes, &name.value) {
                    None => self.external_member(&name.value),
                    Some(id) => {
                        let definition = &self.hir.definitions[id.index()];
                        definition.member_import.is_some()
                            || definition.kind == HirDefinitionKind::Import
                                && self.external_member(&name.value)
                    },
                };
                if member {
                    self.hir.member_patterns.insert(name.location);
                    self.index_expr(&crate::ast::located(ExprKind::Variable(name.clone()), name.location), scopes);
                }
            }
            PatternKind::Constructor { constructor, payload } => {
                self.index_expr(constructor, scopes);
                if let Some(payload) = payload { self.index_pattern_constructors(payload, scopes); }
            }
            PatternKind::Tagged { payload, .. } => self.index_pattern_constructors(payload, scopes),
            PatternKind::Tuple(items) => {
                for item in items { self.index_pattern_constructors(item, scopes); }
            }
            PatternKind::Struct(fields) => {
                for field in fields { self.index_pattern_constructors(&field.pattern, scopes); }
            }
            _ => {}
        }
    }
}

fn resolve_name(scopes: &[Scope], name: &str) -> Option<HirDefinitionId> {
    scopes
        .iter()
        .rev()
        .find_map(|scope| scope.get(name).copied())
}

#[cfg(test)]
#[path = "hir/tests/mod.rs"]
mod tests;
