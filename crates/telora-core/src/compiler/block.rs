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

}

