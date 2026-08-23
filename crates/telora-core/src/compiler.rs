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
            binding.value.kind == BindingKind::Type && binding.value.type_parameters.is_empty()
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
        binding.value.kind == BindingKind::Type
            || !erased_bindings.contains(&binding.value.name.value)
    });
    crate::elaboration::elaborate_program(
        &mut program,
        &analysis.propagation_families,
        &analysis.not_families,
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
            binding.value.kind == BindingKind::Type && binding.value.type_parameters.is_empty()
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
            && !matches!(binding.value.kind, BindingKind::Type | BindingKind::Decl)
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

struct Compiler<'a> {
    source_name: &'a str,
    function_name: String,
    environment: HashMap<String, RegisterId>,
    type_slot_bindings: HashSet<String>,
    ready_type_slot_bindings: HashSet<String>,
    preserved_type_slot_reads: HashSet<String>,
    definition_bindings: HashSet<String>,
    constants: Vec<Constant>,
    external_constant_links: Vec<(usize, String)>,
    items: Vec<Item>,
    next_register: u32,
    next_label: u32,
    parameter_count: u32,
    capture_count: u32,
    closure_index: usize,
    retained_names: HashSet<String>,
    promoted_types: HashSet<String>,
    external_bindings: HashSet<String>,
    type_family_values: BTreeMap<String, crate::types::TypeFamilyTemplate>,
    declared_value_owners: HashMap<Location, String>,
    static_funcs: HashMap<String, crate::FuncId>,
    source_file: Option<&'a SourceFile>,
}

impl<'a> Compiler<'a> {
    fn error_at(&self, location: Location, message: impl Into<String>) -> FrontendError {
        let message = message.into();
        if let Some(source_file) = self.source_file {
            let position = source_file.position(location.start);
            let diagnostic = Diagnostic::error(message.clone(), location);
            FrontendError {
                source_name: source_file.name.to_string(),
                location: SourceLocation {
                    offset: location.start as usize,
                    line: position.line,
                    column: position.column,
                },
                message,
                diagnostic: Some(Box::new(diagnostic)),
            }
        } else {
            unreachable!("located compiler errors require their source file")
        }
    }

    fn program_in(
        source_name: &'a str,
        source_file: Option<&'a SourceFile>,
        program: &Program,
        analysis: &Analysis,
        promoted_types: HashSet<String>,
        static_funcs: HashMap<String, crate::FuncId>,
    ) -> Result<BytecodeFunction, FrontendError> {
        let mut retained_names = HashSet::new();
        collect_runtime_names_block(&program.value.body, &mut retained_names);
        loop {
            let before = retained_names.len();
            for binding in &program.value.body.value.bindings {
                if binding.value.kind == BindingKind::Type
                    && retained_names.contains(&binding.value.name.value)
                {
                    collect_runtime_names(&binding.value.value, &mut retained_names);
                }
            }
            if retained_names.len() == before {
                break;
            }
        }
        let mut compiler = Self {
            source_name,
            function_name: source_name.to_owned(),
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
            retained_names,
            promoted_types,
            external_bindings: analysis.external_bindings.clone(),
            type_family_values: analysis.type_family_values.clone(),
            declared_value_owners: analysis.declared_value_owners.clone(),
            static_funcs,
            source_file,
        };
        let authored_names = program
            .value
            .body
            .value
            .bindings
            .iter()
            .filter(|binding| {
                !matches!(
                    binding.value.kind,
                    BindingKind::OpenImport | BindingKind::Export
                )
            })
            .map(|binding| binding.value.name.value.as_str())
            .collect::<HashSet<_>>();
        for name in analysis.runtime_roots.keys() {
            if !authored_names.contains(name.as_str()) && compiler.retained_names.contains(name) {
                let register = compiler.load_external_constant(name.clone(), program.location);
                compiler.environment.insert(name.clone(), register);
            }
        }
        for name in &analysis.dynamic_bindings {
            if !authored_names.contains(name.as_str()) && compiler.retained_names.contains(name) {
                let register = compiler.load_external_constant(name.clone(), program.location);
                compiler.environment.insert(name.clone(), register);
            }
        }
        for name in &analysis.external_bindings {
            if !authored_names.contains(name.as_str())
                && compiler.retained_names.contains(name)
                && !compiler.environment.contains_key(name)
            {
                let register = compiler.load_external_constant(name.clone(), program.location);
                compiler.environment.insert(name.clone(), register);
            }
        }
        compiler.compile_tail_block(&program.value.body)?;
        compiler.finish()
    }

    fn nested(
        source_name: &'a str,
        source_file: Option<&'a SourceFile>,
        function_name: String,
        parameters: &[Identifier],
        nested_environment: NestedEnvironment<'_>,
    ) -> Result<Self, FrontendError> {
        let NestedEnvironment {
            captures,
            type_slots: captured_type_slots,
            definitions: captured_definitions,
            declared_value_owners,
        } = nested_environment;
        let mut environment = HashMap::new();
        for (index, parameter) in parameters.iter().enumerate() {
            if environment
                .insert(
                    parameter.value.clone(),
                    RegisterId(u32::try_from(index).map_err(|_| {
                        frontend_error(source_name, "too many function parameters")
                    })?),
                )
                .is_some()
            {
                return Err(frontend_error(
                    source_name,
                    format!("duplicate parameter {:?}", parameter.value),
                ));
            }
        }
        for (offset, capture) in captures.iter().enumerate() {
            let index = parameters
                .len()
                .checked_add(offset)
                .ok_or_else(|| frontend_error(source_name, "too many closure registers"))?;
            environment.insert(
                capture.clone(),
                RegisterId(
                    u32::try_from(index)
                        .map_err(|_| frontend_error(source_name, "too many closure captures"))?,
                ),
            );
        }
        let register_count = parameters
            .len()
            .checked_add(captures.len())
            .ok_or_else(|| frontend_error(source_name, "too many closure registers"))?;
        Ok(Self {
            source_name,
            function_name,
            environment,
            type_slot_bindings: captured_type_slots.clone(),
            ready_type_slot_bindings: HashSet::new(),
            preserved_type_slot_reads: HashSet::new(),
            definition_bindings: captured_definitions.clone(),
            constants: Vec::new(),
            external_constant_links: Vec::new(),
            items: Vec::new(),
            next_register: u32::try_from(register_count)
                .map_err(|_| frontend_error(source_name, "too many closure registers"))?,
            next_label: 0,
            parameter_count: u32::try_from(parameters.len())
                .map_err(|_| frontend_error(source_name, "too many function parameters"))?,
            capture_count: u32::try_from(captures.len())
                .map_err(|_| frontend_error(source_name, "too many closure captures"))?,
            closure_index: 0,
            retained_names: HashSet::new(),
            promoted_types: HashSet::new(),
            external_bindings: HashSet::new(),
            type_family_values: BTreeMap::new(),
            declared_value_owners: declared_value_owners.clone(),
            static_funcs: HashMap::new(),
            source_file,
        })
    }

    fn finish_lir(self) -> lir::Function {
        lir::Function {
            name: self.function_name,
            parameter_count: self.parameter_count,
            capture_count: self.capture_count,
            register_count: self.next_register,
            constants: self.constants,
            items: self.items,
        }
    }

    fn finish(self) -> Result<BytecodeFunction, FrontendError> {
        let source_name = self.source_name;
        let external_links = self.external_constant_links.clone();
        let mut function = lir::assemble(self.finish_lir())
            .map_err(|error| frontend_error(source_name, error.to_string()))?;
        for (index, key) in external_links {
            function.bind_external_value(index, key);
        }
        Ok(function)
    }

    fn compile_block(&mut self, block: &Block) -> Result<RegisterId, FrontendError> {
        self.compile_block_inner(block, false)?
            .ok_or_else(|| frontend_error(self.source_name, "value block did not produce a value"))
    }

    fn compile_tail_block(&mut self, block: &Block) -> Result<(), FrontendError> {
        self.compile_block_inner(block, true)?;
        Ok(())
    }

    fn compile_block_inner(
        &mut self,
        block: &Block,
        tail: bool,
    ) -> Result<Option<RegisterId>, FrontendError> {
        let outer = self.environment.clone();
        let outer_type_slots = self.type_slot_bindings.clone();
        let outer_ready_type_slots = self.ready_type_slot_bindings.clone();
        let outer_definitions = self.definition_bindings.clone();
        let mut declared = HashMap::<String, (RegisterId, Location)>::new();
        let mut type_links = HashMap::<String, (RegisterId, Location)>::new();
        let mut native_declarations = HashMap::<String, Location>::new();
        let mut definition_counts = HashMap::<String, usize>::new();

        #[derive(Clone, Copy)]
        enum VisibleFunctionBinding {
            Decl,
            Def,
            Other,
        }

        // Function slots are allocated before the block is emitted so recursive
        // definitions can refer to one another. Validate the source-order binding
        // semantics separately: only `let` may shadow, and a later `def` may not
        // reach through that shadow to initialize an earlier declaration.
        let mut visible_function_bindings = HashMap::<String, VisibleFunctionBinding>::new();
        for binding in &block.value.bindings {
            let name = &binding.value.name.value;
            match binding.value.kind {
                BindingKind::Decl => {
                    if outer.contains_key(name) || visible_function_bindings.contains_key(name) {
                        return Err(self.error_at(
                            binding.location,
                            format!("declaration {name:?} cannot shadow a visible binding"),
                        ));
                    }
                    visible_function_bindings.insert(name.clone(), VisibleFunctionBinding::Decl);
                }
                BindingKind::Def => match visible_function_bindings.get(name) {
                    Some(VisibleFunctionBinding::Decl) => {
                        visible_function_bindings.insert(name.clone(), VisibleFunctionBinding::Def);
                    }
                    None if !outer.contains_key(name) => {
                        visible_function_bindings.insert(name.clone(), VisibleFunctionBinding::Def);
                    }
                    Some(VisibleFunctionBinding::Def | VisibleFunctionBinding::Other) | None => {
                        return Err(self.error_at(
                            binding.location,
                            format!("definition {name:?} cannot shadow a visible binding"),
                        ));
                    }
                },
                BindingKind::Let => {
                    visible_function_bindings.insert(name.clone(), VisibleFunctionBinding::Other);
                }
                BindingKind::Import
                | BindingKind::Native
                | BindingKind::NativeType
                | BindingKind::Type => {
                    visible_function_bindings
                        .entry(name.clone())
                        .or_insert(VisibleFunctionBinding::Other);
                }
                BindingKind::OpenImport | BindingKind::Export => {}
            }
        }

        for binding in &block.value.bindings {
            if binding.value.kind != BindingKind::Native {
                continue;
            }
            let name = &binding.value.name.value;
            if native_declarations
                .insert(name.clone(), binding.location)
                .is_some()
            {
                return Err(self.error_at(
                    binding.location,
                    format!("duplicate native declaration {name:?}"),
                ));
            }
            if outer.contains_key(name) {
                return Err(self.error_at(
                    binding.location,
                    format!("native binding {name:?} cannot shadow an outer binding"),
                ));
            }
        }
        for binding in &block.value.bindings {
            if matches!(
                binding.value.kind,
                BindingKind::OpenImport | BindingKind::Export
            ) {
                continue;
            }
            let name = &binding.value.name.value;
            if native_declarations.contains_key(name) && binding.value.kind != BindingKind::Native {
                return Err(self.error_at(
                    binding.location,
                    format!("binding {name:?} conflicts with a native declaration"),
                ));
            }
        }

        for binding in &block.value.bindings {
            let name = &binding.value.name.value;
            if binding.value.kind == BindingKind::Def {
                *definition_counts.entry(name.clone()).or_default() += 1;
            }
            let function_arity = binding
                .value
                .annotation
                .as_ref()
                .and_then(function_contract_arity)
                .or_else(|| match &binding.value.value.value {
                    ExprKind::Closure { parameters, .. } => u32::try_from(parameters.len()).ok(),
                    _ => None,
                });
            if binding.value.kind == BindingKind::Decl && function_arity.is_none() {
                return Err(self.error_at(binding.location, "decl requires a function contract"));
            }
            if binding.value.kind == BindingKind::Decl && declared.contains_key(name) {
                return Err(
                    self.error_at(binding.location, format!("duplicate declaration {name:?}"))
                );
            }
            if matches!(binding.value.kind, BindingKind::Decl | BindingKind::Def)
                && function_arity.is_some()
                && !declared.contains_key(name)
            {
                if declared.contains_key(name) {
                    return Err(
                        self.error_at(binding.location, format!("duplicate declaration {name:?}"))
                    );
                }
                if outer.contains_key(name) {
                    return Err(self.error_at(
                        binding.location,
                        format!("definition {name:?} cannot shadow an outer definition"),
                    ));
                }
                let link = self.allocate();
                self.emit(
                    Operation::AllocFunc {
                        dst: link,
                        static_id: self.static_funcs.get(name).copied(),
                    },
                    binding.location,
                );
                self.environment.insert(name.clone(), link);
                self.definition_bindings.insert(name.clone());
                declared.insert(name.clone(), (link, binding.location));
            }
        }
        for binding in &block.value.bindings {
            if binding.value.kind != BindingKind::Type
                || !self.retained_names.contains(&binding.value.name.value)
                || self.promoted_types.contains(&binding.value.name.value)
            {
                continue;
            }
            let name = &binding.value.name.value;
            if self.environment.contains_key(name) {
                return Err(self.error_at(
                    binding.location,
                    format!("type definition {name:?} cannot shadow a visible binding"),
                ));
            }
            let link = self.allocate();
            self.emit(Operation::AllocTypeSlot { dst: link }, binding.location);
            self.environment.insert(name.clone(), link);
            self.type_slot_bindings.insert(name.clone());
            type_links.insert(name.clone(), (link, binding.location));
        }
        for (name, count) in &definition_counts {
            if *count > 1 {
                return Err(self.error_at(
                    block.location,
                    format!("definition {name:?} is initialized more than once"),
                ));
            }
        }
        for binding in &block.value.bindings {
            if matches!(
                binding.value.kind,
                BindingKind::OpenImport | BindingKind::Export
            ) {
                continue;
            }
            let name = &binding.value.name.value;
            if declared.contains_key(name)
                && !matches!(
                    binding.value.kind,
                    BindingKind::Decl | BindingKind::Def | BindingKind::Let
                )
            {
                return Err(self.error_at(
                    binding.location,
                    format!("binding {name:?} conflicts with a declaration in this block"),
                ));
            }
        }

        for binding in &block.value.bindings {
            match binding.value.kind {
                BindingKind::OpenImport | BindingKind::Export => continue,
                BindingKind::Decl => continue,
                BindingKind::Type => {
                    if self.retained_names.contains(&binding.value.name.value) {
                        let name = binding.value.name.value.clone();
                        if self.promoted_types.contains(&name) {
                            let register =
                                self.load_external_constant(type_link_key(&name), binding.location);
                            self.environment.insert(name, register);
                        } else if !binding.value.type_parameters.is_empty() {
                            let family = self
                                .type_family_values
                                .get(&name)
                                .expect("analyzed type family has runtime roots");
                            let register = if family.rebuild_at_runtime() {
                                let body = located(
                                    BlockKind {
                                        bindings: Vec::new(),
                                        result: Box::new(binding.value.value.clone()),
                                    },
                                    binding.value.value.location,
                                );
                                self.compile_closure_with_declared_family(
                                    &binding.value.type_parameters,
                                    &body,
                                    binding.location,
                                    family.constructor().cloned(),
                                )?
                            } else {
                                self.load_external_constant(
                                    type_family_link_key(&name),
                                    binding.location,
                                )
                            };
                            let (link, _) = type_links[&binding.value.name.value];
                            self.emit(
                                Operation::SealTypeSlot {
                                    link,
                                    src: register,
                                },
                                binding.location,
                            );
                            self.ready_type_slot_bindings.insert(name);
                        } else {
                            self.preserved_type_slot_reads = type_links
                                .keys()
                                .filter(|name| !self.type_family_values.contains_key(*name))
                                .cloned()
                                .collect();
                            let register = self.compile_expr(&binding.value.value)?;
                            self.preserved_type_slot_reads.clear();
                            let (link, _) = type_links[&binding.value.name.value];
                            self.emit(
                                Operation::SealTypeSlot {
                                    link,
                                    src: register,
                                },
                                binding.location,
                            );
                            self.ready_type_slot_bindings.insert(name);
                        }
                    }
                    continue;
                }
                BindingKind::Import | BindingKind::Native | BindingKind::NativeType => {
                    if !self.external_bindings.contains(&binding.value.name.value) {
                        return Err(frontend_error(
                            self.source_name,
                            format!(
                                "external binding {} has not been resolved",
                                binding.value.name.value
                            ),
                        ));
                    }
                    let register = self
                        .load_external_constant(binding.value.name.value.clone(), binding.location);
                    self.environment
                        .insert(binding.value.name.value.clone(), register);
                    continue;
                }
                BindingKind::Let | BindingKind::Def => {}
            }
            if binding.value.kind == BindingKind::Def
                && !declared.contains_key(&binding.value.name.value)
                && self.environment.contains_key(&binding.value.name.value)
            {
                return Err(self.error_at(
                    binding.location,
                    format!(
                        "definition {:?} cannot shadow a visible binding",
                        binding.value.name.value
                    ),
                ));
            }
            let value = self.compile_expr(&binding.value.value)?;
            let name = binding.value.name.value.clone();
            if binding.value.kind == BindingKind::Def
                && let Some((link, _)) = declared.get(&name)
            {
                self.emit(
                    Operation::SealFunc {
                        target: *link,
                        source: value,
                    },
                    binding.location,
                );
            } else {
                self.environment.insert(name.clone(), value);
                self.type_slot_bindings.remove(&name);
                if binding.value.kind == BindingKind::Def {
                    self.definition_bindings.insert(name);
                } else if binding.value.kind == BindingKind::Let {
                    self.definition_bindings.remove(&name);
                }
            }
        }
        for (link, location) in type_links.values() {
            self.emit(Operation::AssertTypeSlotReady { link: *link }, *location);
        }
        let result = if tail {
            self.compile_tail_expr(&block.value.result)?;
            None
        } else {
            Some(self.compile_expr(&block.value.result)?)
        };
        self.environment = outer;
        self.type_slot_bindings = outer_type_slots;
        self.ready_type_slot_bindings = outer_ready_type_slots;
        self.definition_bindings = outer_definitions;
        Ok(result)
    }

    fn compile_expr(&mut self, expression: &Expr) -> Result<RegisterId, FrontendError> {
        let payload = self.compile_expr_unowned(expression)?;
        let Some(owner) = self
            .declared_value_owners
            .get(&expression.location)
            .cloned()
        else {
            return Ok(payload);
        };
        let owner = self
            .environment
            .get(&owner)
            .copied()
            .unwrap_or_else(|| self.load_external_constant(owner, expression.location));
        let result = self.allocate();
        self.emit(
            Operation::OwnDeclared {
                dst: result,
                owner,
                value: payload,
            },
            expression.location,
        );
        Ok(result)
    }

    fn compile_expr_unowned(&mut self, expression: &Expr) -> Result<RegisterId, FrontendError> {
        match &expression.value {
            ExprKind::Int(value) => {
                Ok(self.load_constant(Constant::Int(*value), expression.location))
            }
            ExprKind::Float(value) => {
                Ok(self.load_constant(Constant::Float(*value), expression.location))
            }
            ExprKind::String(value) => {
                Ok(self.load_constant(Constant::String(value.clone().into()), expression.location))
            }
            ExprKind::InterpolatedString(parts) => {
                let mut registers = Vec::with_capacity(parts.len());
                for part in parts {
                    registers.push(match &part.value {
                        StringPartKind::Text(text) => {
                            self.load_constant(Constant::String(text.clone().into()), part.location)
                        }
                        StringPartKind::Expression(expression) => self.compile_expr(expression)?,
                    });
                }
                let dst = self.allocate();
                self.emit(
                    Operation::InterpolateString {
                        dst,
                        parts: registers,
                    },
                    expression.location,
                );
                Ok(dst)
            }
            ExprKind::Bytes(value) => {
                Ok(self.load_constant(Constant::Bytes(value.clone().into()), expression.location))
            }
            ExprKind::Atom(name) => {
                Ok(self.load_constant(atom_constant(name), expression.location))
            }
            ExprKind::Variable(name) => {
                let register = self.environment.get(&name.value).copied().ok_or_else(|| {
                    self.error_at(
                        expression.location,
                        format!("unknown binding {:?}", name.value),
                    )
                })?;
                if self.type_slot_bindings.contains(&name.value)
                    && !self.preserved_type_slot_reads.contains(&name.value)
                {
                    let dst = self.allocate();
                    self.emit(
                        Operation::ReadTypeSlot {
                            dst,
                            link: register,
                        },
                        expression.location,
                    );
                    Ok(dst)
                } else {
                    Ok(register)
                }
            }
            ExprKind::Array(items) => {
                if items
                    .iter()
                    .any(|item| matches!(item.value, ExprKind::Spread(_)))
                {
                    return self.compile_spread_array(items, expression.location);
                }
                let items = self.compile_many(items)?;
                let dst = self.allocate();
                self.emit(Operation::MakeArray { dst, items }, expression.location);
                Ok(dst)
            }
            ExprKind::Spread(_) => {
                Err(self.error_at(expression.location, "spread is only valid in a collection"))
            }
            ExprKind::Tuple(items) => {
                let items = self.compile_many(items)?;
                let dst = self.allocate();
                self.emit(Operation::MakeTuple { dst, items }, expression.location);
                Ok(dst)
            }
            ExprKind::Dict(fields) => self.compile_dict(fields, expression.location),
            ExprKind::Block(block) => self.compile_block(block),
            ExprKind::Unary { operator, operand } => {
                let src = self.compile_expr(operand)?;
                let dst = self.allocate();
                match operator.value {
                    UnaryOperator::Negate => {
                        self.emit(Operation::Negate { dst, src }, expression.location);
                    }
                    UnaryOperator::Not => {
                        self.emit(Operation::Not { dst, src }, expression.location);
                    }
                    UnaryOperator::LogicalNot => {
                        self.emit(Operation::LogicalNot { dst, src }, expression.location);
                    }
                    UnaryOperator::BitNot => {
                        self.emit(Operation::BitNot { dst, src }, expression.location);
                    }
                }
                Ok(dst)
            }
            ExprKind::Propagate { .. } => {
                Err(self.error_at(expression.location, "unelaborated propagation expression"))
            }
            ExprKind::Return { value } => {
                let value = self.compile_expr(value)?;
                self.emit(Operation::Return { src: value }, expression.location);
                Ok(value)
            }
            ExprKind::Panic { message } => {
                let message = self.compile_expr(message)?;
                self.emit(Operation::Panic { message }, expression.location);
                Ok(message)
            }
            ExprKind::Raise { error } => {
                let error = self.compile_expr(error)?;
                self.emit(Operation::Raise { error }, expression.location);
                Ok(error)
            }
            ExprKind::Debug {
                value,
                message,
                expression: source_expression,
            } => {
                let value = self.compile_expr(value)?;
                let source_file = self
                    .source_file
                    .expect("debug expressions require a registered source");
                let position = source_file.position(expression.location.start);
                self.emit(
                    Operation::Debug {
                        value,
                        module: self.source_name.to_owned(),
                        line: u32::try_from(position.line).unwrap_or(u32::MAX),
                        name: source_expression.clone(),
                        message: message.clone(),
                    },
                    expression.location,
                );
                Ok(value)
            }
            ExprKind::Binary {
                operator,
                left,
                right,
            } => {
                let left = self.compile_expr(left)?;
                let right = self.compile_expr(right)?;
                let dst = self.allocate();
                let operation = match operator.value {
                    BinaryOperator::Add => Operation::Add { dst, left, right },
                    BinaryOperator::Subtract => Operation::Subtract { dst, left, right },
                    BinaryOperator::Multiply => Operation::Multiply { dst, left, right },
                    BinaryOperator::Divide => Operation::Divide { dst, left, right },
                    BinaryOperator::Remainder => Operation::Remainder { dst, left, right },
                    BinaryOperator::LessThan => Operation::LessThan { dst, left, right },
                    BinaryOperator::LessThanOrEqual => {
                        Operation::LessThanOrEqual { dst, left, right }
                    }
                    BinaryOperator::GreaterThan => Operation::LessThan {
                        dst,
                        left: right,
                        right: left,
                    },
                    BinaryOperator::GreaterThanOrEqual => Operation::LessThanOrEqual {
                        dst,
                        left: right,
                        right: left,
                    },
                    BinaryOperator::Equal => Operation::Equal { dst, left, right },
                    BinaryOperator::NotEqual => Operation::NotEqual { dst, left, right },
                    BinaryOperator::BitAnd => Operation::BitAnd { dst, left, right },
                    BinaryOperator::BitOr => Operation::BitOr { dst, left, right },
                    BinaryOperator::BitXor => Operation::BitXor { dst, left, right },
                    BinaryOperator::And | BinaryOperator::Or => {
                        return Err(self.error_at(
                            expression.location,
                            "unelaborated short-circuit expression",
                        ));
                    }
                };
                self.emit(operation, expression.location);
                Ok(dst)
            }
            ExprKind::TypeAscription { value, .. } => self.compile_expr(value),
            ExprKind::CheckedCast { value, target } => {
                let hidden = located(
                    ExprKind::Variable(located("\0telora_cast".to_owned(), expression.location)),
                    expression.location,
                );
                let call = located(
                    ExprKind::Call {
                        callee: Box::new(hidden),
                        arguments: vec![(**target).clone(), (**value).clone()],
                    },
                    expression.location,
                );
                self.compile_expr_unowned(&call)
            }
            ExprKind::DynProject {
                namespace,
                target,
                value,
            } => {
                let callee = located(
                    ExprKind::Field {
                        receiver: namespace.clone(),
                        field: located("project_with".to_owned(), expression.location),
                    },
                    expression.location,
                );
                let call = located(
                    ExprKind::Call {
                        callee: Box::new(callee),
                        arguments: vec![(**target).clone(), (**value).clone()],
                    },
                    expression.location,
                );
                self.compile_expr_unowned(&call)
            }
            ExprKind::Field { receiver, field } => {
                let dict = self.compile_expr(receiver)?;
                let dst = self.allocate();
                self.emit(
                    Operation::GetField {
                        dst,
                        dict,
                        field: field.value.clone(),
                    },
                    expression.location,
                );
                Ok(dst)
            }
            ExprKind::Index { receiver, index } => {
                let array = self.compile_expr(receiver)?;
                let index = self.compile_expr(index)?;
                let dst = self.allocate();
                self.emit(
                    Operation::GetArray { dst, array, index },
                    expression.location,
                );
                Ok(dst)
            }
            ExprKind::TupleProjection { receiver, index } => {
                let tuple = self.compile_expr(receiver)?;
                let dst = self.allocate();
                self.emit(
                    Operation::ProjectTuple {
                        dst,
                        tuple,
                        index: index.value,
                    },
                    expression.location,
                );
                Ok(dst)
            }
            ExprKind::Call { callee, arguments } => {
                let (base, argument_count) =
                    self.compile_call_window(callee, arguments, expression.location)?;
                self.emit(
                    Operation::Call {
                        base,
                        argument_count,
                    },
                    expression.location,
                );
                Ok(base)
            }
            ExprKind::TypeApply { callee, .. } => self.compile_expr(callee),
            ExprKind::Interpreter { elaboration, .. } => self.compile_expr(elaboration),
            ExprKind::Closure {
                parameters, body, ..
            } => {
                let parameters = parameters
                    .iter()
                    .map(|parameter| parameter.name.clone())
                    .collect::<Vec<_>>();
                self.compile_closure(&parameters, body, expression.location)
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => self.compile_if(condition, then_branch, else_branch, expression.location),
            ExprKind::IfLet { .. } => {
                Err(self.error_at(expression.location, "unelaborated if let expression"))
            }
            ExprKind::LetElse { .. } => {
                Err(self.error_at(expression.location, "unelaborated let else expression"))
            }
            ExprKind::Match { value, arms } => self.compile_match(value, arms, expression.location),
        }
    }

    fn compile_tail_expr(&mut self, expression: &Expr) -> Result<(), FrontendError> {
        if self
            .declared_value_owners
            .contains_key(&expression.location)
        {
            let result = self.compile_expr(expression)?;
            self.emit_synthetic(Operation::Return { src: result }, expression.location);
            return Ok(());
        }
        match &expression.value {
            ExprKind::Call { callee, arguments } => {
                let (base, argument_count) =
                    self.compile_call_window(callee, arguments, expression.location)?;
                self.emit(
                    Operation::TailCall {
                        base,
                        argument_count,
                    },
                    expression.location,
                );
            }
            ExprKind::Block(block) => self.compile_tail_block(block)?,
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => self.compile_tail_if(condition, then_branch, else_branch)?,
            ExprKind::Match { value, arms } => {
                self.compile_tail_match(value, arms, expression.location)?
            }
            ExprKind::Return { .. } => {
                self.compile_expr(expression)?;
            }
            _ => {
                let result = self.compile_expr(expression)?;
                self.emit_synthetic(Operation::Return { src: result }, expression.location);
            }
        }
        Ok(())
    }

    fn compile_call_window(
        &mut self,
        callee: &Expr,
        arguments: &[Expr],
        location: Location,
    ) -> Result<(RegisterId, u32), FrontendError> {
        let callee = self.compile_expr(callee)?;
        let arguments = self.compile_many(arguments)?;
        let base = self.allocate();
        self.emit(
            Operation::Move {
                dst: base,
                src: callee,
            },
            location,
        );
        for argument in &arguments {
            let destination = self.allocate();
            self.emit(
                Operation::Move {
                    dst: destination,
                    src: *argument,
                },
                location,
            );
        }
        let argument_count = u32::try_from(arguments.len())
            .map_err(|_| frontend_error(self.source_name, "too many call arguments"))?;
        Ok((base, argument_count))
    }

    fn compile_many(&mut self, expressions: &[Expr]) -> Result<Vec<RegisterId>, FrontendError> {
        expressions
            .iter()
            .map(|expression| self.compile_expr(expression))
            .collect()
    }

    fn compile_spread_array(
        &mut self,
        items: &[Expr],
        location: Location,
    ) -> Result<RegisterId, FrontendError> {
        let mut arrays = Vec::with_capacity(items.len());
        for item in items {
            if let ExprKind::Spread(operand) = &item.value {
                arrays.push(self.compile_expr(operand)?);
            } else {
                let value = self.compile_expr(item)?;
                let array = self.allocate();
                self.emit(
                    Operation::MakeArray {
                        dst: array,
                        items: vec![value],
                    },
                    item.location,
                );
                arrays.push(array);
            }
        }
        let dst = self.allocate();
        self.emit(Operation::ConcatArrays { dst, arrays }, location);
        Ok(dst)
    }

    fn compile_dict(
        &mut self,
        fields: &[DictField],
        location: Location,
    ) -> Result<RegisterId, FrontendError> {
        if fields.iter().any(|field| field.value.name.is_none()) {
            return self.compile_spread_dict(fields, location);
        }
        let mut seen = HashSet::new();
        let mut compiled = Vec::with_capacity(fields.len());
        for field in fields {
            let name = &field
                .value
                .name
                .as_ref()
                .expect("ordinary Dict field has a name")
                .value;
            if !seen.insert(name) {
                return Err(frontend_error(
                    self.source_name,
                    format!("duplicate Dict field {name:?}"),
                ));
            }
            compiled.push((name.clone(), self.compile_expr(&field.value.value)?));
        }
        let dst = self.allocate();
        self.emit(
            Operation::MakeDict {
                dst,
                fields: compiled,
            },
            location,
        );
        Ok(dst)
    }

    fn compile_spread_dict(
        &mut self,
        fields: &[DictField],
        location: Location,
    ) -> Result<RegisterId, FrontendError> {
        let mut seen = HashSet::new();
        let mut dicts = Vec::with_capacity(fields.len());
        for field in fields {
            if let Some(name) = &field.value.name {
                if !seen.insert(&name.value) {
                    return Err(frontend_error(
                        self.source_name,
                        format!("duplicate Dict field {:?}", name.value),
                    ));
                }
                let value = self.compile_expr(&field.value.value)?;
                let dict = self.allocate();
                self.emit(
                    Operation::MakeDict {
                        dst: dict,
                        fields: vec![(name.value.clone(), value)],
                    },
                    field.location,
                );
                dicts.push(dict);
            } else {
                let ExprKind::Spread(operand) = &field.value.value.value else {
                    return Err(self.error_at(field.location, "invalid Dict spread entry"));
                };
                dicts.push(self.compile_expr(operand)?);
            }
        }
        let dst = self.allocate();
        self.emit(Operation::MergeDicts { dst, dicts }, location);
        Ok(dst)
    }

    fn compile_closure(
        &mut self,
        parameters: &[Identifier],
        body: &Block,
        location: Location,
    ) -> Result<RegisterId, FrontendError> {
        self.compile_closure_with_declared_family(parameters, body, location, None)
    }

    fn compile_closure_with_declared_family(
        &mut self,
        parameters: &[Identifier],
        body: &Block,
        location: Location,
        nominal_constructor: Option<NominalTypeConstructor>,
    ) -> Result<RegisterId, FrontendError> {
        let mut bound = parameters
            .iter()
            .map(|parameter| parameter.value.clone())
            .collect::<HashSet<_>>();
        if bound.len() != parameters.len() {
            return Err(frontend_error(
                self.source_name,
                "duplicate closure parameter",
            ));
        }
        let mut free = BTreeSet::new();
        free_block(body, &mut bound, &mut free);
        let mut captures = free.into_iter().collect::<Vec<_>>();
        let mut capture_registers = Vec::with_capacity(captures.len());
        for name in &captures {
            let register = if let Some(register) = self.environment.get(name).copied() {
                register
            } else {
                return Err(frontend_error(
                    self.source_name,
                    format!("unknown binding {name:?}"),
                ));
            };
            if self.ready_type_slot_bindings.contains(name) {
                let value = self.allocate();
                self.emit(
                    Operation::ReadTypeSlot {
                        dst: value,
                        link: register,
                    },
                    location,
                );
                capture_registers.push(value);
            } else {
                capture_registers.push(register);
            }
        }
        for owner in self
            .declared_value_owners
            .iter()
            .filter(|(owner_location, _)| {
                body.location.start <= owner_location.start
                    && owner_location.end <= body.location.end
            })
            .map(|(_, owner)| owner)
            .cloned()
            .collect::<BTreeSet<_>>()
        {
            if captures.contains(&owner) {
                continue;
            }
            let register = self
                .environment
                .get(&owner)
                .copied()
                .unwrap_or_else(|| self.load_external_constant(owner.clone(), location));
            captures.push(owner);
            capture_registers.push(register);
        }
        let captured_type_slots = captures
            .iter()
            .filter(|name| {
                self.type_slot_bindings.contains(*name)
                    && !self.ready_type_slot_bindings.contains(*name)
            })
            .cloned()
            .collect::<HashSet<_>>();
        let captured_definitions = captures
            .iter()
            .filter(|name| self.definition_bindings.contains(*name))
            .cloned()
            .collect::<HashSet<_>>();

        let name = format!("{}::closure{}", self.function_name, self.closure_index);
        self.closure_index += 1;
        let mut nested = Self::nested(
            self.source_name,
            self.source_file,
            name,
            parameters,
            NestedEnvironment {
                captures: &captures,
                type_slots: &captured_type_slots,
                definitions: &captured_definitions,
                declared_value_owners: &self.declared_value_owners,
            },
        )?;
        if let Some(constructor) = nominal_constructor {
            let structural = nested.compile_block(body)?;
            let native = Constant::Native(NativeFunction::new(
                "type-family.declare",
                parameters.len() + 4,
                crate::types::native_declare_type_family,
            ));
            let native = nested.load_constant(native, location);
            let module = nested.load_constant(
                Constant::Int(i64::from(constructor.id.module.raw())),
                location,
            );
            let local =
                nested.load_constant(Constant::Int(i64::from(constructor.id.local)), location);
            let name = nested.load_constant(Constant::String(constructor.name.into()), location);
            let base = nested.allocate();
            nested.emit(
                Operation::Move {
                    dst: base,
                    src: native,
                },
                location,
            );
            for source in std::iter::once(structural)
                .chain(std::iter::once(module))
                .chain(std::iter::once(local))
                .chain(std::iter::once(name))
                .chain((0..parameters.len()).map(|index| RegisterId(index as u32)))
            {
                let destination = nested.allocate();
                nested.emit(
                    Operation::Move {
                        dst: destination,
                        src: source,
                    },
                    location,
                );
            }
            nested.emit(
                Operation::Call {
                    base,
                    argument_count: u32::try_from(parameters.len() + 4).map_err(|_| {
                        frontend_error(self.source_name, "too many type parameters")
                    })?,
                },
                location,
            );
            nested.emit(Operation::Return { src: base }, location);
        } else {
            nested.compile_tail_block(body)?;
        }
        let function = Box::new(nested.finish_lir());

        let dst = self.allocate();
        self.emit(
            Operation::MakeClosure {
                dst,
                function,
                captures: capture_registers,
            },
            location,
        );
        Ok(dst)
    }

    fn compile_if(
        &mut self,
        condition: &Expr,
        then_branch: &Block,
        else_branch: &Block,
        location: Location,
    ) -> Result<RegisterId, FrontendError> {
        let condition_location = condition.location;
        let condition = self.compile_expr(condition)?;
        let else_label = self.new_label();
        self.emit(
            Operation::JumpIfFalse {
                condition,
                target: else_label,
            },
            condition_location,
        );
        let then_value = self.compile_block(then_branch)?;
        let result = self.allocate();
        self.emit_synthetic(
            Operation::Move {
                dst: result,
                src: then_value,
            },
            then_branch.location,
        );
        let end_label = self.new_label();
        self.emit_synthetic(Operation::Jump { target: end_label }, location);
        self.mark_label(else_label);
        let else_value = self.compile_block(else_branch)?;
        self.emit_synthetic(
            Operation::Move {
                dst: result,
                src: else_value,
            },
            else_branch.location,
        );
        self.mark_label(end_label);
        Ok(result)
    }

    fn compile_tail_if(
        &mut self,
        condition: &Expr,
        then_branch: &Block,
        else_branch: &Block,
    ) -> Result<(), FrontendError> {
        let condition_location = condition.location;
        let condition = self.compile_expr(condition)?;
        let else_label = self.new_label();
        self.emit(
            Operation::JumpIfFalse {
                condition,
                target: else_label,
            },
            condition_location,
        );
        self.compile_tail_block(then_branch)?;
        self.mark_label(else_label);
        self.compile_tail_block(else_branch)
    }

    fn compile_match(
        &mut self,
        value: &Expr,
        arms: &[MatchArm],
        location: Location,
    ) -> Result<RegisterId, FrontendError> {
        let value = self.compile_expr(value)?;
        let result = self.allocate();
        let mut end_jumps = Vec::new();

        for arm in arms {
            let outer = self.environment.clone();
            let mut failures = Vec::new();
            let mut pattern_bindings = HashSet::new();
            self.compile_pattern(
                &arm.value.pattern,
                value,
                &mut failures,
                &mut pattern_bindings,
            )?;
            if let Some(guard) = &arm.value.guard {
                let condition = self.compile_expr(guard)?;
                let failure = self.new_label();
                self.emit(
                    Operation::JumpIfFalse {
                        condition,
                        target: failure,
                    },
                    guard.location,
                );
                failures.push(failure);
            }
            let arm_value = self.compile_expr(&arm.value.value)?;
            self.emit_synthetic(
                Operation::Move {
                    dst: result,
                    src: arm_value,
                },
                arm.location,
            );
            let end = self.new_label();
            self.emit_synthetic(Operation::Jump { target: end }, arm.location);
            end_jumps.push(end);
            for failure in failures {
                self.mark_label(failure);
            }
            self.environment = outer;
        }

        self.emit(
            Operation::Fail {
                message: "no match arm accepted the value".into(),
            },
            location,
        );
        for jump in end_jumps {
            self.mark_label(jump);
        }
        Ok(result)
    }

    fn compile_tail_match(
        &mut self,
        value: &Expr,
        arms: &[MatchArm],
        location: Location,
    ) -> Result<(), FrontendError> {
        let value = self.compile_expr(value)?;

        for arm in arms {
            let outer = self.environment.clone();
            let mut failures = Vec::new();
            let mut pattern_bindings = HashSet::new();
            self.compile_pattern(
                &arm.value.pattern,
                value,
                &mut failures,
                &mut pattern_bindings,
            )?;
            if let Some(guard) = &arm.value.guard {
                let condition = self.compile_expr(guard)?;
                let failure = self.new_label();
                self.emit(
                    Operation::JumpIfFalse {
                        condition,
                        target: failure,
                    },
                    guard.location,
                );
                failures.push(failure);
            }
            self.compile_tail_expr(&arm.value.value)?;
            for failure in failures {
                self.mark_label(failure);
            }
            self.environment = outer;
        }

        self.emit(
            Operation::Fail {
                message: "no match arm accepted the value".into(),
            },
            location,
        );
        Ok(())
    }

    fn compile_pattern(
        &mut self,
        pattern: &Pattern,
        value: RegisterId,
        failures: &mut Vec<LabelId>,
        bindings: &mut HashSet<String>,
    ) -> Result<(), FrontendError> {
        match &pattern.value {
            PatternKind::Wildcard => {}
            PatternKind::Binding(name) => {
                if !bindings.insert(name.value.clone()) {
                    return Err(frontend_error(
                        self.source_name,
                        format!("duplicate pattern binding {:?}", name.value),
                    ));
                }
                self.environment.insert(name.value.clone(), value);
            }
            PatternKind::Int(item) => {
                let expected = self.load_constant(Constant::Int(*item), pattern.location);
                self.emit_pattern_equality(value, expected, failures, pattern.location);
            }
            PatternKind::Float(item) => {
                let expected = self.load_constant(Constant::Float(*item), pattern.location);
                self.emit_pattern_equality(value, expected, failures, pattern.location);
            }
            PatternKind::String(item) => {
                let expected =
                    self.load_constant(Constant::String(item.clone().into()), pattern.location);
                self.emit_pattern_equality(value, expected, failures, pattern.location);
            }
            PatternKind::Atom(item) => {
                let expected = self.load_constant(atom_constant(item), pattern.location);
                let condition = self.allocate();
                self.emit(
                    Operation::TaggedTagEquals {
                        dst: condition,
                        value,
                        tag: expected,
                    },
                    pattern.location,
                );
                let failure = self.new_label();
                self.emit(
                    Operation::JumpIfFalse {
                        condition,
                        target: failure,
                    },
                    pattern.location,
                );
                failures.push(failure);
            }
            PatternKind::Tagged { tag, payload } => {
                let expected = self.load_constant(atom_constant(tag), pattern.location);
                let condition = self.allocate();
                self.emit(
                    Operation::TaggedTagEquals {
                        dst: condition,
                        value,
                        tag: expected,
                    },
                    pattern.location,
                );
                let failure = self.new_label();
                self.emit(
                    Operation::JumpIfFalse {
                        condition,
                        target: failure,
                    },
                    pattern.location,
                );
                failures.push(failure);
                let payload_value = self.allocate();
                self.emit(
                    Operation::GetTaggedPayload {
                        dst: payload_value,
                        value,
                    },
                    pattern.location,
                );
                self.compile_pattern(payload, payload_value, failures, bindings)?;
            }
            PatternKind::Tuple(items) => {
                let condition = self.allocate();
                self.emit(
                    Operation::TupleLengthEquals {
                        dst: condition,
                        value,
                        length: items.len(),
                    },
                    pattern.location,
                );
                let failure = self.new_label();
                self.emit(
                    Operation::JumpIfFalse {
                        condition,
                        target: failure,
                    },
                    pattern.location,
                );
                failures.push(failure);
                for (index, pattern) in items.iter().enumerate() {
                    let element = self.allocate();
                    self.emit(
                        Operation::GetTuple {
                            dst: element,
                            tuple: value,
                            index,
                        },
                        pattern.location,
                    );
                    self.compile_pattern(pattern, element, failures, bindings)?;
                }
            }
            PatternKind::Struct(fields) => {
                let condition = self.allocate();
                self.emit(
                    Operation::IsDict {
                        dst: condition,
                        value,
                    },
                    pattern.location,
                );
                let failure = self.new_label();
                self.emit(
                    Operation::JumpIfFalse {
                        condition,
                        target: failure,
                    },
                    pattern.location,
                );
                failures.push(failure);
                let mut field_names = HashSet::new();
                for field in fields {
                    if !field_names.insert(field.name.value.clone()) {
                        return Err(frontend_error(
                            self.source_name,
                            format!("duplicate Struct pattern field {:?}", field.name.value),
                        ));
                    }
                    let condition = self.allocate();
                    self.emit(
                        Operation::FieldExists {
                            dst: condition,
                            value,
                            field: field.name.value.clone(),
                        },
                        field.name.location,
                    );
                    let failure = self.new_label();
                    self.emit(
                        Operation::JumpIfFalse {
                            condition,
                            target: failure,
                        },
                        field.name.location,
                    );
                    failures.push(failure);
                    let selected = self.allocate();
                    self.emit(
                        Operation::GetField {
                            dst: selected,
                            dict: value,
                            field: field.name.value.clone(),
                        },
                        field.name.location,
                    );
                    self.compile_pattern(&field.pattern, selected, failures, bindings)?;
                }
            }
        }
        Ok(())
    }

    fn emit_pattern_equality(
        &mut self,
        value: RegisterId,
        expected: RegisterId,
        failures: &mut Vec<LabelId>,
        location: Location,
    ) {
        let condition = self.allocate();
        self.emit(
            Operation::Equal {
                dst: condition,
                left: value,
                right: expected,
            },
            location,
        );
        let failure = self.new_label();
        self.emit(
            Operation::JumpIfFalse {
                condition,
                target: failure,
            },
            location,
        );
        failures.push(failure);
    }

    fn load_constant(&mut self, value: Constant, location: Location) -> RegisterId {
        let constant = self.constants.len();
        self.constants.push(value);
        let dst = self.allocate();
        self.emit(
            Operation::LoadConst {
                dst,
                constant: ConstantId(u32::try_from(constant).expect("constant pool exceeds u32")),
            },
            location,
        );
        dst
    }

    fn load_external_constant(&mut self, key: String, location: Location) -> RegisterId {
        let index = self.constants.len();
        let register = self.load_constant(Constant::Placeholder, location);
        self.external_constant_links.push((index, key));
        register
    }

    fn allocate(&mut self) -> RegisterId {
        let register = RegisterId(self.next_register);
        self.next_register = self
            .next_register
            .checked_add(1)
            .expect("register count exceeds u32");
        register
    }

    fn emit(&mut self, operation: Operation, location: Location) {
        self.items.push(Item::Operation(WithOrigin {
            value: operation,
            origin: Origin::Source(location),
        }));
    }

    fn emit_synthetic(&mut self, operation: Operation, derived_from: Location) {
        self.items.push(Item::Operation(WithOrigin {
            value: operation,
            origin: Origin::Synthetic {
                derived_from: Some(derived_from),
            },
        }));
    }

    fn new_label(&mut self) -> LabelId {
        let label = LabelId(self.next_label);
        self.next_label = self
            .next_label
            .checked_add(1)
            .expect("label count exceeds u32");
        label
    }

    fn mark_label(&mut self, label: LabelId) {
        self.items.push(Item::Label(label));
    }
}

fn atom_constant(name: &str) -> Constant {
    let builtin = match name {
        "None" => Some(BuiltinAtom::None),
        "Some" => Some(BuiltinAtom::Some),
        "Ok" => Some(BuiltinAtom::Ok),
        "Err" => Some(BuiltinAtom::Err),
        "True" => Some(BuiltinAtom::True),
        "False" => Some(BuiltinAtom::False),
        _ => None,
    };
    Constant::Atom(match builtin {
        Some(builtin) => Atom::builtin(builtin),
        None => Atom::named(name),
    })
}

pub(crate) fn function_contract_arity(contract: &Expr) -> Option<u32> {
    let ExprKind::Call { callee, arguments } = &contract.value else {
        return None;
    };
    let ExprKind::Variable(name) = &callee.value else {
        return None;
    };
    if name.value != "Func" || arguments.len() != 2 {
        return None;
    }
    let ExprKind::Array(parameters) = &arguments[0].value else {
        return None;
    };
    u32::try_from(parameters.len()).ok()
}

fn free_block(block: &Block, bound: &mut HashSet<String>, free: &mut BTreeSet<String>) {
    for binding in &block.value.bindings {
        if matches!(binding.value.kind, BindingKind::Decl | BindingKind::Native) {
            bound.insert(binding.value.name.value.clone());
        }
    }
    for binding in &block.value.bindings {
        if !matches!(binding.value.kind, BindingKind::Decl | BindingKind::Native) {
            free_expr(&binding.value.value, bound, free);
        }
        bound.insert(binding.value.name.value.clone());
    }
    free_expr(&block.value.result, bound, free);
}

fn free_expr(expression: &Expr, bound: &HashSet<String>, free: &mut BTreeSet<String>) {
    match &expression.value {
        ExprKind::Variable(name) => {
            if !bound.contains(&name.value) {
                free.insert(name.value.clone());
            }
        }
        ExprKind::Array(items) | ExprKind::Tuple(items) => {
            for item in items {
                free_expr(item, bound, free);
            }
        }
        ExprKind::Spread(operand) => free_expr(operand, bound, free),
        ExprKind::InterpolatedString(parts) => {
            for part in parts {
                if let StringPartKind::Expression(expression) = &part.value {
                    free_expr(expression, bound, free);
                }
            }
        }
        ExprKind::Dict(fields) => {
            for field in fields {
                free_expr(&field.value.value, bound, free);
            }
        }
        ExprKind::Block(block) => {
            let mut inner = bound.clone();
            free_block(block, &mut inner, free);
        }
        ExprKind::Unary { operand, .. } | ExprKind::Propagate { operand } => {
            free_expr(operand, bound, free)
        }
        ExprKind::Return { value } => free_expr(value, bound, free),
        ExprKind::Panic { message } => free_expr(message, bound, free),
        ExprKind::Raise { error } => free_expr(error, bound, free),
        ExprKind::Debug { value, .. } => free_expr(value, bound, free),
        ExprKind::Binary { left, right, .. } => {
            free_expr(left, bound, free);
            free_expr(right, bound, free);
        }
        ExprKind::Field { receiver, .. } => free_expr(receiver, bound, free),
        ExprKind::Index { receiver, index } => {
            free_expr(receiver, bound, free);
            free_expr(index, bound, free);
        }
        ExprKind::TupleProjection { receiver, .. } => free_expr(receiver, bound, free),
        ExprKind::TypeAscription { value, .. } => free_expr(value, bound, free),
        ExprKind::CheckedCast { value, target } => {
            free.insert("\0telora_cast".to_owned());
            free_expr(target, bound, free);
            free_expr(value, bound, free);
        }
        ExprKind::DynProject {
            namespace,
            target,
            value,
        } => {
            free_expr(namespace, bound, free);
            free_expr(target, bound, free);
            free_expr(value, bound, free);
        }
        ExprKind::Call { callee, arguments } => {
            free_expr(callee, bound, free);
            for argument in arguments {
                free_expr(argument, bound, free);
            }
        }
        ExprKind::TypeApply { callee, .. } => free_expr(callee, bound, free),
        ExprKind::Interpreter { elaboration, .. } => free_expr(elaboration, bound, free),
        ExprKind::Closure {
            parameters, body, ..
        } => {
            let mut closure_bound = parameters
                .iter()
                .map(|parameter| parameter.name.value.clone())
                .collect::<HashSet<_>>();
            let mut closure_free = BTreeSet::new();
            free_block(body, &mut closure_bound, &mut closure_free);
            for name in closure_free {
                if !bound.contains(&name) {
                    free.insert(name);
                }
            }
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            free_expr(condition, bound, free);
            let mut then_bound = bound.clone();
            free_block(then_branch, &mut then_bound, free);
            let mut else_bound = bound.clone();
            free_block(else_branch, &mut else_bound, free);
        }
        ExprKind::IfLet {
            pattern,
            value,
            then_branch,
            else_branch,
        } => {
            free_expr(value, bound, free);
            let mut then_bound = bound.clone();
            bind_pattern(pattern, &mut then_bound);
            free_block(then_branch, &mut then_bound, free);
            let mut else_bound = bound.clone();
            free_block(else_branch, &mut else_bound, free);
        }
        ExprKind::LetElse {
            pattern,
            value,
            else_branch,
            body,
        } => {
            free_expr(value, bound, free);
            let mut else_bound = bound.clone();
            free_block(else_branch, &mut else_bound, free);
            let mut body_bound = bound.clone();
            bind_pattern(pattern, &mut body_bound);
            free_block(body, &mut body_bound, free);
        }
        ExprKind::Match { value, arms } => {
            free_expr(value, bound, free);
            for arm in arms {
                let mut arm_bound = bound.clone();
                bind_pattern(&arm.value.pattern, &mut arm_bound);
                if let Some(guard) = &arm.value.guard {
                    free_expr(guard, &arm_bound, free);
                }
                free_expr(&arm.value.value, &arm_bound, free);
            }
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Bytes(_)
        | ExprKind::Atom(_) => {}
    }
}

fn bind_pattern(pattern: &Pattern, bound: &mut HashSet<String>) {
    match &pattern.value {
        PatternKind::Binding(name) => {
            bound.insert(name.value.clone());
        }
        PatternKind::Tuple(items) => {
            for item in items {
                bind_pattern(item, bound);
            }
        }
        PatternKind::Tagged { payload, .. } => bind_pattern(payload, bound),
        PatternKind::Struct(fields) => {
            for field in fields {
                bind_pattern(&field.pattern, bound);
            }
        }
        PatternKind::Wildcard
        | PatternKind::Int(_)
        | PatternKind::Float(_)
        | PatternKind::String(_)
        | PatternKind::Atom(_) => {}
    }
}

fn collect_runtime_names_block(block: &Block, names: &mut HashSet<String>) {
    for binding in &block.value.bindings {
        if matches!(binding.value.kind, BindingKind::Let | BindingKind::Def) {
            collect_runtime_names(&binding.value.value, names);
        }
    }
    collect_runtime_names(&block.value.result, names);
}

fn collect_decorator_runtime_names(decorator: &crate::ast::Decorator, names: &mut HashSet<String>) {
    collect_runtime_names(&decorator.value.callee, names);
    for argument in &decorator.value.arguments {
        collect_runtime_names(argument, names);
    }
}

pub(crate) fn collect_runtime_names(expression: &Expr, names: &mut HashSet<String>) {
    match &expression.value {
        ExprKind::Variable(name) => {
            names.insert(name.value.clone());
        }
        ExprKind::Array(items) | ExprKind::Tuple(items) => {
            for item in items {
                collect_runtime_names(item, names);
            }
        }
        ExprKind::Spread(operand) => collect_runtime_names(operand, names),
        ExprKind::InterpolatedString(parts) => {
            for part in parts {
                if let StringPartKind::Expression(expression) = &part.value {
                    collect_runtime_names(expression, names);
                }
            }
        }
        ExprKind::Dict(fields) => {
            for field in fields {
                for decorator in &field.value.decorators {
                    collect_decorator_runtime_names(decorator, names);
                }
                collect_runtime_names(&field.value.value, names);
            }
        }
        ExprKind::Block(block) => collect_runtime_names_block(block, names),
        ExprKind::Unary { operand, .. } | ExprKind::Propagate { operand } => {
            collect_runtime_names(operand, names)
        }
        ExprKind::Return { value } => collect_runtime_names(value, names),
        ExprKind::Panic { message } => collect_runtime_names(message, names),
        ExprKind::Raise { error } => collect_runtime_names(error, names),
        ExprKind::Debug { value, .. } => collect_runtime_names(value, names),
        ExprKind::Binary { left, right, .. } => {
            collect_runtime_names(left, names);
            collect_runtime_names(right, names);
        }
        ExprKind::Field { receiver, .. } => collect_runtime_names(receiver, names),
        ExprKind::Index { receiver, index } => {
            collect_runtime_names(receiver, names);
            collect_runtime_names(index, names);
        }
        ExprKind::TupleProjection { receiver, .. } => collect_runtime_names(receiver, names),
        ExprKind::TypeAscription { value, .. } => collect_runtime_names(value, names),
        ExprKind::CheckedCast { value, target } => {
            names.insert("\0telora_cast".to_owned());
            collect_runtime_names(target, names);
            collect_runtime_names(value, names);
        }
        ExprKind::DynProject {
            namespace,
            target,
            value,
        } => {
            collect_runtime_names(namespace, names);
            collect_runtime_names(target, names);
            collect_runtime_names(value, names);
        }
        ExprKind::Call { callee, arguments } => {
            collect_runtime_names(callee, names);
            for argument in arguments {
                collect_runtime_names(argument, names);
            }
        }
        ExprKind::TypeApply { callee, .. } => collect_runtime_names(callee, names),
        ExprKind::Interpreter { elaboration, .. } => {
            collect_runtime_names(elaboration, names);
        }
        ExprKind::Closure { body, .. } => collect_runtime_names_block(body, names),
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_runtime_names(condition, names);
            collect_runtime_names_block(then_branch, names);
            collect_runtime_names_block(else_branch, names);
        }
        ExprKind::IfLet {
            value,
            then_branch,
            else_branch,
            ..
        } => {
            collect_runtime_names(value, names);
            collect_runtime_names_block(then_branch, names);
            collect_runtime_names_block(else_branch, names);
        }
        ExprKind::LetElse {
            value,
            else_branch,
            body,
            ..
        } => {
            collect_runtime_names(value, names);
            collect_runtime_names_block(else_branch, names);
            collect_runtime_names_block(body, names);
        }
        ExprKind::Match { value, arms } => {
            collect_runtime_names(value, names);
            for arm in arms {
                if let Some(guard) = &arm.value.guard {
                    collect_runtime_names(guard, names);
                }
                collect_runtime_names(&arm.value.value, names);
            }
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Bytes(_)
        | ExprKind::Atom(_) => {}
    }
}

fn frontend_error(source_name: &str, message: impl Into<String>) -> FrontendError {
    FrontendError::new(
        source_name,
        SourceLocation {
            offset: 0,
            line: 1,
            column: 1,
        },
        message,
    )
}

#[cfg(test)]
#[path = "compiler/tests/mod.rs"]
mod tests;
