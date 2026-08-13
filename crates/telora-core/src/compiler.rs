use crate::ast::{
    BinaryOperator, BindingKind, Block, BlockKind, DictField, DictFieldKind, Expr, ExprKind,
    Identifier, MatchArm, Pattern, PatternKind, Program, ProgramKind, StringPartKind,
    UnaryOperator, located,
};
use crate::bytecode::BytecodeFunction;
use crate::hir::HirProgram;
use crate::lexer::{FrontendError, SourceLocation};
use crate::lir::{self, ConstantId, Item, LabelId, Operation, RegisterId};
use crate::parser::parse_registered;
use crate::source::{Diagnostic, Location, Origin, SourceDatabase, SourceFile, WithOrigin};
use crate::types::{Analysis, TypeDescriptor, analyze_program_registered, contains_named_type};
use crate::value::{Atom, BuiltinAtom, Value};
use crate::{RuntimeError, Vm};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;

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

pub fn compile_source(source_name: &str, source: &str) -> Result<BytecodeFunction, FrontendError> {
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
    let analysis = analyze_program_registered(source_name, &sources, &program, 100_000)?;
    compile_program_analyzed_in(sources.get(source_id), &program, &analysis)
}

pub(crate) fn compile_program_analyzed_in(
    source_file: &SourceFile,
    program: &Program,
    analysis: &Analysis,
) -> Result<BytecodeFunction, FrontendError> {
    compile_program_with_promoted_types(
        source_file,
        program,
        analysis,
        &HashSet::new(),
        &HashSet::new(),
    )
}

pub(crate) fn compile_program_with_promoted_types(
    source_file: &SourceFile,
    program: &Program,
    analysis: &Analysis,
    promoted_types: &HashSet<String>,
    erased_bindings: &HashSet<String>,
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
    )
}

pub(crate) fn type_link_key(name: &str) -> String {
    format!("type:{name}")
}

pub(crate) struct MetadataInitializer {
    pub(crate) function: BytecodeFunction,
    pub(crate) type_names: Vec<String>,
    pub(crate) erased_bindings: HashSet<String>,
}

pub(crate) fn compile_metadata_initializer(
    source_file: &SourceFile,
    program: &Program,
    analysis: &Analysis,
) -> Result<Option<MetadataInitializer>, FrontendError> {
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
        return Ok(None);
    }
    type_names.sort();
    type_names.dedup();

    let mut needed = type_names.iter().cloned().collect::<HashSet<_>>();
    loop {
        let before = needed.len();
        for binding in &program.value.body.value.bindings {
            if needed.contains(&binding.value.name.value) {
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
    let bindings = program
        .value
        .body
        .value
        .bindings
        .iter()
        .filter(|binding| needed.contains(&binding.value.name.value))
        .cloned()
        .collect();
    let result = located(
        ExprKind::Dict(
            type_names
                .iter()
                .map(|name| {
                    located(
                        DictFieldKind {
                            decorators: Vec::new(),
                            name: Some(located(name.clone(), program.location)),
                            value: located(
                                ExprKind::Variable(located(name.clone(), program.location)),
                                program.location,
                            ),
                        },
                        program.location,
                    )
                })
                .collect(),
        ),
        program.location,
    );
    let metadata_program = located(
        ProgramKind {
            options: Vec::new(),
            body: located(
                BlockKind {
                    bindings,
                    result: Box::new(result),
                },
                program.value.body.location,
            ),
            authored_result: true,
        },
        program.location,
    );
    let metadata_hir = HirProgram::resolve(
        &metadata_program,
        analysis
            .prelude
            .keys()
            .chain(analysis.external_values.keys())
            .cloned()
            .collect::<Vec<_>>(),
    );
    validate_hir(source_file, &metadata_hir)?;
    let function = compile_program_analyzed_in(source_file, &metadata_program, analysis)?;
    Ok(Some(MetadataInitializer {
        function,
        type_names,
        erased_bindings,
    }))
}

pub fn run_source(
    source_name: &str,
    source: &str,
    evaluation_fuel: usize,
) -> Result<Value, ExecutionError> {
    let function = compile_source(source_name, source)?;
    let mut sources = SourceDatabase::default();
    sources.add(source_name, source);
    Vm::new()
        .execute(&function, evaluation_fuel)
        .map_err(|error| ExecutionError::Runtime(error.with_sources(&sources)))
}

pub(crate) fn compile_expression_with_bindings(
    source_name: &str,
    function_name: &str,
    expression: &Expr,
    bindings: &BTreeMap<String, Value>,
    source_file: &SourceFile,
) -> Result<BytecodeFunction, FrontendError> {
    let hir = HirProgram::resolve_expression(expression, bindings.keys().cloned());
    validate_hir(source_file, &hir)?;
    let mut compiler = Compiler {
        source_name,
        function_name: function_name.to_owned(),
        environment: HashMap::new(),
        up_link_bindings: HashSet::new(),
        ready_up_link_bindings: HashSet::new(),
        preserved_up_link_reads: HashSet::new(),
        definition_bindings: HashSet::new(),
        constants: Vec::new(),
        external_constant_links: Vec::new(),
        items: Vec::new(),
        next_register: 0,
        next_label: 0,
        parameter_count: 0,
        capture_count: 0,
        closure_index: 0,
        located_constants: HashMap::new(),
        retained_names: HashSet::new(),
        promoted_types: HashSet::new(),
        external_values: BTreeMap::new(),
        type_family_values: BTreeMap::new(),
        source_file: Some(source_file),
    };
    for (name, value) in bindings {
        let register = compiler.load_constant(value.clone(), expression.location);
        compiler.environment.insert(name.clone(), register);
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
    up_link_bindings: HashSet<String>,
    ready_up_link_bindings: HashSet<String>,
    preserved_up_link_reads: HashSet<String>,
    definition_bindings: HashSet<String>,
    constants: Vec<Value>,
    external_constant_links: Vec<(usize, String)>,
    items: Vec<Item>,
    next_register: u32,
    next_label: u32,
    parameter_count: u32,
    capture_count: u32,
    closure_index: usize,
    located_constants: HashMap<String, Value>,
    retained_names: HashSet<String>,
    promoted_types: HashSet<String>,
    external_values: BTreeMap<String, Value>,
    type_family_values: BTreeMap<String, Value>,
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
            up_link_bindings: HashSet::new(),
            ready_up_link_bindings: HashSet::new(),
            preserved_up_link_reads: HashSet::new(),
            definition_bindings: HashSet::new(),
            constants: Vec::new(),
            external_constant_links: Vec::new(),
            items: Vec::new(),
            next_register: 0,
            next_label: 0,
            parameter_count: 0,
            capture_count: 0,
            closure_index: 0,
            located_constants: HashMap::new(),
            retained_names,
            promoted_types,
            external_values: analysis.external_values.clone(),
            type_family_values: analysis.type_family_values.clone(),
            source_file,
        };
        for (name, value) in &analysis.prelude {
            if compiler.retained_names.contains(name) {
                if matches!(value, Value::Func(_)) {
                    let register = compiler.load_constant(value.clone(), program.location);
                    compiler.environment.insert(name.clone(), register);
                } else {
                    compiler
                        .located_constants
                        .insert(name.clone(), value.clone());
                }
            }
        }
        for name in &analysis.dynamic_bindings {
            if compiler.retained_names.contains(name) {
                let value = analysis
                    .external_values
                    .get(name)
                    .expect("analyzed dynamic binding")
                    .clone();
                let register = compiler.load_constant(value, program.location);
                compiler.environment.insert(name.clone(), register);
            }
        }
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
        for (name, value) in &analysis.external_values {
            if !authored_names.contains(name.as_str())
                && compiler.retained_names.contains(name)
                && !compiler.environment.contains_key(name)
            {
                let register =
                    compiler.load_external_constant(value.clone(), name.clone(), program.location);
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
        captures: &[String],
        captured_up_links: &HashSet<String>,
        captured_definitions: &HashSet<String>,
    ) -> Result<Self, FrontendError> {
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
            up_link_bindings: captured_up_links.clone(),
            ready_up_link_bindings: HashSet::new(),
            preserved_up_link_reads: HashSet::new(),
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
            located_constants: HashMap::new(),
            retained_names: HashSet::new(),
            promoted_types: HashSet::new(),
            external_values: BTreeMap::new(),
            type_family_values: BTreeMap::new(),
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
        let outer_up_links = self.up_link_bindings.clone();
        let outer_ready_up_links = self.ready_up_link_bindings.clone();
        let outer_definitions = self.definition_bindings.clone();
        let mut declared = HashMap::<String, (RegisterId, Location, Option<u32>)>::new();
        let mut type_links = HashMap::<String, (RegisterId, Location)>::new();
        let mut native_declarations = HashMap::<String, Location>::new();
        let mut definition_counts = HashMap::<String, usize>::new();

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
            if binding.value.kind == BindingKind::Decl
                || binding.value.kind == BindingKind::Def && binding.value.annotation.is_some()
                || binding.value.kind == BindingKind::Def
                    && matches!(binding.value.value.value, ExprKind::Closure { .. })
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
                self.emit(Operation::MakeUpLink { dst: link }, binding.location);
                self.environment.insert(name.clone(), link);
                self.up_link_bindings.insert(name.clone());
                self.definition_bindings.insert(name.clone());
                let arity = binding
                    .value
                    .annotation
                    .as_ref()
                    .and_then(function_contract_arity)
                    .or_else(|| match &binding.value.value.value {
                        ExprKind::Closure { parameters, .. } => {
                            u32::try_from(parameters.len()).ok()
                        }
                        _ => None,
                    });
                declared.insert(name.clone(), (link, binding.location, arity));
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
            self.emit(Operation::MakeUpLink { dst: link }, binding.location);
            self.environment.insert(name.clone(), link);
            self.up_link_bindings.insert(name.clone());
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
                && !matches!(binding.value.kind, BindingKind::Decl | BindingKind::Def)
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
                            let register = self.load_external_constant(
                                Value::none(),
                                type_link_key(&name),
                                binding.location,
                            );
                            self.environment.insert(name, register);
                        } else if !binding.value.type_parameters.is_empty() {
                            let value = self
                                .type_family_values
                                .get(&name)
                                .expect("analyzed type family has a callable value")
                                .clone();
                            let has_named_metadata = match &value {
                                Value::Func(closure) => closure
                                    .upvalues()
                                    .first()
                                    .and_then(|metadata| TypeDescriptor::from_value(metadata).ok())
                                    .is_some_and(|metadata| contains_named_type(&metadata)),
                                _ => false,
                            };
                            let register = if has_named_metadata {
                                let body = located(
                                    BlockKind {
                                        bindings: Vec::new(),
                                        result: Box::new(binding.value.value.clone()),
                                    },
                                    binding.value.value.location,
                                );
                                self.compile_closure(
                                    &binding.value.type_parameters,
                                    &body,
                                    binding.location,
                                )?
                            } else {
                                self.load_constant(value, binding.location)
                            };
                            let (link, _) = type_links[&binding.value.name.value];
                            self.emit(
                                Operation::InitializeUpLink {
                                    link,
                                    src: register,
                                },
                                binding.location,
                            );
                            self.ready_up_link_bindings.insert(name);
                        } else {
                            self.preserved_up_link_reads = type_links
                                .keys()
                                .filter(|name| !self.type_family_values.contains_key(*name))
                                .cloned()
                                .collect();
                            let register = self.compile_expr(&binding.value.value)?;
                            self.preserved_up_link_reads.clear();
                            let (link, _) = type_links[&binding.value.name.value];
                            self.emit(
                                Operation::InitializeUpLink {
                                    link,
                                    src: register,
                                },
                                binding.location,
                            );
                            self.ready_up_link_bindings.insert(name);
                        }
                    }
                    continue;
                }
                BindingKind::Import | BindingKind::Native | BindingKind::NativeType => {
                    let value = self
                        .external_values
                        .get(&binding.value.name.value)
                        .cloned()
                        .ok_or_else(|| {
                            frontend_error(
                                self.source_name,
                                format!(
                                    "external binding {} has not been resolved",
                                    binding.value.name.value
                                ),
                            )
                        })?;
                    let register = self.load_external_constant(
                        value,
                        binding.value.name.value.clone(),
                        binding.location,
                    );
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
                && let Some((link, _, arity)) = declared.get(&name)
            {
                if let Some(arity) = arity {
                    self.emit(
                        Operation::AssertFunctionArity {
                            value,
                            arity: *arity,
                        },
                        binding.location,
                    );
                }
                self.emit(
                    Operation::InitializeUpLink {
                        link: *link,
                        src: value,
                    },
                    binding.location,
                );
                self.ready_up_link_bindings.insert(name);
            } else {
                self.environment.insert(name.clone(), value);
                self.up_link_bindings.remove(&name);
                if binding.value.kind == BindingKind::Def {
                    self.definition_bindings.insert(name);
                } else if binding.value.kind == BindingKind::Let {
                    self.definition_bindings.remove(&name);
                }
            }
        }
        for (link, location, _) in declared.values() {
            self.emit(Operation::AssertUpLinkReady { link: *link }, *location);
        }
        for (link, location) in type_links.values() {
            self.emit(Operation::AssertUpLinkReady { link: *link }, *location);
        }
        let result = if tail {
            self.compile_tail_expr(&block.value.result)?;
            None
        } else {
            Some(self.compile_expr(&block.value.result)?)
        };
        self.environment = outer;
        self.up_link_bindings = outer_up_links;
        self.ready_up_link_bindings = outer_ready_up_links;
        self.definition_bindings = outer_definitions;
        Ok(result)
    }

    fn compile_expr(&mut self, expression: &Expr) -> Result<RegisterId, FrontendError> {
        match &expression.value {
            ExprKind::Int(value) => Ok(self.load_constant(Value::Int(*value), expression.location)),
            ExprKind::Float(value) => {
                Ok(self.load_constant(Value::Float(*value), expression.location))
            }
            ExprKind::String(value) => {
                Ok(self.load_constant(Value::string(value.clone()), expression.location))
            }
            ExprKind::InterpolatedString(parts) => {
                let mut registers = Vec::with_capacity(parts.len());
                for part in parts {
                    registers.push(match &part.value {
                        StringPartKind::Text(text) => {
                            self.load_constant(Value::string(text.clone()), part.location)
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
                Ok(self.load_constant(Value::Bytes(value.clone().into()), expression.location))
            }
            ExprKind::Atom(name) => Ok(self.load_constant(atom_value(name), expression.location)),
            ExprKind::Variable(name) => {
                if let Some(value) = self.located_constants.get(&name.value).cloned() {
                    return Ok(self.load_constant(value, expression.location));
                }
                let register = self.environment.get(&name.value).copied().ok_or_else(|| {
                    self.error_at(
                        expression.location,
                        format!("unknown binding {:?}", name.value),
                    )
                })?;
                if self.up_link_bindings.contains(&name.value)
                    && !self.preserved_up_link_reads.contains(&name.value)
                {
                    let dst = self.allocate();
                    self.emit(
                        Operation::ReadUpLink {
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
        let captures = free.into_iter().collect::<Vec<_>>();
        let mut capture_registers = Vec::with_capacity(captures.len());
        for name in &captures {
            let register = if let Some(register) = self.environment.get(name).copied() {
                register
            } else if let Some(value) = self.located_constants.get(name).cloned() {
                self.load_constant(value, location)
            } else {
                return Err(frontend_error(
                    self.source_name,
                    format!("unknown binding {name:?}"),
                ));
            };
            if self.ready_up_link_bindings.contains(name) {
                let value = self.allocate();
                self.emit(
                    Operation::ReadUpLink {
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
        let captured_up_links = captures
            .iter()
            .filter(|name| {
                self.up_link_bindings.contains(*name)
                    && !self.ready_up_link_bindings.contains(*name)
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
            &captures,
            &captured_up_links,
            &captured_definitions,
        )?;
        nested.compile_tail_block(body)?;
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
                let expected = self.load_constant(Value::Int(*item), pattern.location);
                self.emit_pattern_equality(value, expected, failures, pattern.location);
            }
            PatternKind::Float(item) => {
                let expected = self.load_constant(Value::Float(*item), pattern.location);
                self.emit_pattern_equality(value, expected, failures, pattern.location);
            }
            PatternKind::String(item) => {
                let expected = self.load_constant(Value::string(item.clone()), pattern.location);
                self.emit_pattern_equality(value, expected, failures, pattern.location);
            }
            PatternKind::Atom(item) => {
                let expected = self.load_constant(atom_value(item), pattern.location);
                self.emit_pattern_equality(value, expected, failures, pattern.location);
            }
            PatternKind::Tagged { tag, payload } => {
                let expected = self.load_constant(atom_value(tag), pattern.location);
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

    fn load_constant(&mut self, value: Value, location: Location) -> RegisterId {
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

    fn load_external_constant(
        &mut self,
        value: Value,
        key: String,
        location: Location,
    ) -> RegisterId {
        let index = self.constants.len();
        let register = self.load_constant(value, location);
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

fn atom_value(name: &str) -> Value {
    let builtin = match name {
        "None" => Some(BuiltinAtom::None),
        "Some" => Some(BuiltinAtom::Some),
        "Ok" => Some(BuiltinAtom::Ok),
        "Err" => Some(BuiltinAtom::Err),
        "True" => Some(BuiltinAtom::True),
        "False" => Some(BuiltinAtom::False),
        _ => None,
    };
    Value::Atom(match builtin {
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
mod tests {
    use super::*;
    use crate::{Quota, RuntimeErrorKind};

    fn run(source: &str) -> Result<Value, ExecutionError> {
        run_source("test", source, 10_000)
    }

    #[test]
    fn executes_precedence_blocks_and_dict_access() {
        let value = run("let x = 2 + 3 * 4; {b: x, a: 1}.b").unwrap();
        assert!(matches!(value, Value::Int(14)));
    }

    #[test]
    fn decorators_transform_type_and_field_rhs_with_syntax_context() {
        let value = run(r#"
let choose: Fn(Any, Any) -> Any = fn(ctx, rhs) {
    if ctx.kind == 'Type {
        if ctx.name == "Alias" { rhs } else { 'Bad }
    } else {
        'Bad
    }
};
@choose
type Alias = Int;
let typed: Alias = 7;

let outer: Fn(Any, Int) -> Int = fn(ctx, rhs) { if ctx.name == "value" { rhs * 10 } else { 0 } };
let decorators = {
    add: fn(amount) { let decorate: Fn(Any, Int) -> Int = fn(ctx, rhs) { if ctx.kind == 'Field { rhs + amount } else { 0 } }; decorate },
};
{
    @outer
    @decorators.add(2)
    value: typed,
}
"#)
        .unwrap();
        assert_eq!(value.to_string(), "{value: 90}");
    }

    #[test]
    fn compares_tagged_tuples_structurally() {
        assert!(matches!(
            run("('Ok, 42) == ('Ok, 42)").unwrap(),
            Value::Atom(Atom::Builtin(BuiltinAtom::True))
        ));
        assert!(matches!(
            run("('Ok, 42) == ('Err, 42)").unwrap(),
            Value::Atom(Atom::Builtin(BuiltinAtom::False))
        ));
    }

    #[test]
    fn compares_functions_by_opaque_identity() {
        let value = run(
            "let f: Fn(Any) -> Any = fn(x) { x }; let same = f == f; let distinct = f == fn(x) { x }; (same, distinct)",
        )
        .unwrap();
        let Value::Tuple(values) = value else {
            panic!("expected tuple")
        };
        assert!(matches!(
            values.as_ref(),
            [
                Value::Atom(Atom::Builtin(BuiltinAtom::True)),
                Value::Atom(Atom::Builtin(BuiltinAtom::False))
            ]
        ));
    }

    #[test]
    fn executes_complete_numeric_comparison_semantics() {
        let integers = run("(1 == 1, 1 != 2, 1 < 2, 2 > 1, 1 <= 1, 2 >= 2)").unwrap();
        assert_eq!(
            integers.to_string(),
            "('True, 'True, 'True, 'True, 'True, 'True)"
        );

        let floats = run("(1.0 == 1.0, 1.0 != 2.0, 1.0 < 2.0, 2.0 > 1.0, \
             1.0 <= 1.0, 2.0 >= 2.0, -0.0 == 0.0)")
        .unwrap();
        assert_eq!(
            floats.to_string(),
            "('True, 'True, 'True, 'True, 'True, 'True, 'True)"
        );
    }

    #[test]
    fn non_finite_float_arithmetic_raises_sourced_blame() {
        let sources = [
            "0.0 / 0.0".to_owned(),
            "1.0 / 0.0".to_owned(),
            "-1.0 / 0.0".to_owned(),
            "1e308 * 2.0".to_owned(),
            "1e308 + 1e308".to_owned(),
            "-1e308 - 1e308".to_owned(),
        ];
        for source in sources {
            let error = run(&source).unwrap_err();
            let ExecutionError::Runtime(error) = error else {
                panic!("expected runtime blame for {source}")
            };
            assert_eq!(error.kind, RuntimeErrorKind::RaisedBlame, "{source}");
            assert_eq!(error.message, "NonFiniteFloat", "{source}");
            assert!(error.data_location().is_some(), "{source}");
            assert!(error.rule_location().is_some(), "{source}");
            assert_eq!(error.origin(), error.rule_location().map(Origin::Source));
        }
    }

    #[test]
    fn compares_strings_by_internal_utf8_byte_sequence() {
        let value = run(r#"("app" < "apple", "10" < "2", "Z" < "a", "é" < "中",
                "same" <= "same", "z" > "a", "z" >= "z",
                "a deliberately heap-backed string" > "a")"#)
        .unwrap();
        assert_eq!(
            value.to_string(),
            "('True, 'True, 'True, 'True, 'True, 'True, 'True, 'True)"
        );
    }

    #[test]
    fn inequality_preserves_structural_equality_semantics() {
        let value = run("(('Ok, [1, 2]) != ('Ok, [1, 2]), ('Ok, [1]) != ('Err, [1]))").unwrap();
        assert_eq!(value.to_string(), "('False, 'True)");
    }

    #[test]
    fn dynamic_ordered_comparison_rejects_mismatched_domains() {
        let error = run("let left: Any = \"a\"; let right: Any = 1; left < right").unwrap_err();
        let ExecutionError::Runtime(error) = error else {
            panic!("expected runtime error")
        };
        assert_eq!(error.kind, RuntimeErrorKind::TypeMismatch);
        assert!(error.message.contains("matching Int, Float, or String"));
    }

    #[test]
    fn executes_single_assignment_recursive_definitions() {
        let explicit = run(
            "decl countdown: Fn(Int) -> Int; def countdown = fn(n) { if n < 1 { 0 } else { countdown(n - 1) } }; countdown(4)",
        )
        .unwrap();
        assert!(matches!(explicit, Value::Int(0)));

        let mutual = run(
            "decl even: Fn(Int) -> Int; decl odd: Fn(Int) -> Int; def even = fn(n) { if n < 1 { 0 } else { odd(n - 1) } }; def odd = fn(n) { if n < 1 { 1 } else { even(n - 1) } }; even(4)",
        )
        .unwrap();
        assert!(matches!(mutual, Value::Int(0)));

        let higher_order = run(
            "decl loop: Fn(Int) -> Int; let build = fn(body) { body }; def loop = build(fn(n) { if n < 1 { 0 } else { loop(n - 1) } }); loop(3)",
        )
        .unwrap();
        assert!(matches!(higher_order, Value::Int(0)));

        let passed_as_value = run(
            "decl countdown: Fn(Int) -> Int; def countdown = fn(n) { if n < 1 { 0 } else { countdown(n - 1) } }; let invoke = fn(f, n) { f(n) }; invoke(countdown, 4)",
        )
        .unwrap();
        assert!(matches!(passed_as_value, Value::Int(0)));

        let named = run(
            "def loop: Fn(Int) -> Int = fn(n) { if n < 1 { 0 } else { loop(n - 1) } }; loop(3)",
        )
        .unwrap();
        assert!(matches!(named, Value::Int(0)));

        let annotated =
            run("def increment: Fn(Int) -> Int = fn(value) { value + 1 }; increment(41)").unwrap();
        assert!(matches!(annotated, Value::Int(42)));
    }

    #[test]
    fn executes_inferred_direct_and_mutual_recursive_definitions() {
        let direct = run("def countdown = fn(value) {\
                 if value < 1 { 0 } else { countdown(value - 1) }\
             }; countdown(4)")
        .unwrap();
        assert!(matches!(direct, Value::Int(0)));

        let mutual = run("def even = fn(value) {\
                 if value < 1 { 'True } else { odd(value - 1) }\
             };\
             def odd = fn(value) {\
                 if value < 1 { 'False } else { even(value - 1) }\
             }; even(4)")
        .unwrap();
        assert!(matches!(
            mutual,
            Value::Atom(Atom::Builtin(BuiltinAtom::True))
        ));
    }

    #[test]
    fn proper_tail_calls_cross_recursive_branches_and_match_arms() {
        let direct =
            run("def countdown: Fn(Int) -> Int = fn(n) { if n < 1 { 0 } else { countdown(n - 1) } }; countdown(1500)")
                .unwrap();
        assert!(matches!(direct, Value::Int(0)));

        let mutual = run(
            "decl even: Fn(Int) -> Int; decl odd: Fn(Int) -> Int; def even = fn(n) { if n < 1 { 0 } else { odd(n - 1) } }; def odd = fn(n) { if n < 1 { 1 } else { even(n - 1) } }; even(1500)",
        )
        .unwrap();
        assert!(matches!(mutual, Value::Int(0)));

        let matched = run(
            "def countdown: Fn(Int) -> Int = fn(n) { match n { 0 => 0, value => countdown(value - 1) } }; countdown(1500)",
        )
        .unwrap();
        assert!(matches!(matched, Value::Int(0)));

        let higher_order = run(
            "let iterate: Fn(Any, Int) -> Int = fn(step, n) { if n < 1 { 0 } else { step(step, n - 1) } }; iterate(iterate, 1500)",
        )
        .unwrap();
        assert!(matches!(higher_order, Value::Int(0)));

        let non_tail =
            run("def descend: Fn(Int) -> Int = fn(n) { if n < 1 { 0 } else { 1 + descend(n - 1) } }; descend(1500)")
                .unwrap_err();
        assert!(matches!(
            non_tail,
            ExecutionError::Runtime(RuntimeError {
                kind: RuntimeErrorKind::CallDepthExceeded,
                ..
            })
        ));
    }

    #[test]
    fn emits_contiguous_call_windows_and_structural_tail_calls() {
        let tail = compile_source("test", "let id = fn(x) { x }; id(1)").unwrap();
        assert!(matches!(
            tail.instructions().last(),
            Some(crate::Opcode::TailCall {
                argument_count: 1,
                ..
            })
        ));

        let non_tail =
            compile_source("test", "let id = fn(x) { x }; let value = id(1); value").unwrap();
        assert!(non_tail.instructions().iter().any(|instruction| matches!(
            instruction,
            crate::Opcode::Call {
                argument_count: 1,
                ..
            }
        )));
        assert!(matches!(
            non_tail.instructions().last(),
            Some(crate::Opcode::Return { .. })
        ));

        let branches = compile_source(
            "test",
            "let id = fn(x) { x }; if 'True { id(1) } else { id(2) }",
        )
        .unwrap();
        assert_eq!(
            branches
                .instructions()
                .iter()
                .filter(|instruction| matches!(instruction, crate::Opcode::TailCall { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn definition_contract_failures_keep_source_origins() {
        let missing = run("decl missing: Fn(Int) -> Int; 0").unwrap_err();
        assert!(
            missing
                .to_string()
                .contains("declared but never initialized")
        );

        let early_read = run("decl value: Int; def value = value + 1; value").unwrap_err();
        assert!(matches!(
            early_read,
            ExecutionError::Runtime(RuntimeError {
                kind: RuntimeErrorKind::UninitializedDefinition,
                ..
            })
        ));

        let shadow = run("def value = 1; { def value = 2; value }").unwrap_err();
        assert!(shadow.to_string().contains("cannot shadow"));

        let let_shadow = run("let value = 1; def value = 2; value").unwrap_err();
        assert!(let_shadow.to_string().contains("cannot shadow"));

        let declaration_conflict =
            run("decl value: Int; let value = 1; def value = 2; value").unwrap_err();
        assert!(declaration_conflict.to_string().contains("conflicts"));

        let wrong_arity = run(
            "decl f: Fn(Int) -> Int; let build = fn(value) { value }; def f = build(fn(a, b) { a + b }); f",
        )
        .unwrap_err();
        let ExecutionError::Frontend(wrong_arity) = wrong_arity else {
            panic!("expected strict contract error");
        };
        assert!(
            wrong_arity
                .message
                .contains("cannot unify Fn(Any, Any) -> Any with Fn(Int) -> Int")
        );
        assert_eq!(wrong_arity.location.line, 1);
        assert_eq!(wrong_arity.location.column, 72);
    }

    #[test]
    fn allocation_and_stack_quotas_keep_source_origins() {
        let source = "[1, 2]";
        let function = compile_source("quota.telora", source).unwrap();
        let mut sources = SourceDatabase::default();
        sources.add("quota.telora", source);
        let allocation = Vm::new()
            .execute_with_quota(&function, Quota::new(0, 100, 0))
            .unwrap_err()
            .with_sources(&sources);
        assert_eq!(allocation.kind, RuntimeErrorKind::AllocationQuotaExceeded);
        assert!(allocation.to_string().contains("quota.telora:1:1"));

        let stack = Vm::new()
            .execute_with_quota(&function, Quota::new(0, 1, u64::MAX))
            .unwrap_err()
            .with_sources(&sources);
        assert_eq!(stack.kind, RuntimeErrorKind::StackLimitExceeded);
        assert!(stack.to_string().contains("quota.telora:1:"));

        let native_source = "validate(Int, \"wrong\")";
        let native = compile_source("native-quota.telora", native_source).unwrap();
        let native_error = Vm::new()
            .execute_with_quota(&native, Quota::new(1, 100, 0))
            .unwrap_err();
        assert_eq!(native_error.kind, RuntimeErrorKind::AllocationQuotaExceeded);
    }

    #[test]
    fn captures_values_and_calls_closures() {
        let value = run("let base = 40; let add = fn(value) { base + value }; add(2)").unwrap();
        assert!(matches!(value, Value::Int(42)));
    }

    #[test]
    fn executes_partially_annotated_closures_without_runtime_annotation_work() {
        let value = run("(fn(value: Int) -> Int { value + 1 })(41)").unwrap();
        assert!(matches!(value, Value::Int(42)));
    }

    #[test]
    fn erases_explicit_type_application_from_runtime_calls() {
        let value = run("decl identity: for(A) Fn(A) -> A;\
             def identity = fn(value) { value };\
             identity@[Int](42)")
        .unwrap();
        assert!(matches!(value, Value::Int(42)));
    }

    #[test]
    fn executes_inferred_generic_closures_without_runtime_instances() {
        let value = run("let identity = fn(value) { value };\
             (identity(42), identity(\"value\"), identity@[Int](7))")
        .unwrap();
        assert!(matches!(
            value,
            Value::Tuple(items)
                if matches!(items[0], Value::Int(42))
                    && matches!(&items[1], Value::String(text) if text.as_ref() == "value")
                    && matches!(items[2], Value::Int(7))
        ));
    }

    #[test]
    fn indexes_arrays_and_projects_tuples() {
        let value = run("let values = [10, 20, 30]; (values[1], (\"left\", 42).1)").unwrap();
        assert!(matches!(
            value,
            Value::Tuple(items)
                if matches!(items[0], Value::Int(20))
                    && matches!(items[1], Value::Int(42))
        ));
    }

    #[test]
    fn array_index_out_of_range_raises_sourced_blame() {
        for source in ["[1][-1]", "[1][1]", "[1][2]"] {
            let ExecutionError::Runtime(error) = run(source).unwrap_err() else {
                panic!("expected runtime failure for {source}");
            };
            assert_eq!(error.kind, RuntimeErrorKind::RaisedBlame);
            assert_eq!(error.message, "OutOfRange");
            assert!(error.rule_location().is_some());
        }

        let function = compile_source("test", "[1][1]").unwrap();
        let array_only = std::mem::size_of::<Value>() as u64;
        let allocation = Vm::new()
            .execute_with_quota(&function, Quota::new(0, 100, array_only))
            .unwrap_err();
        assert_eq!(allocation.kind, RuntimeErrorKind::AllocationQuotaExceeded);

        let complete_failure = array_only
            .checked_mul(6)
            .and_then(|bytes| bytes.checked_add(15))
            .unwrap();
        let blame = Vm::new()
            .execute_with_quota(&function, Quota::new(0, 100, complete_failure))
            .unwrap_err();
        assert_eq!(blame.kind, RuntimeErrorKind::RaisedBlame);
    }

    #[test]
    fn dynamic_projection_boundaries_check_runtime_values() {
        assert!(matches!(
            run("let pair = (0, (1, \"ok\")); pair.1.0").unwrap(),
            Value::Int(1)
        ));
        assert!(matches!(
            run("let values: Any = [1, 2]; values[1]").unwrap(),
            Value::Int(2)
        ));
        assert!(matches!(
            run("let pair: Any = (1, \"x\"); pair.1").unwrap(),
            Value::String(text) if text.as_ref() == "x"
        ));

        for source in [
            "let value: Any = 1; value[0]",
            "let values: Any = [1]; let index: Any = \"x\"; values[index]",
            "let value: Any = 1; value.0",
        ] {
            let ExecutionError::Runtime(error) = run(source).unwrap_err() else {
                panic!("expected runtime type mismatch for {source}");
            };
            assert_eq!(error.kind, RuntimeErrorKind::TypeMismatch);
        }

        let ExecutionError::Runtime(error) = run("let pair: Any = (1, 2); pair.2").unwrap_err()
        else {
            panic!("expected dynamic tuple bounds failure");
        };
        assert_eq!(error.kind, RuntimeErrorKind::RaisedBlame);
        assert_eq!(error.message, "OutOfRange");
    }

    #[test]
    fn projection_types_are_checked_statically() {
        let tuple_bounds = compile_source("test", "(1, \"x\").2").unwrap_err();
        assert!(tuple_bounds.message.contains("has no item at index 2"));

        let old_type_application = compile_source(
            "test",
            "decl identity: for(A) Fn(A) -> A; def identity = fn(value) { value }; identity[Int](1)",
        )
        .unwrap_err();
        assert!(
            old_type_application.message.contains("cannot index value"),
            "{}",
            old_type_application.message
        );
    }

    #[test]
    fn pipeline_is_uniform_reverse_application() {
        let value = run("let add = fn(a) { fn(b) { a + b } }; 40 |> add(2)").unwrap();
        assert!(matches!(value, Value::Int(42)));

        let chained = run("let ops = { increment: fn(value) { value + 1 } }; \
             40 |> ops.increment |> fn(value) { value + 1 }")
        .unwrap();
        assert!(matches!(chained, Value::Int(42)));

        let error = run("let add = fn(a, b) { a + b }; 40 |> add(2)").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("call expects 2 arguments, found 1")
        );
    }

    #[test]
    fn call_sections_elaborate_to_ordinary_closures() {
        let bare = run("let combine = fn(a, middle, b) { a + middle + b }; \
             let section = combine\\(_, 10, _); section(1, 2)")
        .unwrap();
        assert!(matches!(bare, Value::Int(13)));

        let reordered = run("let subtract = fn(a, b) { a - b }; \
             let flipped = subtract\\(_1, _0); flipped(2, 10)")
        .unwrap();
        assert!(matches!(reordered, Value::Int(8)));

        let repeated =
            run("let add = fn(a, b) { a + b }; let twice = add\\(_0, _0); twice(21)").unwrap();
        assert!(matches!(repeated, Value::Int(42)));

        let nested = run("let increment = fn(value) { value + 1 }; \
             let apply = fn(callback, value) { callback(value) }; \
             apply(increment\\(_), 41)")
        .unwrap();
        assert!(matches!(nested, Value::Int(42)));

        let piped = run("let add = fn(a, b) { a + b }; \
             40 |> add\\(_, 2)")
        .unwrap();
        assert!(matches!(piped, Value::Int(42)));

        let native = run("let array_type = Array\\(_); array_type(Int)").unwrap();
        let Value::Dict(metadata) = native else {
            panic!("expected Array metadata")
        };
        assert_eq!(metadata.get("kind").unwrap().to_string(), "'Array");

        let reevaluated = run("let second = fn(first, second) { second }; \
             let make: Fn() -> Fn(Any) -> Any = fn() { fn(value) { value } }; \
             let section = second\\(_, make()); \
             section(1) == section(2)")
        .unwrap();
        assert!(matches!(
            reevaluated,
            Value::Atom(Atom::Builtin(BuiltinAtom::False))
        ));
    }

    #[test]
    fn interpolates_strings_ints_and_atoms() {
        let value = run(
            r#"let name = "Ada"; let count = 3; let state = 'Ok; `hi, \{name} count=\{count} state=\{state}`"#,
        )
        .unwrap();
        assert!(
            matches!(&value, Value::String(text) if text.as_ref() == "hi, Ada count=3 state=Ok")
        );

        let nested = run(r#"`value=\{if 'True { "yes" } else { "no" }}`"#).unwrap();
        assert!(matches!(&nested, Value::String(text) if text.as_ref() == "value=yes"));
    }

    #[test]
    fn evaluates_escaped_raw_and_continued_strings() {
        let value = run(r####"("A=\x41, shape=\u{5f62}, first \
                second", r##"raw \n "quote" and #"##)"####)
        .unwrap();
        assert_eq!(
            value.to_string(),
            "(\"A=A, shape=形, first second\", \"raw \\\\n \\\"quote\\\" and #\")"
        );
    }

    #[test]
    fn checks_known_and_dynamic_unsupported_interpolation_values() {
        let static_error = run(r#"`items=\{[1, 2]}`"#).unwrap_err();
        assert!(
            static_error
                .to_string()
                .contains("does not support Array<Int>")
        );

        let dynamic_error = run(r#"def render = fn(x) { `x=\{x}` }; render([1])"#).unwrap_err();
        assert!(matches!(
            dynamic_error,
            ExecutionError::Runtime(RuntimeError {
                kind: RuntimeErrorKind::TypeMismatch,
                ..
            })
        ));
    }

    #[test]
    fn if_evaluates_only_the_selected_branch() {
        let value = run("if 'True { 42 } else { 1 / 0 }").unwrap();
        assert!(matches!(value, Value::Int(42)));
    }

    #[test]
    fn control_flow_else_chains_evaluate_like_nested_expressions() {
        let cases = [
            "if 'False { 1 } else if 'False { 2 } else if 'True { 3 } else { 4 }",
            "if 'False { 1 } else if let 'Some(value) = 'Some(3) { value } else { 4 }",
            "let choose = fn(value: Bool) { if 'False { 1 } else match value { 'True => 3, 'False => 4 } }; choose('True)",
        ];
        for source in cases {
            assert!(matches!(run(source).unwrap(), Value::Int(3)));
        }

        let returned = run(
            "let choose = fn(condition: Bool) { if condition { 3 } else return 4; }; (choose('True), choose('False))",
        )
        .unwrap();
        assert_eq!(returned.to_string(), "(3, 4)");
    }

    #[test]
    fn match_destructures_tagged_tuples() {
        let value = run("match ('Ok, 42) { ('Ok, value) => value }").unwrap();
        assert!(matches!(value, Value::Int(42)));
    }

    #[test]
    fn atom_call_constructs_tagged_value_and_pattern_destructures_it() {
        let value = run("let Some = 'Some; let option: Option(Int) = Some(42);\
             match option { 'None => 0, 'Some(value) => value }")
        .unwrap();
        assert!(matches!(value, Value::Int(42)));
    }

    #[test]
    fn struct_patterns_select_nested_fields_and_fall_through_dynamically() {
        let selected = run("let user = {name: \"Ada\", address: {city: \"London\"}};\
             match user { {name, address: {city}} => (name, city) }")
        .unwrap();
        assert_eq!(selected.to_string(), "(\"Ada\", \"London\")");

        let fallback = run("let select: Fn(Any) -> String = fn(value) {\
                match value { {name} => name, _ => \"fallback\" }\
             }; select(1)")
        .unwrap();
        assert_eq!(fallback.to_string(), "\"fallback\"");

        let empty = run("let is_struct: Fn(Any) -> Bool = fn(value) {\
                match value { {} => 'True, _ => 'False }\
             }; (is_struct({}), is_struct(1))")
        .unwrap();
        assert_eq!(empty.to_string(), "('True, 'False)");
    }

    #[test]
    fn local_destructuring_let_preserves_order_scope_and_nested_selection() {
        let value = run("let outer = \"outer\"; {
            let (left, user) = (1, {name: \"Ada\", address: {city: \"London\"}});
            let {name, address: {city}} = user;
            let outer = name;
            (left, outer, city)
        }")
        .unwrap();
        assert_eq!(value.to_string(), "(1, \"Ada\", \"London\")");
    }

    #[test]
    fn propagates_option_and_result_from_the_nearest_function() {
        let option = run("let step: Fn(Option(Int)) -> Option(Int) = fn(value) { let item = value?; 'Some(item + 1) }; (step('Some(2)), step('None))").unwrap();
        assert_eq!(option.to_string(), "('Some(3), 'None)");

        let result = run("let step: Fn(Result(Int, String)) -> Result(Int, String) = fn(value) { let item = value?; 'Ok(item + 1) }; (step('Ok(2)), step('Err(\"bad\")))").unwrap();
        assert_eq!(result.to_string(), "('Ok(3), 'Err(\"bad\"))");
    }

    #[test]
    fn infers_propagation_boundary_from_success_constructor() {
        let value = run("let step = fn(value: Option(Int)) { let item = { value? }; 'Some(item + 1) }; (step('Some(1)), step('None))").unwrap();
        assert_eq!(value.to_string(), "('Some(2), 'None)");
    }

    #[test]
    fn propagates_from_module_blocks_and_isolates_nested_functions() {
        let module =
            run("{ let value: Option(Int) = 'None; let item = value?; 'Some(item) }").unwrap();
        assert_eq!(module.to_string(), "'None");

        let nested = run("let outer = fn(value: Option(Int)) { let inner: Fn(Option(Int)) -> Option(Int) = fn(inner_value) { let item = inner_value?; 'Some(item + 1) }; 'Some(inner(value)) }; outer('None)").unwrap();
        assert_eq!(nested.to_string(), "'Some('None)");
    }

    #[test]
    fn rejects_mixed_and_unsupported_propagation() {
        let mixed = compile_source("test", "let f = fn(a: Option(Int), b: Result(Int, String)) { let x = a?; let y = b?; 'Some(x + y) }; f").unwrap_err();
        assert!(
            mixed.message.contains("cannot mix Option and Result"),
            "{}",
            mixed.message
        );

        let unsupported =
            compile_source("test", "let f = fn(value: Bool) { value? }; f").unwrap_err();
        assert!(
            unsupported
                .message
                .contains("Option-shaped or Result-shaped"),
            "{}",
            unsupported.message
        );
    }

    #[test]
    fn returns_values_from_the_nearest_function() {
        let value = run("let choose = fn(condition: Bool, value: Int, fallback: Int) { if condition { return value; } else { fallback } }; (choose('True, 1, 2), choose('False, 1, 2))").unwrap();
        assert_eq!(value.to_string(), "(1, 2)");

        let nested =
            run("let outer = fn() { let inner = fn() { return 1; }; inner() + 1 }; outer()")
                .unwrap();
        assert_eq!(nested.to_string(), "2");
    }

    #[test]
    fn rejects_return_outside_functions_and_wrong_result_types() {
        let module = compile_source("test", "return 1;").unwrap_err();
        assert!(module.message.contains("only inside a Function"));

        let wrong = compile_source("test", "let f: Fn(Bool) -> Int = fn(condition) { if condition { return \"wrong\"; } else { 1 } }; f").unwrap_err();
        assert!(wrong.message.contains("String") && wrong.message.contains("Int"));
    }

    #[test]
    fn panic_is_a_sourced_never_expression() {
        let value = run("if 'False { panic!(\"unused\") } else { 3 }").unwrap();
        assert_eq!(value.to_string(), "3");

        let error = run("let fail = fn() {\n  panic!(\"broken\")\n};\nfail()").unwrap_err();
        let ExecutionError::Runtime(error) = error else {
            panic!("expected runtime panic")
        };
        assert_eq!(error.kind, RuntimeErrorKind::Panic);
        assert_eq!(error.message, "broken");
        assert!(error.to_string().contains("test:2:3"));
    }

    #[test]
    fn panic_requires_one_string_message() {
        let wrong_type = compile_source("test", "panic!(1)").unwrap_err();
        assert!(wrong_type.message.contains("Int") && wrong_type.message.contains("String"));

        let wrong_arity = compile_source("test", "panic!()").unwrap_err();
        assert!(wrong_arity.message.contains("exactly one argument"));
    }

    #[test]
    fn raise_preserves_structured_blame_locations() {
        let error =
            run("let fail = fn() {\n  let data = 1;\n  raise!(blame!(\"bad\", data))\n};\nfail()")
                .unwrap_err();
        let ExecutionError::Runtime(error) = error else {
            panic!("expected raised blame")
        };
        assert_eq!(error.kind, RuntimeErrorKind::RaisedBlame);
        assert_eq!(error.message, "bad");
        assert!(error.data_location().is_some());
        assert!(error.rule_location().is_some());
        assert_ne!(error.data_location(), error.rule_location());
        assert!(error.trace.iter().any(|frame| frame.origin.is_some()));
    }

    #[test]
    fn raise_requires_one_blame_error() {
        let wrong_type = compile_source("test", "raise!(1)").unwrap_err();
        assert!(
            wrong_type.message.contains("Int") && wrong_type.message.contains("message"),
            "{}",
            wrong_type.message
        );

        let wrong_arity = compile_source("test", "raise!()").unwrap_err();
        assert!(wrong_arity.message.contains("exactly one argument"));
    }

    #[test]
    fn blame_accepts_heterogeneous_variadic_subjects() {
        let error = run(
            "let error: BlameError = blame!(\"different subjects\", 1, \"two\", 'Three); raise!(error)",
        )
        .unwrap_err();
        let ExecutionError::Runtime(error) = error else {
            panic!("expected raised blame")
        };
        assert_eq!(error.kind, RuntimeErrorKind::RaisedBlame);
        assert_eq!(error.message, "different subjects");
    }

    #[test]
    fn report_records_a_diagnostic_and_returns_the_error() {
        let function = compile_source(
            "test",
            "let error = report('Warn, blame!(\"warning\", 1, \"two\")); error.message",
        )
        .unwrap();
        let mut account = crate::QuotaAccount::new(crate::Quota::with_fuel(100_000));
        let value = Vm::new()
            .execute_with_account(&function, &[], &mut account)
            .unwrap();
        assert_eq!(value.to_string(), "\"warning\"");
        assert_eq!(account.diagnostics().len(), 1);
        assert_eq!(
            account.diagnostics()[0].severity,
            crate::source::Severity::Warning
        );
        assert_eq!(account.diagnostics()[0].labels.len(), 3);
    }

    #[test]
    fn diagnostic_convenience_intrinsics_compose_blame_report_and_raise() {
        let function = compile_source(
            "test",
            "let warning: BlameError = emit_warn!(\"deprecated\", \"old\", 42); warning.message",
        )
        .unwrap();
        let mut account = crate::QuotaAccount::new(crate::Quota::with_fuel(100_000));
        let value = Vm::new()
            .execute_with_account(&function, &[], &mut account)
            .unwrap();
        assert_eq!(value.to_string(), "\"deprecated\"");
        assert_eq!(account.diagnostics().len(), 1);
        assert_eq!(
            account.diagnostics()[0].severity,
            crate::source::Severity::Warning
        );
        assert_eq!(account.diagnostics()[0].labels.len(), 3);

        let error = run("let ignored = emit_error!(\"invalid\", 42); 7").unwrap_err();
        let ExecutionError::Runtime(error) = error else {
            panic!("expected a reported diagnostic")
        };
        assert_eq!(error.kind, RuntimeErrorKind::ReportedDiagnostic);
        assert_eq!(error.message, "invalid");

        let error = run("fail!(\"stopped\", 42)").unwrap_err();
        let ExecutionError::Runtime(error) = error else {
            panic!("expected raised blame")
        };
        assert_eq!(error.kind, RuntimeErrorKind::RaisedBlame);
        assert_eq!(error.message, "stopped");
    }

    #[test]
    fn diagnostic_convenience_intrinsics_require_string_messages() {
        for source in [
            "emit_info!(1)",
            "emit_warn!(1)",
            "emit_error!(1)",
            "fail!(1)",
        ] {
            let error = compile_source("test", source).unwrap_err();
            assert!(
                error.message.contains("Int") && error.message.contains("String"),
                "{source}: {}",
                error.message
            );
        }
    }

    #[test]
    fn if_let_selects_and_scopes_structural_patterns() {
        let some = run(
            "let value: Option(Int) = 'Some(3); if let 'Some(item) = value { item + 1 } else { 0 }",
        )
        .unwrap();
        assert_eq!(some.to_string(), "4");

        let none = run(
            "let value: Option(Int) = 'None; if let 'Some(item) = value { item + 1 } else { 0 }",
        )
        .unwrap();
        assert_eq!(none.to_string(), "0");

        let error =
            compile_source("test", "if let 'Some(item) = 1 { item } else { 0 }").unwrap_err();
        assert!(error.message.contains("pattern cannot match Int"));
    }

    #[test]
    fn let_else_binds_the_remaining_block_and_requires_divergence() {
        let value = run("let step: Fn(Option(Int)) -> Option(Int) = fn(option) { let 'Some(item) = option else { return 'None; }; 'Some(item + 1) }; (step('Some(2)), step('None))").unwrap();
        assert_eq!(value.to_string(), "('Some(3), 'None)");

        let panic = run("let require = fn(option: Option(Int)) { let 'Some(item) = option else { panic!(\"none\") }; item }; require('Some(4))").unwrap();
        assert_eq!(panic.to_string(), "4");

        let non_never = compile_source(
            "test",
            "let f = fn(option: Option(Int)) { let 'Some(item) = option else { 0 }; item }; f",
        )
        .unwrap_err();
        assert!(non_never.message.contains("must have type Never"));

        let irrefutable = compile_source("test", "@struct type Pair = {a: Int, b: Int}; let f = fn(pair: Pair) { let {a, b} = pair else { panic!(\"never\") }; a + b }; f").unwrap_err();
        assert!(
            irrefutable.message.contains("irrefutable"),
            "{}",
            irrefutable.message
        );
    }

    #[test]
    fn boolean_operators_short_circuit_and_preserve_precedence() {
        let value =
            run("('False && (1 / 0 == 0), 'True || (1 / 0 == 0), 'False || 'True && 'True)")
                .unwrap();
        assert_eq!(value.to_string(), "('False, 'True, 'True)");

        let error = compile_source("test", "'True && 1").unwrap_err();
        assert!(error.message.contains("Int"), "{}", error.message);
    }

    #[test]
    fn logical_negation_executes_with_unary_precedence_and_dynamic_checks() {
        let value =
            run("(!'True, !'False, !!'True, !('True && 'False), !'False == 'True, !0, !-1)")
                .unwrap();
        assert_eq!(
            value.to_string(),
            "('False, 'True, 'True, 'True, 'True, -1, 0)"
        );

        let dynamic = run("let invert: Fn(Any) -> Bool = fn(value) { !value };\
             (invert('True), invert('False))")
        .unwrap();
        assert_eq!(dynamic.to_string(), "('False, 'True)");

        let ExecutionError::Runtime(error) =
            run("let invert: Fn(Any) -> Bool = fn(value) { !value }; invert(1)").unwrap_err()
        else {
            panic!("expected runtime Bool check");
        };
        assert_eq!(error.kind, RuntimeErrorKind::TypeMismatch);

        let dynamic = run("let invert: Fn(Any) -> Any = fn(value) { !value };\
             (invert('True), invert(0))")
        .unwrap();
        assert_eq!(dynamic.to_string(), "('False, -1)");

        let ExecutionError::Runtime(error) =
            run("let invert: Fn(Any) -> Int = fn(value) { !value }; invert('True)").unwrap_err()
        else {
            panic!("expected runtime Int check");
        };
        assert_eq!(error.kind, RuntimeErrorKind::TypeMismatch);
    }

    #[test]
    fn bitwise_integer_operators_execute_with_stable_precedence() {
        let value = run("(6 & 3, 4 | 1, 6 ^ 3, 1 | 2 ^ 3 & 1, 6 & 3 == 2)").unwrap();
        assert_eq!(value.to_string(), "(2, 5, 5, 3, 'True)");

        for source in ["1 & 1.0", "1 | 'True", "\"x\" ^ 1"] {
            let error = compile_source("test", source).unwrap_err();
            assert!(error.message.contains("Int"), "{}", error.message);
        }

        let ExecutionError::Runtime(error) =
            run("let bit_and: Fn(Any, Any) -> Int = fn(left, right) { left & right }; bit_and(1, \"x\")")
                .unwrap_err()
        else {
            panic!("expected runtime Int check");
        };
        assert_eq!(error.kind, RuntimeErrorKind::TypeMismatch);
    }

    #[test]
    fn remainder_supports_int_float_precedence_and_dynamic_boundaries() {
        let value = run("(7 % 3, -7 % 3, 7 % -3, -7 % -3, \
             5.5 % 2.0, -5.5 % 2.0, 5.5 % -2.0, \
             2 + 7 % 4 * 3, 20 / 3 % 2, 20 % 6 * 2)")
        .unwrap();
        assert_eq!(
            value.to_string(),
            "(1, -1, 1, -1, 1.5, -1.5, 1.5, 11, 0, 4)"
        );

        let dynamic =
            run("let rem: Fn(Any, Any) -> Int = fn(left, right) { left % right }; rem(7, 3)")
                .unwrap();
        assert_eq!(dynamic.to_string(), "1");

        for source in ["1 % 1.0", "\"x\" % 1"] {
            let error = compile_source("test", source).unwrap_err();
            assert!(
                error.message.contains("Int or Float") || error.message.contains("cannot unify"),
                "{source}: {}",
                error.message
            );
        }

        let ExecutionError::Runtime(error) =
            run("let rem: Fn(Any, Any) -> Int = fn(left, right) { left % right }; rem(7, \"x\")")
                .unwrap_err()
        else {
            panic!("expected runtime numeric type error")
        };
        assert_eq!(error.kind, RuntimeErrorKind::TypeMismatch);
    }

    #[test]
    fn remainder_uses_existing_numeric_failure_paths() {
        let ExecutionError::Runtime(error) = run("7 % 0").unwrap_err() else {
            panic!("expected Int zero-divisor failure")
        };
        assert_eq!(error.kind, RuntimeErrorKind::DivisionByZero);
        assert_eq!(error.message, "integer remainder by zero");
        assert!(error.origin().is_some());

        let ExecutionError::Runtime(error) = run("(-9223372036854775807 - 1) % -1").unwrap_err()
        else {
            panic!("expected Int remainder overflow")
        };
        assert_eq!(error.kind, RuntimeErrorKind::IntegerOverflow);

        for source in ["7.0 % 0.0", "7.0 % -0.0"] {
            let ExecutionError::Runtime(error) = run(source).unwrap_err() else {
                panic!("expected Float non-finite failure")
            };
            assert_eq!(error.kind, RuntimeErrorKind::RaisedBlame);
            assert_eq!(error.message, "NonFiniteFloat");
            assert!(error.data_location().is_some());
            assert!(error.rule_location().is_some());
            assert_eq!(error.origin(), error.rule_location().map(Origin::Source));
        }
    }

    #[test]
    fn match_guards_use_pattern_bindings_and_continue_after_false() {
        let value = run("let value: Option(Int) = 'Some(3); match value {\
                'Some(item) if 4 < item => 40,\
                'Some(item) if 2 < item && item < 4 => item,\
                'Some(_) => 0,\
                'None => -1,\
            }")
        .unwrap();
        assert_eq!(value.to_string(), "3");
    }

    #[test]
    fn match_guards_require_bool() {
        let error = compile_source(
            "test",
            "let value: Option(Int) = 'Some(1); match value {\
                'Some(item) if item => item,\
                _ => 0,\
            }",
        )
        .unwrap_err();
        assert!(
            error.message.contains("Int")
                && error.message.contains("False")
                && error.message.contains("True"),
            "{}",
            error.message
        );
    }

    #[test]
    fn guarded_match_arms_do_not_establish_exhaustiveness() {
        let error = compile_source(
            "test",
            "let value: Option(Int) = 'Some(1); match value {\
                'Some(item) if 'True => item,\
                'None if 'True => 0,\
            }",
        )
        .unwrap_err();
        assert!(
            error.message.contains("non-exhaustive match"),
            "{}",
            error.message
        );
    }

    #[test]
    fn match_guard_redundancy_depends_only_on_unguarded_coverage() {
        compile_source(
            "test",
            "let value: Option(Int) = 'Some(1); match value {\
                'Some(item) if 0 < item => item,\
                'Some(item) => item,\
                'None => 0,\
            }",
        )
        .unwrap();

        let error = compile_source(
            "test",
            "let value: Option(Int) = 'Some(1); match value {\
                'Some(item) => item,\
                'Some(item) if 0 < item => item,\
                'None => 0,\
            }",
        )
        .unwrap_err();
        assert!(
            error.message.contains("unreachable match arm"),
            "{}",
            error.message
        );
    }

    #[test]
    fn array_spread_flattens_fragments_in_source_order() {
        let value =
            run("let middle = [1, 2]; let empty: Array(Int) = []; [0, ...middle, 3, ...empty, 4]")
                .unwrap();
        assert_eq!(value.to_string(), "[0, 1, 2, 3, 4]");

        let nested = run("let values = [[1], [2, 3]]; [...values]").unwrap();
        assert_eq!(nested.to_string(), "[[1], [2, 3]]");
    }

    #[test]
    fn array_spread_requires_an_array_operand() {
        let error = compile_source("test", "[0, ...1]").unwrap_err();
        assert!(
            error.message.contains("array spread requires Array") && error.message.contains("Int"),
            "{}",
            error.message
        );
    }

    #[test]
    fn dict_spread_merges_in_source_order_with_later_values_winning() {
        let value = run("let base: Dict(Int) = {a: 1, b: 2};\
             let extra: Dict(Int) = {b: 3, c: 4};\
             {...base, x: 0, ...extra, c: 5}")
        .unwrap();
        assert_eq!(value.to_string(), "{a: 1, b: 3, c: 5, x: 0}");

        let contextual = run("let value: Dict(Int) = {...{a: 1}, b: 2}; value").unwrap();
        assert_eq!(contextual.to_string(), "{a: 1, b: 2}");
    }

    #[test]
    fn dict_field_shorthand_lowers_to_an_ordinary_named_field() {
        let value = run("let name = \"telora\"; let version = 1; { name, version }").unwrap();
        assert_eq!(value.to_string(), "{name: \"telora\", version: 1}");

        let mixed = run("let name = 1; let extra: Dict(Int) = {version: 2};\
             {name, explicit: 3, ...extra}")
        .unwrap();
        assert_eq!(mixed.to_string(), "{explicit: 3, name: 1, version: 2}");

        let duplicate = compile_source("test", "let name = 1; {name, name: 2}").unwrap_err();
        assert!(duplicate.message.contains("duplicate Dict field"));

        let unknown = compile_source("test", "{missing}").unwrap_err();
        assert!(
            unknown.message.contains("unknown binding") && unknown.message.contains("missing"),
            "{}",
            unknown.message
        );
    }

    #[test]
    fn dict_spread_requires_dict_without_adding_struct_update() {
        let error = compile_source("test", "let base = {a: 1}; {...base, b: 2}").unwrap_err();
        assert!(
            error.message.contains("Dict spread requires Dict")
                && error.message.contains("{a: Int}"),
            "{}",
            error.message
        );

        let duplicate =
            compile_source("test", "let base: Dict(Int) = {}; {...base, a: 1, a: 2}").unwrap_err();
        assert!(duplicate.message.contains("duplicate Dict field"));
    }

    #[test]
    fn non_exhaustive_match_has_a_dedicated_error() {
        let error =
            run("let fail: Fn(Any) -> Int = fn(value) { match value { 'Some => 1 } }; fail('None)")
                .unwrap_err();
        let ExecutionError::Runtime(error) = error else {
            panic!("expected runtime error");
        };
        assert_eq!(error.kind, RuntimeErrorKind::NoPatternMatched);
    }

    #[test]
    fn reports_unknown_bindings_and_arity_errors() {
        let unknown = compile_source("test", "let present = 1;\nmissing").unwrap_err();
        assert!(unknown.message.contains("unknown binding"));
        assert_eq!(unknown.location.line, 2);
        assert_eq!(unknown.location.column, 1);

        let error = run("let f = fn(a) { a }; f(1, 2)").unwrap_err();
        let ExecutionError::Frontend(error) = error else {
            panic!("expected frontend error");
        };
        assert!(error.message.contains("call expects 1 arguments, found 2"));
    }

    #[test]
    fn runtime_errors_retain_expression_origins_and_call_trace() {
        let error =
            run("let divide = fn(x) {\n  x / 0\n};\nlet result = divide(4); result").unwrap_err();
        let ExecutionError::Runtime(error) = error else {
            panic!("expected runtime error");
        };
        assert_eq!(error.kind, RuntimeErrorKind::DivisionByZero);
        assert_eq!(error.trace.len(), 2);
        let Origin::Source(location) = error.origin().expect("runtime origin") else {
            panic!("expected source origin");
        };
        assert_eq!(location.start, 23);
        assert!(error.to_string().contains("test:2:3"));

        let tail = run("let divide = fn(x) { x / 0 }; divide(4)").unwrap_err();
        let ExecutionError::Runtime(tail) = tail else {
            panic!("expected runtime error");
        };
        assert_eq!(tail.trace.len(), 1);
    }

    #[test]
    fn runtime_field_and_interpolation_errors_render_their_expressions() {
        let field = run("let value = {present: 1};\nvalue.missing").unwrap_err();
        assert!(field.to_string().contains("test:2:1"));

        let interpolation =
            run("def render = fn(value) {\n  `value=\\{value}`\n};\nrender([1])").unwrap_err();
        assert!(interpolation.to_string().contains("test:2:3"));
    }

    #[test]
    fn generated_function_results_rebase_to_the_authored_call_site() {
        let source = "def inner: Fn() -> Any = fn() { 1 + 1 };\ndef outer: Fn() -> Any = fn() { inner() };\nlet value = outer();\nvalue.missing";
        let call_start = source.find("outer();").unwrap();
        let error = run(source).unwrap_err();
        let ExecutionError::Runtime(error) = error else {
            panic!("expected runtime error");
        };
        let data = error.data_location().expect("generated value location");
        assert_eq!(data.range(), call_start..call_start + "outer()".len());
    }

    #[test]
    fn fuel_exhaustion_points_to_the_call_expression() {
        let error = run_source("test", "let f = fn() { 1 };\nf()", 0).unwrap_err();
        let ExecutionError::Runtime(error) = error else {
            panic!("expected runtime error");
        };
        assert_eq!(error.kind, RuntimeErrorKind::FuelExhausted);
        assert!(error.to_string().contains("test:2:1"));
    }
}
