use crate::ast::{
    BinaryOperator, BindingKind, Block, BlockKind, DictField, Expr, ExprKind, Identifier, MatchArm,
    Pattern, PatternKind, StringPartKind, UnaryOperator, located,
};
use crate::bytecode::{BytecodeFunction, Constant};
use crate::lexer::{FrontendError, SourceLocation};
use crate::lir::{self, ConstantId, Item, LabelId, Operation, RegisterId};
use crate::source::{Diagnostic, Location, Origin, SourceFile, WithOrigin};
use crate::types::NominalTypeConstructor;
use crate::value::{Atom, BuiltinAtom, NativeFunction};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

struct NestedEnvironment<'a, 'facts> {
    captures: &'a [String],
    type_slots: &'a HashSet<String>,
    definitions: &'a HashSet<String>,
    declared_value_owners: &'facts HashMap<Location, crate::types::ResolvedEvidence>,
    owner_index: &'facts OwnerEvidenceIndex<'facts>,
    value_constructors: &'facts HashMap<Location, crate::types::ValueConstructor>,
}

struct OwnerEvidenceIndex<'a> {
    entries: Vec<(&'a Location, &'a crate::types::ResolvedEvidence)>,
}

impl<'a> OwnerEvidenceIndex<'a> {
    fn new(owners: &'a HashMap<Location, crate::types::ResolvedEvidence>) -> Self {
        let mut entries = owners.iter().collect::<Vec<_>>();
        entries.sort_unstable_by_key(|(location, _)| **location);
        Self { entries }
    }

    fn within(&self, scope: Location) -> impl Iterator<Item = &'a crate::types::ResolvedEvidence> + '_ {
        let start = self.entries.partition_point(|(location, _)|
            (location.source, location.start) < (scope.source, scope.start));
        self.entries[start..].iter()
            .take_while(move |(location, _)| location.source == scope.source && location.start <= scope.end)
            .filter(move |(location, _)| location.end <= scope.end)
            .map(|(_, owner)| *owner)
    }
}


pub(crate) fn type_link_key(name: &str) -> String {
    format!("type:{name}")
}

pub(crate) fn type_family_link_key(name: &str) -> String {
    format!("type-family:{name}")
}

pub(crate) fn type_family_template_link_key(name: &str) -> String {
    format!("type-family-template:{name}")
}

pub(crate) fn declared_owner_link_key(location: Location) -> String {
    format!("\0declared-owner:{}:{}", location.start, location.end)
}


pub(crate) struct PreparedExternalExpression {
    expression: Expr,
    declared_value_owners: HashMap<Location, crate::types::ResolvedEvidence>,
    value_constructors: HashMap<Location, crate::types::ValueConstructor>,
}

pub(crate) fn prepare_expression_with_external_bindings(
    mut expression: Expr,
    binding_exists: impl Fn(&str) -> bool,
    declared_value_owners: HashMap<Location, crate::types::ResolvedEvidence>,
    value_constructors: HashMap<Location, crate::types::ValueConstructor>,
    source_file: &SourceFile,
) -> Result<(PreparedExternalExpression, Vec<String>), FrontendError> {
    crate::elaboration::lower_constructor_patterns(&mut expression, &value_constructors);
    let mut required = BTreeSet::new();
    free_expr(&expression, &HashSet::new(), &mut required);
    for evidence in declared_value_owners.values() { evidence.collect_bindings(&mut required); }
    let bindings = required.into_iter().collect::<Vec<_>>();
    if let Some(name) = bindings.iter().find(|name| !binding_exists(name)) {
        let diagnostic = Diagnostic::error(format!("prepared expression is missing runtime binding {name:?}"), expression.location);
        let position = source_file.position(expression.location.start);
        return Err(FrontendError {
            source_name: source_file.name.to_string(),
            location: SourceLocation { offset: expression.location.start as usize, line: position.line, column: position.column },
            message: diagnostic.message.clone(), diagnostic: Some(Box::new(diagnostic)),
        });
    }
    Ok((PreparedExternalExpression { expression, declared_value_owners, value_constructors }, bindings))
}

pub(crate) fn compile_prepared_external_expression(
    source_name: &str,
    function_name: &str,
    prepared: &PreparedExternalExpression,
    bindings: &[String],
    source_file: &SourceFile,
) -> Result<BytecodeFunction, FrontendError> {
    let PreparedExternalExpression { expression, declared_value_owners, value_constructors } = prepared;
    let owner_index = OwnerEvidenceIndex::new(&declared_value_owners);
    let mut compiler = Compiler {
        source_name,
        function_name: function_name.to_owned(),
        environment: HashMap::new(),
        type_slot_bindings: HashSet::new(),
        ready_type_slot_bindings: HashSet::new(),
        preserved_type_slot_reads: HashSet::new(),
        definition_bindings: HashSet::new(),
        constants: Vec::new(),
        external_constant_links: Vec::new(),
        items: Vec::new(),
        next_register: 0,
        next_label: 0,
        parameter_count: 0,
        capture_count: 0,
        closure_index: 0,
        retained_names: HashSet::new(),
        promoted_types: HashSet::new(),
        external_bindings: HashSet::new(),
        type_family_values: BTreeMap::new(),
        declared_value_owners: &declared_value_owners,
        owner_index: &owner_index,
        value_constructors: &value_constructors,
        source_file: Some(source_file),
    };
    for name in bindings {
        let register = compiler.load_external_constant(name.clone(), expression.location);
        compiler.environment.insert(name.clone(), register);
    }
    compiler.compile_tail_expr(expression)?;
    compiler.finish()
}
