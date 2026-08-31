use crate::ast::Program;
use crate::hir::{HirDefinitionId, HirProgram, HirResolution};
use crate::module_id::ModuleCName;
use crate::source::{Diagnostic, Location, SourceDatabase, SourceId};
use crate::types::{
    Analysis, AnalysisTypeId, ModuleInterface, PartialAnalysis, TypeGraph, TypeNode,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

macro_rules! compact_id {
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

compact_id!(WorkspaceModuleId);
compact_id!(DefinitionId);
compact_id!(ReferenceId);
compact_id!(WorkspaceExpressionId);
compact_id!(WorkspaceTypeId);
compact_id!(DiagnosticId);

impl DiagnosticId {
    pub(crate) const fn from_index(index: usize) -> Self {
        Self(index as u32)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FactIdentity {
    HirDefinition(HirDefinitionId),
    Definition(DefinitionId),
    Expression(WorkspaceExpressionId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UnknownReason {
    MissingSyntax,
    InvalidSyntax,
    UnresolvedName,
    BlockedBy(FactIdentity),
    UnavailableDependency,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Conflict {
    DuplicateDefinition,
    IncompatibleContract,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IncomputableReason {
    QuotaExceeded,
    RuntimeOnly,
    UnsupportedOperation,
    CyclicEvaluation,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FactState {
    Known,
    Unknown(UnknownReason),
    Conflicted(Conflict),
    Incomputable(IncomputableReason),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticFact<T> {
    pub value: Option<T>,
    pub state: FactState,
    pub causes: Vec<FactIdentity>,
    pub diagnostics: Vec<DiagnosticId>,
}

impl<T> SemanticFact<T> {
    pub fn known(value: T) -> Self {
        Self {
            value: Some(value),
            state: FactState::Known,
            causes: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    pub fn unknown(reason: UnknownReason) -> Self {
        Self {
            value: None,
            state: FactState::Unknown(reason),
            causes: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    pub fn conflicted(value: Option<T>, conflict: Conflict) -> Self {
        Self {
            value,
            state: FactState::Conflicted(conflict),
            causes: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    pub fn incomputable(value: Option<T>, reason: IncomputableReason) -> Self {
        Self {
            value,
            state: FactState::Incomputable(reason),
            causes: Vec::new(),
            diagnostics: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceModuleKind {
    Telora,
    Json,
    Toml,
    Yaml,
    Core,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceModuleState {
    Available,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceImport {
    pub name: String,
    pub location: Location,
    pub target: WorkspaceModuleId,
    pub namespace: bool,
}

#[derive(Clone, Debug)]
pub struct WorkspaceModule {
    pub id: WorkspaceModuleId,
    pub name: String,
    pub path: Option<PathBuf>,
    pub kind: WorkspaceModuleKind,
    pub state: WorkspaceModuleState,
    pub source: Option<SourceId>,
    pub imports: Vec<WorkspaceImport>,
    pub result_location: Option<Location>,
    pub result_type: Option<WorkspaceTypeId>,
    pub export_schemes: BTreeMap<String, String>,
}

pub use crate::hir::HirDefinitionKind as DefinitionKind;

#[derive(Clone, Debug)]
pub struct Definition {
    pub id: DefinitionId,
    pub module: WorkspaceModuleId,
    pub name: String,
    pub kind: DefinitionKind,
    pub location: Location,
    pub additional_locations: Vec<Location>,
    pub top_level: bool,
    pub ty: SemanticFact<WorkspaceTypeId>,
    pub scheme: Option<String>,
    pub import_target: Option<WorkspaceModuleId>,
    pub import_namespace: bool,
}

impl Definition {
    fn contains(&self, location: Location) -> bool {
        contains(self.location, location)
            || self
                .additional_locations
                .iter()
                .any(|candidate| contains(*candidate, location))
    }
}

#[derive(Clone, Debug)]
pub struct Reference {
    pub id: ReferenceId,
    pub module: WorkspaceModuleId,
    pub name: String,
    pub location: Location,
    pub definition: Option<DefinitionId>,
    pub external: bool,
}

#[derive(Clone, Debug)]
pub struct WorkspaceExpression {
    pub id: WorkspaceExpressionId,
    pub module: WorkspaceModuleId,
    pub location: Location,
    pub reference: Option<ReferenceId>,
    pub ty: SemanticFact<WorkspaceTypeId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceExport {
    pub name: String,
    pub ty: WorkspaceTypeId,
    pub scheme: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CompletionKind {
    ModuleExport,
    StructField,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletionCandidate {
    pub label: String,
    pub kind: CompletionKind,
    pub ty: WorkspaceTypeId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletionResult {
    pub replacement: crate::source::TextRange,
    pub candidates: Vec<CompletionCandidate>,
}

struct CompletionContext {
    receiver_offset: Option<u32>,
    receiver_name: Option<String>,
    replacement: crate::source::TextRange,
    prefix: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceTypeNode {
    Pending,
    Ref(WorkspaceTypeId),
    Bound(u32),
    Declared {
        name: String,
        body: WorkspaceTypeId,
    },
    Any,
    Never,
    Type,
    Dyn,
    TypeOf(WorkspaceTypeId),
    Int,
    Float,
    String,
    Bytes,
    AtomValue,
    Opaque(String),
    Atom(String),
    Array(WorkspaceTypeId),
    Dict(WorkspaceTypeId),
    Tagged {
        tag: String,
        payload: WorkspaceTypeId,
    },
    Tuple(Vec<WorkspaceTypeId>),
    Struct(BTreeMap<String, WorkspaceTypeId>),
    Enum(BTreeMap<String, Option<WorkspaceTypeId>>),
    Union(Vec<WorkspaceTypeId>),
    Function {
        parameters: Vec<WorkspaceTypeId>,
        result: WorkspaceTypeId,
    },
}

#[derive(Clone, Debug, Default)]
pub struct WorkspaceTypeGraph {
    nodes: Vec<WorkspaceTypeNode>,
    names: BTreeMap<String, WorkspaceTypeId>,
}

impl WorkspaceTypeGraph {
    pub fn nodes(&self) -> impl ExactSizeIterator<Item = (WorkspaceTypeId, &WorkspaceTypeNode)> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (WorkspaceTypeId(index as u32), node))
    }

    pub fn node(&self, id: WorkspaceTypeId) -> Option<&WorkspaceTypeNode> {
        self.nodes.get(id.index())
    }

    pub fn names(&self) -> impl Iterator<Item = (&str, WorkspaceTypeId)> {
        self.names.iter().map(|(name, id)| (name.as_str(), *id))
    }

    pub fn display(&self, id: WorkspaceTypeId) -> Option<String> {
        self.node(id)?;
        Some(self.display_with(id, &mut HashSet::new()))
    }

    pub fn members_of(&self, id: WorkspaceTypeId) -> Vec<WorkspaceExport> {
        let mut current = id;
        let mut visited = HashSet::new();
        loop {
            if !visited.insert(current) {
                return Vec::new();
            }
            match self.node(current) {
                Some(WorkspaceTypeNode::Ref(target)) => current = *target,
                Some(WorkspaceTypeNode::Declared { body, .. }) => current = *body,
                Some(WorkspaceTypeNode::Struct(fields)) => {
                    return fields
                        .iter()
                        .map(|(name, ty)| WorkspaceExport {
                            name: name.clone(),
                            ty: *ty,
                            scheme: None,
                        })
                        .collect();
                }
                _ => return Vec::new(),
            }
        }
    }

    fn display_with(&self, id: WorkspaceTypeId, active: &mut HashSet<WorkspaceTypeId>) -> String {
        if !active.insert(id) {
            return self
                .names
                .iter()
                .find_map(|(name, candidate)| (*candidate == id).then(|| name.clone()))
                .unwrap_or_else(|| "recursive".into());
        }
        let shown = match &self.nodes[id.index()] {
            WorkspaceTypeNode::Pending => "<pending>".into(),
            WorkspaceTypeNode::Ref(target) => self.display_with(*target, active),
            WorkspaceTypeNode::Bound(parameter) => format!("T{parameter}"),
            WorkspaceTypeNode::Declared { name, .. } => name.clone(),
            WorkspaceTypeNode::Any => "Any".into(),
            WorkspaceTypeNode::Never => "Never".into(),
            WorkspaceTypeNode::Type => "Type".into(),
            WorkspaceTypeNode::Dyn => "Dyn".into(),
            WorkspaceTypeNode::TypeOf(instance) => {
                format!("TypeOf({})", self.display_with(*instance, active))
            }
            WorkspaceTypeNode::Int => "Int".into(),
            WorkspaceTypeNode::Float => "Float".into(),
            WorkspaceTypeNode::String => "String".into(),
            WorkspaceTypeNode::Bytes => "Bytes".into(),
            WorkspaceTypeNode::AtomValue => "Atom".into(),
            WorkspaceTypeNode::Opaque(name) => format!("opaque({name})"),
            WorkspaceTypeNode::Atom(atom) => format!("'{atom}"),
            WorkspaceTypeNode::Array(item) => {
                format!("Array<{}>", self.display_with(*item, active))
            }
            WorkspaceTypeNode::Dict(item) => {
                format!("Dict<{}>", self.display_with(*item, active))
            }
            WorkspaceTypeNode::Tagged { tag, payload } => {
                format!("'{tag}({})", self.display_with(*payload, active))
            }
            WorkspaceTypeNode::Tuple(items) => format!(
                "({})",
                items
                    .iter()
                    .map(|item| self.display_with(*item, active))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            WorkspaceTypeNode::Struct(fields) => format!(
                "{{{}}}",
                fields
                    .iter()
                    .map(|(name, item)| format!("{name}: {}", self.display_with(*item, active)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            WorkspaceTypeNode::Enum(variants) => format!(
                "enum {{{}}}",
                variants
                    .iter()
                    .map(|(name, payload)| payload.map_or_else(
                        || name.clone(),
                        |payload| format!("{name}({})", self.display_with(payload, active))
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            WorkspaceTypeNode::Union(items) => items
                .iter()
                .map(|item| self.display_with(*item, active))
                .collect::<Vec<_>>()
                .join(" | "),
            WorkspaceTypeNode::Function { parameters, result } => format!(
                "Fn({}) -> {}",
                parameters
                    .iter()
                    .map(|item| self.display_with(*item, active))
                    .collect::<Vec<_>>()
                    .join(", "),
                self.display_with(*result, active)
            ),
        };
        active.remove(&id);
        shown
    }
}

#[derive(Clone, Debug)]
pub struct WorkspaceSnapshot {
    revision: crate::query::Revision,
    sources: SourceDatabase,
    modules: Vec<WorkspaceModule>,
    definitions: Vec<Definition>,
    references: Vec<Reference>,
    expressions: Vec<WorkspaceExpression>,
    types: WorkspaceTypeGraph,
    diagnostics: Vec<Diagnostic>,
}

impl WorkspaceSnapshot {
    pub fn recover_source(source_name: impl Into<String>, source: impl Into<String>) -> Self {
        let source_name = source_name.into();
        let source_text = source.into();
        let partial = crate::types::analyze_partial_types(
            &source_name,
            &source_text,
            crate::vm::Quota::with_fuel(100_000),
        );
        let mut sources = SourceDatabase::default();
        let source = sources.add(source_name.clone(), source_text);
        let parsed = crate::parser::parse_registered(&sources, source);
        let hir = &partial.hir;
        let module = WorkspaceModuleId(0);
        let mut types = WorkspaceTypeGraph::default();
        let type_map = merge_type_graph(&source_name, &partial.types, &mut types);
        let mut definitions = hir
            .definitions()
            .iter()
            .map(|definition| Definition {
                id: DefinitionId(definition.id.index() as u32),
                module,
                name: definition.name.clone(),
                kind: definition.kind,
                location: definition.location,
                additional_locations: definition.additional_locations.clone(),
                top_level: definition.top_level,
                ty: partial.definition_facts.get(&definition.id).map_or_else(
                    || SemanticFact::unknown(UnknownReason::InvalidSyntax),
                    |fact| map_partial_fact(fact, &type_map),
                ),
                scheme: None,
                import_target: None,
                import_namespace: false,
            })
            .collect::<Vec<_>>();
        let mut diagnostics = partial.diagnostics;
        let mut slots = HashMap::<String, Vec<usize>>::new();
        for (index, definition) in definitions.iter().enumerate() {
            if definition.kind == DefinitionKind::DefinitionSlot {
                slots
                    .entry(definition.name.clone())
                    .or_default()
                    .push(index);
            }
        }
        for (name, indices) in slots {
            if indices.len() < 2 {
                continue;
            }
            let location = definitions[indices[1]].location;
            let diagnostic = DiagnosticId(diagnostics.len() as u32);
            diagnostics.push(Diagnostic::error(
                format!("duplicate definition slot {name:?}"),
                location,
            ));
            for index in indices {
                definitions[index].ty =
                    SemanticFact::conflicted(None, Conflict::DuplicateDefinition);
                definitions[index].ty.diagnostics.push(diagnostic);
            }
        }
        let mut indexed_diagnostics = diagnostics.into_iter().enumerate().collect::<Vec<_>>();
        indexed_diagnostics.sort_by_key(|(_, diagnostic)| {
            diagnostic
                .labels
                .first()
                .map_or(0, |label| label.location.start)
        });
        let mut remapped = vec![DiagnosticId::from_index(0); indexed_diagnostics.len()];
        for (new, (old, _)) in indexed_diagnostics.iter().enumerate() {
            remapped[*old] = DiagnosticId::from_index(new);
        }
        for definition in &mut definitions {
            for diagnostic in &mut definition.ty.diagnostics {
                *diagnostic = remapped[diagnostic.index()];
            }
        }
        let diagnostics = indexed_diagnostics
            .into_iter()
            .map(|(_, diagnostic)| diagnostic)
            .collect();
        let references = hir
            .references()
            .iter()
            .map(|reference| Reference {
                id: ReferenceId(reference.id.index() as u32),
                module,
                name: reference.name.clone(),
                location: reference.location,
                definition: match reference.resolution {
                    HirResolution::Definition(definition) => {
                        Some(DefinitionId(definition.index() as u32))
                    }
                    HirResolution::External | HirResolution::Unresolved => None,
                },
                external: reference.resolution == HirResolution::External,
            })
            .collect::<Vec<_>>();
        let expressions = hir
            .expressions()
            .iter()
            .map(|expression| {
                let unresolved = expression
                    .reference
                    .and_then(|reference| hir.reference(reference))
                    .is_some_and(|reference| reference.resolution == HirResolution::Unresolved);
                WorkspaceExpression {
                    id: WorkspaceExpressionId(expression.id.index() as u32),
                    module,
                    location: expression.location,
                    reference: expression
                        .reference
                        .map(|reference| ReferenceId(reference.index() as u32)),
                    ty: SemanticFact::unknown(if unresolved {
                        UnknownReason::UnresolvedName
                    } else {
                        UnknownReason::InvalidSyntax
                    }),
                }
            })
            .collect();
        Self {
            revision: crate::query::Revision::default(),
            sources,
            modules: vec![WorkspaceModule {
                id: module,
                name: source_name,
                path: None,
                kind: WorkspaceModuleKind::Telora,
                state: WorkspaceModuleState::Available,
                source: Some(source),
                imports: Vec::new(),
                result_location: parsed
                    .recovered
                    .result
                    .as_ref()
                    .map(|result| result.location),
                result_type: None,
                export_schemes: BTreeMap::new(),
            }],
            definitions,
            references,
            expressions,
            types,
            diagnostics,
        }
    }

    pub fn sources(&self) -> &SourceDatabase {
        &self.sources
    }

    pub const fn revision(&self) -> crate::query::Revision {
        self.revision
    }

    pub(crate) fn set_revision(&mut self, revision: crate::query::Revision) {
        self.revision = revision;
    }

    pub async fn query_definition_at(
        &self,
        context: &crate::query::QueryContext,
        location: Location,
    ) -> Result<Option<&Definition>, crate::query::QueryError> {
        context.checkpoint().await?;
        context.ensure_snapshot(self.revision)?;
        Ok(self.definition_at(location))
    }

    pub async fn query_reference_at(
        &self,
        context: &crate::query::QueryContext,
        location: Location,
    ) -> Result<Option<&Reference>, crate::query::QueryError> {
        context.checkpoint().await?;
        context.ensure_snapshot(self.revision)?;
        Ok(self.reference_at(location))
    }

    pub async fn query_type_at(
        &self,
        context: &crate::query::QueryContext,
        location: Location,
    ) -> Result<Option<WorkspaceTypeId>, crate::query::QueryError> {
        context.checkpoint().await?;
        context.ensure_snapshot(self.revision)?;
        Ok(self.type_at(location))
    }

    pub async fn query_diagnostics(
        &self,
        context: &crate::query::QueryContext,
    ) -> Result<&[Diagnostic], crate::query::QueryError> {
        context.checkpoint().await?;
        context.ensure_snapshot(self.revision)?;
        Ok(self.diagnostics())
    }

    pub async fn query_exports_of(
        &self,
        context: &crate::query::QueryContext,
        module: WorkspaceModuleId,
    ) -> Result<Vec<WorkspaceExport>, crate::query::QueryError> {
        context.checkpoint().await?;
        context.ensure_snapshot(self.revision)?;
        let exports = self.exports_of(module);
        context.ensure_snapshot(self.revision)?;
        Ok(exports)
    }

    pub async fn query_references_of(
        &self,
        context: &crate::query::QueryContext,
        definition: DefinitionId,
    ) -> Result<Vec<&Reference>, crate::query::QueryError> {
        context.checkpoint().await?;
        context.ensure_snapshot(self.revision)?;
        let mut references = Vec::new();
        for (index, reference) in self.references.iter().enumerate() {
            if index % 256 == 0 {
                context.checkpoint().await?;
            }
            if reference.definition == Some(definition) {
                references.push(reference);
            }
        }
        context.ensure_snapshot(self.revision)?;
        Ok(references)
    }

    pub async fn query_completion_at(
        &self,
        context: &crate::query::QueryContext,
        location: Location,
    ) -> Result<Option<CompletionResult>, crate::query::QueryError> {
        context.checkpoint().await?;
        context.ensure_snapshot(self.revision)?;
        let Some(completion) = self.completion_context(location) else {
            return Ok(None);
        };
        context.checkpoint().await?;

        let Some(receiver_offset) = completion.receiver_offset else {
            return Ok(Some(CompletionResult {
                replacement: completion.replacement,
                candidates: Vec::new(),
            }));
        };
        let receiver = Location::new(
            location.source,
            crate::source::TextRange::at(receiver_offset),
        );
        let definition = self
            .reference_at(receiver)
            .and_then(|reference| reference.definition)
            .and_then(|definition| self.definition(definition))
            .or_else(|| self.definition_at(receiver))
            .or_else(|| {
                let name = completion.receiver_name.as_deref()?;
                let module = self
                    .modules
                    .iter()
                    .find(|module| module.source == Some(location.source))?;
                let mut matches = self.definitions.iter().filter(|definition| {
                    definition.module == module.id
                        && definition.kind == DefinitionKind::Import
                        && definition.name == name
                        && definition.location.end <= receiver_offset
                });
                let definition = matches.next()?;
                matches.next().is_none().then_some(definition)
            });
        let (kind, members) = if let Some(module) = definition.and_then(|item| item.import_target) {
            let exports = self.query_exports_of(context, module).await?;
            let members = if exports.is_empty() {
                definition
                    .and_then(|item| item.ty.value)
                    .map_or_else(Vec::new, |ty| self.types.members_of(ty))
            } else {
                exports
            };
            (CompletionKind::ModuleExport, members)
        } else {
            let members = self
                .type_at(receiver)
                .map_or_else(Vec::new, |ty| self.types.members_of(ty));
            (CompletionKind::StructField, members)
        };

        let mut candidates = Vec::new();
        for (index, member) in members.into_iter().enumerate() {
            if index % 256 == 0 {
                context.checkpoint().await?;
            }
            if member.name.starts_with(&completion.prefix) {
                candidates.push(CompletionCandidate {
                    label: member.name,
                    kind,
                    ty: member.ty,
                });
            }
        }
        candidates.sort_by(|left, right| {
            left.label
                .cmp(&right.label)
                .then_with(|| left.kind.cmp(&right.kind))
        });
        candidates.dedup_by(|left, right| left.label == right.label && left.kind == right.kind);
        context.ensure_snapshot(self.revision)?;
        Ok(Some(CompletionResult {
            replacement: completion.replacement,
            candidates,
        }))
    }

    pub fn modules(&self) -> &[WorkspaceModule] {
        &self.modules
    }

    pub fn module(&self, id: WorkspaceModuleId) -> Option<&WorkspaceModule> {
        self.modules.get(id.index())
    }

    pub fn module_by_path(&self, path: &Path) -> Option<&WorkspaceModule> {
        self.modules
            .iter()
            .find(|module| module.path.as_deref() == Some(path))
    }

    pub fn module_by_source(&self, source: SourceId) -> Option<&WorkspaceModule> {
        self.modules
            .iter()
            .find(|module| module.source == Some(source))
    }

    pub fn definitions(&self) -> &[Definition] {
        &self.definitions
    }

    pub fn definition(&self, id: DefinitionId) -> Option<&Definition> {
        self.definitions.get(id.index())
    }

    pub fn definition_at(&self, location: Location) -> Option<&Definition> {
        self.definitions
            .iter()
            .filter(|definition| definition.contains(location))
            .min_by_key(|definition| definition.location.end - definition.location.start)
    }

    pub fn references(&self) -> &[Reference] {
        &self.references
    }

    pub fn reference(&self, id: ReferenceId) -> Option<&Reference> {
        self.references.get(id.index())
    }

    pub fn reference_at(&self, location: Location) -> Option<&Reference> {
        self.references
            .iter()
            .filter(|reference| contains(reference.location, location))
            .min_by_key(|reference| reference.location.end - reference.location.start)
    }

    pub fn references_of(&self, definition: DefinitionId) -> Vec<&Reference> {
        self.references
            .iter()
            .filter(|reference| reference.definition == Some(definition))
            .collect()
    }

    pub fn expressions(&self) -> &[WorkspaceExpression] {
        &self.expressions
    }

    pub fn expression(&self, id: WorkspaceExpressionId) -> Option<&WorkspaceExpression> {
        self.expressions.get(id.index())
    }

    pub fn expression_at(&self, location: Location) -> Option<&WorkspaceExpression> {
        self.expressions
            .iter()
            .filter(|expression| contains(expression.location, location))
            .min_by_key(|expression| expression.location.end - expression.location.start)
    }

    pub fn type_of_expression(&self, id: WorkspaceExpressionId) -> Option<WorkspaceTypeId> {
        self.expression(id)
            .and_then(|expression| expression.ty.value)
    }

    pub fn fact_of_expression(
        &self,
        id: WorkspaceExpressionId,
    ) -> Option<&SemanticFact<WorkspaceTypeId>> {
        self.expression(id).map(|expression| &expression.ty)
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn type_at(&self, location: Location) -> Option<WorkspaceTypeId> {
        if let Some(reference) = self.reference_at(location)
            && let Some(ty) = reference
                .definition
                .and_then(|id| self.definition(id))
                .and_then(|definition| definition.ty.value)
        {
            return Some(ty);
        }
        if let Some(definition) = self.definition_at(location)
            && let Some(ty) = definition.ty.value
        {
            return Some(ty);
        }
        self.expression_at(location)
            .and_then(|expression| expression.ty.value)
    }

    fn completion_context(&self, location: Location) -> Option<CompletionContext> {
        use crate::syntax::telora::lexer::Token;

        if location.start != location.end {
            return None;
        }
        let file = self.sources.get(location.source);
        let cursor = location.start as usize;
        if cursor > file.text().byte_len() {
            return None;
        }
        let mut diagnostics = Vec::new();
        let (tokens, spans) =
            crate::syntax::telora::lexer::tokenize_document(file.text(), &mut diagnostics);
        let significant = tokens
            .iter()
            .zip(&spans)
            .filter(|(token, _)| !matches!(token, Token::Whitespace | Token::Comment))
            .take_while(|(_, span)| span.end <= cursor)
            .collect::<Vec<_>>();
        let (dot_span, replacement, prefix) = match significant.as_slice() {
            [.., (Token::Dot, dot)] if dot.end == cursor => (
                (*dot).clone(),
                crate::source::TextRange::at(location.start),
                String::new(),
            ),
            [.., (Token::Dot, dot), (Token::Identifier, prefix)]
                if dot.end == prefix.start && prefix.end == cursor =>
            {
                let replacement = crate::source::TextRange::from_usize((*prefix).clone()).ok()?;
                let prefix = file.text().slice(replacement).ok()?.into_owned();
                ((*dot).clone(), replacement, prefix)
            }
            _ => return None,
        };
        let receiver_end = u32::try_from(dot_span.start).ok()?;
        let lexical_receiver = significant
            .iter()
            .rev()
            .find(|(_, span)| span.end <= dot_span.start)
            .and_then(|(token, span)| {
                (matches!(token, Token::Identifier) && span.end == dot_span.start)
                    .then_some((*span).clone())
            });
        let receiver_location = self
            .expressions
            .iter()
            .map(|expression| expression.location)
            .chain(self.references.iter().map(|reference| reference.location))
            .chain(
                self.definitions
                    .iter()
                    .map(|definition| definition.location),
            )
            .filter(|candidate| {
                candidate.source == location.source && candidate.end == receiver_end
            })
            .min_by_key(|candidate| candidate.end - candidate.start);
        let receiver_range = receiver_location.map(Location::text_range).or_else(|| {
            lexical_receiver.and_then(|range| crate::source::TextRange::from_usize(range).ok())
        });
        Some(CompletionContext {
            receiver_offset: receiver_range.and_then(|receiver| receiver.end.checked_sub(1)),
            receiver_name: receiver_range
                .and_then(|receiver| file.text().slice(receiver).ok())
                .map(|name| name.into_owned()),
            replacement,
            prefix,
        })
    }

    pub fn types(&self) -> &WorkspaceTypeGraph {
        &self.types
    }

    pub fn exports_of(&self, module: WorkspaceModuleId) -> Vec<WorkspaceExport> {
        let Some(result) = self.module(module).and_then(|module| module.result_type) else {
            return Vec::new();
        };
        let Some(WorkspaceTypeNode::Struct(fields)) = self.types.node(result) else {
            return Vec::new();
        };
        fields
            .iter()
            .map(|(name, ty)| WorkspaceExport {
                name: name.clone(),
                ty: *ty,
                scheme: self
                    .module(module)
                    .and_then(|module| module.export_schemes.get(name))
                    .cloned(),
            })
            .collect()
    }

    pub(crate) fn build(sources: SourceDatabase, mut inputs: Vec<SemanticModuleInput>) -> Self {
        let mut core_names = inputs
            .iter()
            .flat_map(|input| input.imports.iter())
            .filter_map(|import| match &import.target {
                ModuleCName::Builtin(name) => Some(name.clone()),
                ModuleCName::Source { .. }
                | ModuleCName::Standalone { .. }
                | ModuleCName::Test { .. }
                | ModuleCName::Dependency { .. } => None,
            })
            .collect::<HashSet<_>>();
        for name in core_names.drain() {
            if inputs.iter().any(|input| input.key == name) {
                continue;
            }
            inputs.push(SemanticModuleInput {
                key: name.clone(),
                path: None,
                kind: WorkspaceModuleKind::Core,
                source: None,
                program: None,
                analysis: None,
                partial: None,
                interface: None,
                state: WorkspaceModuleState::Available,
                imports: Vec::new(),
                diagnostics: Vec::new(),
            });
        }
        inputs.sort_by(|left, right| left.key.cmp(&right.key));

        let ids = inputs
            .iter()
            .enumerate()
            .map(|(index, input)| (input.key.clone(), WorkspaceModuleId(index as u32)))
            .collect::<HashMap<_, _>>();
        let mut types = WorkspaceTypeGraph::default();
        let mut type_maps = Vec::with_capacity(inputs.len());
        for input in &inputs {
            let map = input
                .analysis
                .as_ref()
                .map(|analysis| &analysis.types)
                .or_else(|| input.partial.as_ref().map(|partial| &partial.types))
                .or_else(|| input.interface.as_ref().map(|interface| &interface.types))
                .map_or_else(Vec::new, |graph| {
                    merge_type_graph(&input.key, graph, &mut types)
                });
            type_maps.push(map);
        }

        let mut modules = Vec::with_capacity(inputs.len());
        for (index, input) in inputs.iter().enumerate() {
            let id = WorkspaceModuleId(index as u32);
            let imports = input
                .imports
                .iter()
                .map(|import| WorkspaceImport {
                    name: import.name.clone(),
                    location: import.location,
                    target: ids[&import.target.to_string()],
                    namespace: import.namespace,
                })
                .collect();
            let result_type = input
                .analysis
                .as_ref()
                .map(|analysis| type_maps[index][analysis.result_type.index()])
                .or_else(|| {
                    input
                        .interface
                        .as_ref()
                        .map(|interface| type_maps[index][interface.result_type.index()])
                });
            modules.push(WorkspaceModule {
                id,
                name: input.key.clone(),
                path: input.path.clone(),
                kind: input.kind,
                state: input.state,
                source: input.source,
                imports,
                result_location: input
                    .program
                    .as_ref()
                    .map(|program| program.value.body.value.result.location),
                result_type,
                export_schemes: input
                    .analysis
                    .as_ref()
                    .map(|analysis| {
                        analysis
                            .module_interface
                            .exports
                            .iter()
                            .map(|(name, scheme)| (name.clone(), scheme.display_name()))
                            .collect()
                    })
                    .or_else(|| {
                        input
                            .interface
                            .as_ref()
                            .map(|interface| interface.export_schemes.clone())
                    })
                    .unwrap_or_default(),
            });
        }

        let mut diagnostic_records = Vec::new();
        for (input_index, input) in inputs.iter().enumerate() {
            diagnostic_records.extend(
                input
                    .diagnostics
                    .iter()
                    .cloned()
                    .map(|diagnostic| (input_index, None, diagnostic)),
            );
            if let Some(partial) = &input.partial {
                diagnostic_records.extend(
                    partial
                        .diagnostics
                        .iter()
                        .cloned()
                        .enumerate()
                        .map(|(local, diagnostic)| (input_index, Some(local), diagnostic)),
                );
            }
        }
        diagnostic_records.sort_by_key(|(_, _, diagnostic)| {
            diagnostic.labels.first().map_or((0, 0), |label| {
                (label.location.source.get(), label.location.start)
            })
        });
        let mut diagnostic_maps = inputs
            .iter()
            .map(|input| {
                vec![
                    DiagnosticId::from_index(0);
                    input
                        .partial
                        .as_ref()
                        .map_or(0, |partial| partial.diagnostics.len())
                ]
            })
            .collect::<Vec<_>>();
        for (global, (input, local, _)) in diagnostic_records.iter().enumerate() {
            if let Some(local) = local {
                diagnostic_maps[*input][*local] = DiagnosticId::from_index(global);
            }
        }
        let diagnostics = diagnostic_records
            .into_iter()
            .map(|(_, _, diagnostic)| diagnostic)
            .collect::<Vec<_>>();

        let mut definitions = Vec::new();
        let mut definition_maps = Vec::with_capacity(inputs.len());
        for (index, input) in inputs.iter().enumerate() {
            let Some(hir) = input_hir(input) else {
                definition_maps.push(Vec::new());
                continue;
            };
            let import_targets = input
                .imports
                .iter()
                .map(|import| {
                    (
                        import.name.as_str(),
                        (ids[&import.target.to_string()], import.namespace),
                    )
                })
                .collect::<HashMap<_, _>>();
            let module = WorkspaceModuleId(index as u32);
            let mut map = Vec::with_capacity(hir.definitions().len());
            let definition_base = definitions.len();
            for definition in hir.definitions() {
                let id = DefinitionId(definitions.len() as u32);
                let ty = input.analysis.as_ref().map_or_else(
                    || {
                        input
                            .partial
                            .as_ref()
                            .and_then(|partial| partial.definition_facts.get(&definition.id))
                            .map_or_else(
                                || SemanticFact::unknown(UnknownReason::UnavailableDependency),
                                |fact| {
                                    map_partial_fact_with_base(
                                        fact,
                                        &type_maps[index],
                                        definition_base,
                                        Some(&diagnostic_maps[index]),
                                    )
                                },
                            )
                    },
                    |analysis| {
                        analysis
                            .definition_types
                            .get(&definition.id)
                            .map(|local| SemanticFact::known(type_maps[index][local.index()]))
                            .unwrap_or_else(|| {
                                SemanticFact::unknown(UnknownReason::UnavailableDependency)
                            })
                    },
                );
                definitions.push(Definition {
                    id,
                    module,
                    name: definition.name.clone(),
                    kind: definition.kind,
                    location: definition.location,
                    additional_locations: definition.additional_locations.clone(),
                    top_level: definition.top_level,
                    ty,
                    scheme: input
                        .analysis
                        .as_ref()
                        .and_then(|analysis| analysis.definition_schemes.get(&definition.id))
                        .or_else(|| {
                            input
                                .partial
                                .as_ref()
                                .and_then(|partial| partial.definition_schemes.get(&definition.id))
                        })
                        .map(crate::types::TypeScheme::display_name),
                    import_target: (definition.kind == DefinitionKind::Import)
                        .then(|| {
                            import_targets
                                .get(definition.name.as_str())
                                .map(|(target, _)| *target)
                        })
                        .flatten(),
                    import_namespace: definition.kind == DefinitionKind::Import
                        && import_targets
                            .get(definition.name.as_str())
                            .is_some_and(|(_, namespace)| *namespace),
                });
                map.push(id);
            }
            definition_maps.push(map);
        }

        let mut references = Vec::new();
        let mut reference_maps = Vec::with_capacity(inputs.len());
        for (index, input) in inputs.iter().enumerate() {
            let Some(hir) = input_hir(input) else {
                reference_maps.push(Vec::new());
                continue;
            };
            let module = WorkspaceModuleId(index as u32);
            let mut map = Vec::with_capacity(hir.references().len());
            for reference in hir.references() {
                let id = ReferenceId(references.len() as u32);
                references.push(Reference {
                    id,
                    module,
                    name: reference.name.clone(),
                    location: reference.location,
                    definition: match reference.resolution {
                        HirResolution::Definition(definition) => {
                            Some(definition_maps[index][definition.index()])
                        }
                        HirResolution::External | HirResolution::Unresolved => None,
                    },
                    external: reference.resolution == HirResolution::External,
                });
                map.push(id);
            }
            reference_maps.push(map);
        }

        let mut expressions = Vec::new();
        for (index, input) in inputs.iter().enumerate() {
            let Some(hir) = input_hir(input) else {
                continue;
            };
            let module = WorkspaceModuleId(index as u32);
            for expression in hir.expressions() {
                expressions.push(WorkspaceExpression {
                    id: WorkspaceExpressionId(expressions.len() as u32),
                    module,
                    location: expression.location,
                    reference: expression
                        .reference
                        .map(|reference| reference_maps[index][reference.index()]),
                    ty: input.analysis.as_ref().map_or_else(
                        || {
                            SemanticFact::unknown(
                                if expression
                                    .reference
                                    .and_then(|id| hir.reference(id))
                                    .is_some_and(|reference| {
                                        reference.resolution == HirResolution::Unresolved
                                    })
                                {
                                    UnknownReason::UnresolvedName
                                } else {
                                    UnknownReason::UnavailableDependency
                                },
                            )
                        },
                        |analysis| {
                            analysis
                                .expression_types
                                .get(&expression.id)
                                .map(|ty| SemanticFact::known(type_maps[index][ty.index()]))
                                .unwrap_or_else(|| {
                                    let unresolved = expression
                                        .reference
                                        .and_then(|id| hir.reference(id))
                                        .is_some_and(|reference| {
                                            reference.resolution == HirResolution::Unresolved
                                        });
                                    SemanticFact::unknown(if unresolved {
                                        UnknownReason::UnresolvedName
                                    } else {
                                        UnknownReason::UnavailableDependency
                                    })
                                })
                        },
                    ),
                });
            }
        }

        Self {
            revision: crate::query::Revision::default(),
            sources,
            modules,
            definitions,
            references,
            expressions,
            types,
            diagnostics,
        }
    }
}

fn contains(range: Location, point: Location) -> bool {
    range.source == point.source
        && range.start <= point.start
        && (point.start < range.end || range.start == range.end && point.start == range.start)
}

fn map_partial_fact(
    fact: &SemanticFact<AnalysisTypeId>,
    type_map: &[WorkspaceTypeId],
) -> SemanticFact<WorkspaceTypeId> {
    map_partial_fact_with_base(fact, type_map, 0, None)
}

fn map_partial_fact_with_base(
    fact: &SemanticFact<AnalysisTypeId>,
    type_map: &[WorkspaceTypeId],
    definition_base: usize,
    diagnostic_map: Option<&[DiagnosticId]>,
) -> SemanticFact<WorkspaceTypeId> {
    let map_identity = |identity| match identity {
        FactIdentity::HirDefinition(id) => {
            FactIdentity::Definition(DefinitionId((definition_base + id.index()) as u32))
        }
        other => other,
    };
    let state = match &fact.state {
        FactState::Known => FactState::Known,
        FactState::Unknown(UnknownReason::BlockedBy(identity)) => {
            FactState::Unknown(UnknownReason::BlockedBy(map_identity(*identity)))
        }
        state => state.clone(),
    };
    SemanticFact {
        value: fact.value.map(|ty| type_map[ty.index()]),
        state,
        causes: fact.causes.iter().copied().map(map_identity).collect(),
        diagnostics: fact
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic_map.map_or(*diagnostic, |map| map[diagnostic.index()]))
            .collect(),
    }
}

fn input_hir(input: &SemanticModuleInput) -> Option<&HirProgram> {
    input
        .analysis
        .as_ref()
        .map(|analysis| &analysis.hir)
        .or_else(|| input.partial.as_ref().map(|partial| &partial.hir))
}

fn merge_type_graph(
    module: &str,
    source: &TypeGraph,
    target: &mut WorkspaceTypeGraph,
) -> Vec<WorkspaceTypeId> {
    let mut mapped = vec![None; source.nodes().len()];
    for (id, _) in source.nodes() {
        merge_type_node(id, source, target, &mut mapped);
    }
    let mapped = mapped
        .into_iter()
        .map(|id| id.expect("all source type nodes are merged"))
        .collect::<Vec<_>>();
    for (name, id) in source.names() {
        target
            .names
            .insert(format!("{module}::{name}"), mapped[id.index()]);
    }
    mapped
}

fn merge_type_node(
    id: AnalysisTypeId,
    source: &TypeGraph,
    target: &mut WorkspaceTypeGraph,
    mapped: &mut [Option<WorkspaceTypeId>],
) -> WorkspaceTypeId {
    if let Some(id) = mapped[id.index()] {
        return id;
    }
    let output = WorkspaceTypeId(target.nodes.len() as u32);
    target.nodes.push(WorkspaceTypeNode::Pending);
    mapped[id.index()] = Some(output);
    let map = |child, target: &mut WorkspaceTypeGraph, mapped: &mut [Option<WorkspaceTypeId>]| {
        merge_type_node(child, source, target, mapped)
    };
    let node = match source.node(id) {
        TypeNode::Pending => WorkspaceTypeNode::Pending,
        TypeNode::Ref(child) => WorkspaceTypeNode::Ref(map(*child, target, mapped)),
        TypeNode::Bound(parameter) => WorkspaceTypeNode::Bound(parameter.index()),
        TypeNode::Named(name) => WorkspaceTypeNode::Opaque(format!("type-ref:{name}")),
        TypeNode::Declared { name, body, .. } => WorkspaceTypeNode::Declared {
            name: name.clone(),
            body: map(*body, target, mapped),
        },
        TypeNode::Any => WorkspaceTypeNode::Any,
        TypeNode::Never => WorkspaceTypeNode::Never,
        TypeNode::Type => WorkspaceTypeNode::Type,
        TypeNode::Dyn => WorkspaceTypeNode::Dyn,
        TypeNode::TypeOf(instance) => WorkspaceTypeNode::TypeOf(map(*instance, target, mapped)),
        TypeNode::Int => WorkspaceTypeNode::Int,
        TypeNode::Float => WorkspaceTypeNode::Float,
        TypeNode::String => WorkspaceTypeNode::String,
        TypeNode::Bytes => WorkspaceTypeNode::Bytes,
        TypeNode::AtomValue => WorkspaceTypeNode::AtomValue,
        TypeNode::Opaque(native_type) => {
            WorkspaceTypeNode::Opaque(native_type.qualified_name().into())
        }
        TypeNode::Atom(atom) => WorkspaceTypeNode::Atom(atom.name().into()),
        TypeNode::Array(child) => WorkspaceTypeNode::Array(map(*child, target, mapped)),
        TypeNode::Dict(child) => WorkspaceTypeNode::Dict(map(*child, target, mapped)),
        TypeNode::Tagged { tag, payload } => WorkspaceTypeNode::Tagged {
            tag: tag.name().into(),
            payload: map(*payload, target, mapped),
        },
        TypeNode::Tuple(children) => WorkspaceTypeNode::Tuple(
            children
                .iter()
                .map(|child| map(*child, target, mapped))
                .collect(),
        ),
        TypeNode::Struct(fields) => WorkspaceTypeNode::Struct(
            fields
                .iter()
                .map(|(name, child)| (name.clone(), map(*child, target, mapped)))
                .collect(),
        ),
        TypeNode::Enum(variants) => WorkspaceTypeNode::Enum(
            variants
                .iter()
                .map(|(name, child)| (name.clone(), child.map(|child| map(child, target, mapped))))
                .collect(),
        ),
        TypeNode::Union(children) => WorkspaceTypeNode::Union(
            children
                .iter()
                .map(|child| map(*child, target, mapped))
                .collect(),
        ),
        TypeNode::Function { parameters, result } => WorkspaceTypeNode::Function {
            parameters: parameters
                .iter()
                .map(|child| map(*child, target, mapped))
                .collect(),
            result: map(*result, target, mapped),
        },
    };
    target.nodes[output.index()] = node;
    output
}

#[derive(Clone, Debug)]
pub(crate) struct SemanticImport {
    pub name: String,
    pub location: Location,
    pub target: ModuleCName,
    pub namespace: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct SemanticModuleInput {
    pub key: String,
    pub path: Option<PathBuf>,
    pub kind: WorkspaceModuleKind,
    pub source: Option<SourceId>,
    pub program: Option<Program>,
    pub analysis: Option<Analysis>,
    pub partial: Option<PartialAnalysis>,
    pub interface: Option<SemanticModuleInterface>,
    pub state: WorkspaceModuleState,
    pub imports: Vec<SemanticImport>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug)]
pub(crate) struct SemanticModuleInterface {
    types: TypeGraph,
    result_type: AnalysisTypeId,
    export_schemes: BTreeMap<String, String>,
}

impl SemanticModuleInterface {
    pub(crate) fn new(interface: &ModuleInterface) -> Self {
        let (types, result_type) = TypeGraph::from_module_interface(interface);
        let export_schemes = interface
            .exports
            .iter()
            .map(|(name, scheme)| (name.clone(), scheme.display_name()))
            .collect();
        Self {
            types,
            result_type,
            export_schemes,
        }
    }
}

#[cfg(test)]
#[path = "semantic/tests/mod.rs"]
mod tests;
