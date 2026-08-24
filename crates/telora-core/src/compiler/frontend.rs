use crate::ast::{
    BinaryOperator, BindingKind, Block, BlockKind, DictField, Expr, ExprKind, Identifier, MatchArm,
    Pattern, PatternKind, Program, StringPartKind, UnaryOperator, located,
};
use crate::bytecode::{BytecodeFunction, Constant};
#[cfg(test)]
use crate::heap::Val;
use crate::heap::{Heap, PersistentValue};
use crate::hir::HirProgram;
use crate::lexer::{FrontendError, SourceLocation};
use crate::lir::{self, ConstantId, Item, LabelId, Operation, RegisterId};
use crate::parser::parse_registered;
use crate::source::{Diagnostic, Location, Origin, SourceDatabase, SourceFile, WithOrigin};
use crate::types::{Analysis, NominalTypeConstructor, analyze_program_with_bindings_observed};
use crate::value::{Atom, BuiltinAtom, NativeFunction};
use crate::{DiscardDebugSink, ExecutionWorld, Quota, QuotaAccount, RuntimeError, Vm};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

struct NestedEnvironment<'a> {
    captures: &'a [String],
    type_slots: &'a HashSet<String>,
    definitions: &'a HashSet<String>,
    declared_value_owners: &'a HashMap<Location, String>,
}

#[derive(Debug)]
pub enum ExecutionError {
    Frontend(FrontendError),
    Runtime(RuntimeError),
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Frontend(error) => error.fmt(formatter),
            Self::Runtime(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ExecutionError {}

impl From<FrontendError> for ExecutionError {
    fn from(value: FrontendError) -> Self {
        Self::Frontend(value)
    }
}

impl From<RuntimeError> for ExecutionError {
    fn from(value: RuntimeError) -> Self {
        Self::Runtime(value)
    }
}

pub struct CompiledSource {
    function: BytecodeFunction,
    main: Arc<Heap>,
    externals: HashMap<String, crate::heap::Val>,
}

impl fmt::Debug for CompiledSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompiledSource")
            .field("function", &self.function)
            .finish_non_exhaustive()
    }
}

impl CompiledSource {
    pub fn function(&self) -> &BytecodeFunction {
        &self.function
    }

    pub fn into_function(self) -> BytecodeFunction {
        self.function
    }

    pub(crate) fn execute_with_account(
        &self,
        vm: &mut Vm,
        account: &mut QuotaAccount,
    ) -> Result<ExecutionWorld, RuntimeError> {
        let work = vm.execute_in_work(&self.main, &self.externals, &self.function, &[], account)?;
        Ok(ExecutionWorld::new(Arc::clone(&self.main), work))
    }

    #[cfg(test)]
    pub(crate) fn execute_with_quota(
        &self,
        vm: &mut Vm,
        quota: Quota,
    ) -> Result<ExecutionWorld, RuntimeError> {
        self.execute_with_account(vm, &mut QuotaAccount::new(quota))
    }
}

impl std::ops::Deref for CompiledSource {
    type Target = BytecodeFunction;

    fn deref(&self) -> &Self::Target {
        &self.function
    }
}

pub fn compile_source(source_name: &str, source: &str) -> Result<CompiledSource, FrontendError> {
    let mut sources = SourceDatabase::default();
    let source_id = sources.add(source_name, source);
    let parsed = parse_registered(&sources, source_id);
    let program = parsed.program.ok_or_else(|| {
        FrontendError::from_diagnostic(
            &sources,
            parsed
                .diagnostics
                .into_iter()
                .next()
                .expect("failed parse has a diagnostic"),
        )
    })?;
    let mut main = Heap::main();
    let mut type_store = crate::type_store::TypeStore::default();
    let mut account = QuotaAccount::new(Quota::with_fuel(100_000));
    let debug_sink: std::sync::Arc<dyn crate::DebugSink> = std::sync::Arc::new(DiscardDebugSink);
    let analysis = analyze_program_with_bindings_observed(
        source_name,
        crate::ModuleId::ANONYMOUS,
        &program,
        &mut account,
        &BTreeMap::new(),
        &HashSet::new(),
        &sources,
        &BTreeMap::new(),
        &BTreeMap::new(),
        &debug_sink,
        &mut main,
        &mut type_store,
    )?;
    let promoted_types = program
        .value
        .body
        .value
        .bindings
        .iter()
        .filter(|binding| {
            matches!(binding.value.kind, BindingKind::Type | BindingKind::Trait)
                && binding.value.type_parameters.is_empty()
        })
        .map(|binding| binding.value.name.value.clone())
        .collect::<HashSet<_>>();
    let erased_bindings = metadata_compilation_plan(&program)
        .map(|metadata| metadata.erased_bindings)
        .unwrap_or_default();
    let function = compile_program_with_promoted_types(
        sources.get(source_id),
        &program,
        &analysis,
        &promoted_types,
        &erased_bindings,
    )?;
    let mut roots = analysis.runtime_roots.clone();
    roots.extend(
        analysis
            .type_family_values
            .iter()
            .flat_map(|(name, family)| {
                [
                    (type_family_link_key(name), family.root()),
                    (type_family_template_link_key(name), family.template()),
                ]
            }),
    );
    let externals = roots
        .into_iter()
        .map(|(name, root): (String, PersistentValue)| (name, root.runtime()))
        .collect();
    Ok(CompiledSource {
        function,
        main: Arc::new(main),
        externals,
    })
}

pub(crate) fn compile_program_analyzed_in_module(
    source_file: &SourceFile,
    program: &Program,
    analysis: &Analysis,
    static_funcs: &HashMap<String, crate::FuncId>,
) -> Result<BytecodeFunction, FrontendError> {
    compile_program_with_promoted_types_and_static_funcs(
        source_file,
        program,
        analysis,
        &HashSet::new(),
        &HashSet::new(),
        static_funcs,
    )
}

pub(crate) fn compile_program_with_promoted_types(
    source_file: &SourceFile,
    program: &Program,
    analysis: &Analysis,
    promoted_types: &HashSet<String>,
    erased_bindings: &HashSet<String>,
) -> Result<BytecodeFunction, FrontendError> {
    compile_program_with_promoted_types_and_static_funcs(
        source_file,
        program,
        analysis,
        promoted_types,
        erased_bindings,
        &HashMap::new(),
    )
}

pub(crate) fn compile_program_with_promoted_types_and_static_funcs(
    source_file: &SourceFile,
    program: &Program,
    analysis: &Analysis,
    promoted_types: &HashSet<String>,
    erased_bindings: &HashSet<String>,
    static_funcs: &HashMap<String, crate::FuncId>,
) -> Result<BytecodeFunction, FrontendError> {
    validate_hir(source_file, &analysis.hir)?;
    let mut program = program.clone();
    program.value.body.value.bindings.retain(|binding| {
        matches!(binding.value.kind, BindingKind::Type | BindingKind::Trait)
            || !erased_bindings.contains(&binding.value.name.value)
    });
    crate::elaboration::elaborate_program(
        &mut program,
        &analysis.propagation_families,
        &analysis.not_families,
        &analysis.trait_member_evidence,
        &analysis.generic_call_evidence,
        &analysis.generic_evidence_parameters,
        &analysis.generic_dictionary_factories,
    );
    Compiler::program_in(
        source_file.name.as_ref(),
        Some(source_file),
        &program,
        analysis,
        promoted_types.clone(),
        static_funcs.clone(),
    )
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

pub(crate) struct MetadataCompilationPlan {
    pub(crate) type_names: Vec<String>,
    pub(crate) erased_bindings: HashSet<String>,
}

pub(crate) fn metadata_compilation_plan(program: &Program) -> Option<MetadataCompilationPlan> {
    let mut type_names = program
        .value
        .body
        .value
        .bindings
        .iter()
        .filter(|binding| {
            matches!(binding.value.kind, BindingKind::Type | BindingKind::Trait)
                && binding.value.type_parameters.is_empty()
        })
        .map(|binding| binding.value.name.value.clone())
        .collect::<Vec<_>>();
    if type_names.is_empty() {
        return None;
    }
    type_names.sort();
    type_names.dedup();

    let mut needed = type_names.iter().cloned().collect::<HashSet<_>>();
    loop {
        let before = needed.len();
        for binding in &program.value.body.value.bindings {
            if needed.contains(&binding.value.name.value) {
                for decorator in &binding.value.decorators {
                    collect_decorator_runtime_names(decorator, &mut needed);
                }
                collect_runtime_names(&binding.value.value, &mut needed);
                if let Some(annotation) = &binding.value.annotation {
                    collect_runtime_names(annotation, &mut needed);
                }
            }
        }
        if needed.len() == before {
            break;
        }
    }
    let mut runtime_needed = HashSet::new();
    collect_runtime_names(&program.value.body.value.result, &mut runtime_needed);
    for binding in &program.value.body.value.bindings {
        if !needed.contains(&binding.value.name.value)
            && !matches!(
                binding.value.kind,
                BindingKind::Type | BindingKind::Trait | BindingKind::Decl
            )
        {
            collect_runtime_names(&binding.value.value, &mut runtime_needed);
        }
    }
    loop {
        let before = runtime_needed.len();
        for binding in &program.value.body.value.bindings {
            if needed.contains(&binding.value.name.value)
                && runtime_needed.contains(&binding.value.name.value)
            {
                collect_runtime_names(&binding.value.value, &mut runtime_needed);
            }
        }
        if runtime_needed.len() == before {
            break;
        }
    }
    let type_name_set = type_names.iter().cloned().collect::<HashSet<_>>();
    let erased_bindings = needed
        .iter()
        .filter(|name| !type_name_set.contains(*name) && !runtime_needed.contains(*name))
        .cloned()
        .collect();
    Some(MetadataCompilationPlan {
        type_names,
        erased_bindings,
    })
}

pub fn run_source(
    source_name: &str,
    source: &str,
    evaluation_fuel: usize,
) -> Result<ExecutionWorld, ExecutionError> {
    let compiled = compile_source(source_name, source)?;
    let mut sources = SourceDatabase::default();
    sources.add(source_name, source);
    let mut account = QuotaAccount::new(Quota::with_fuel(evaluation_fuel));
    compiled
        .execute_with_account(&mut Vm::new(), &mut account)
        .map_err(|error| ExecutionError::Runtime(error.with_sources(&sources)))
}

pub(crate) fn compile_expression_with_external_bindings(
    source_name: &str,
    function_name: &str,
    expression: &Expr,
    bindings: impl IntoIterator<Item = String>,
    declared_value_owners: HashMap<Location, String>,
    source_file: &SourceFile,
) -> Result<BytecodeFunction, FrontendError> {
    let bindings = bindings.into_iter().collect::<Vec<_>>();
    let hir = HirProgram::resolve_runtime_expression(expression, bindings.iter().cloned());
    validate_hir(source_file, &hir)?;
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
        declared_value_owners,
        static_funcs: HashMap::new(),
        source_file: Some(source_file),
    };
    for name in bindings {
        let register = compiler.load_external_constant(name.clone(), expression.location);
        compiler.environment.insert(name, register);
    }
    compiler.compile_tail_expr(expression)?;
    compiler.finish()
}

fn validate_hir(source_file: &SourceFile, hir: &HirProgram) -> Result<(), FrontendError> {
    let Some(reference) = hir.unresolved().next() else {
        return Ok(());
    };
    let position = source_file.position(reference.location.start);
    let message = format!("unknown binding {:?}", reference.name);
    Err(FrontendError {
        source_name: source_file.name.to_string(),
        location: SourceLocation {
            offset: reference.location.start as usize,
            line: position.line,
            column: position.column,
        },
        message: message.clone(),
        diagnostic: Some(Box::new(Diagnostic::error(message, reference.location))),
    })
}
