struct ToolEvaluator<'a> {
    observed_vm: Vm,
    silent_vm: Vm,
    main: &'a mut Heap,
    work: Heap,
}

impl<'a> ToolEvaluator<'a> {
    fn new(debug_sink: Arc<dyn DebugSink>, main: &'a mut Heap) -> Self {
        let work = Heap::work_for(main);
        Self {
            observed_vm: Vm::new().with_debug_sink(debug_sink),
            silent_vm: Vm::new().with_debug_sink(Arc::new(DiscardDebugSink)),
            main,
            work,
        }
    }

    fn descriptor(&mut self, descriptor: &TypeDescriptor) -> Result<Val, FrontendError> {
        self.work
            .type_descriptor_value(Some(self.main), descriptor)
            .map_err(|error| frontend_error("<tool-stage>", error.to_string()))
    }

    fn install_bootstrap(&mut self) -> Result<BTreeMap<String, Val>, FrontendError> {
        let mut values = BTreeMap::new();
        for (name, descriptor) in [
            ("Type", TypeDescriptor::Type),
            ("Dyn", TypeDescriptor::Dyn),
            ("Any", TypeDescriptor::Any),
            ("Never", TypeDescriptor::Never),
            ("Int", TypeDescriptor::Int),
            ("Float", TypeDescriptor::Float),
            ("String", TypeDescriptor::String),
            ("Bytes", TypeDescriptor::Bytes),
            ("BlameError", blame_error_descriptor()),
        ] {
            values.insert(name.into(), self.descriptor(&descriptor)?);
        }
        values.insert(
            "Bool".into(),
            self.work
                .normalized_bool_type_value(Some(self.main))
                .map_err(|error| frontend_error("<tool-stage>", error.to_string()))?,
        );
        for function in [
            NativeFunction::core_model(CoreModelFunction::Struct),
            NativeFunction::core_model(CoreModelFunction::Enum),
            NativeFunction::core_model(CoreModelFunction::Union),
            NativeFunction::core_builtin_type(CoreBuiltinTypeFunction::Option),
            NativeFunction::core_builtin_type(CoreBuiltinTypeFunction::Result),
            NativeFunction::core_builtin_type(CoreBuiltinTypeFunction::FoldControl),
            NativeFunction::new("Atom", 1, native_atom_type),
            NativeFunction::new("Array", 1, native_array_type),
            NativeFunction::new("Dict", 1, native_dict_type),
            NativeFunction::new("TypeOf", 1, native_type_of_type),
            NativeFunction::new("Tagged", 2, native_tagged_type),
            NativeFunction::new("Tuple", 1, native_tuple_type),
            NativeFunction::new("Func", 2, native_function_type),
            NativeFunction::new("validate", 2, native_validate),
            NativeFunction::new("\0telora_cast", 2, native_checked_cast),
            NativeFunction::core_diagnostic(CoreDiagnosticFunction::Warn),
        ] {
            values.insert(
                function.name().into(),
                self.work
                    .native_closure(function, Vec::<Val>::new().into_boxed_slice()),
            );
        }
        let pack = NativeFunction::core_dyn(CoreDynFunction::Pack);
        values.insert(
            "\0telora_pack_dyn".into(),
            self.work
                .native_closure(pack, Vec::<Val>::new().into_boxed_slice()),
        );
        Ok(values)
    }

    fn persist_table(
        &mut self,
        entries: impl IntoIterator<Item = (String, Val)>,
    ) -> Result<PersistentValue, FrontendError> {
        let root = self
            .work
            .module(entries)
            .map_err(|error| frontend_error("<tool-stage>", error.to_string()))?;
        publish_root(self.main, &self.work, root)
            .map_err(|error| frontend_error("<tool-stage>", error.to_string()))
    }

    fn decode_type(&self, value: Val, path: &str) -> Result<TypeDescriptor, String> {
        decode_type_ref(ValueRef::work(value, &self.work, self.main), path)
    }

    fn declared_type_id(&self, value: Val) -> Result<TypeId, FrontendError> {
        HeapView {
            current: &self.work,
            background: Some(self.main),
        }
        .declared_type_id(value)
        .map_err(|error| frontend_error("<tool-stage>", error.to_string()))
    }

    fn canonical_type_id(&self, descriptor: &TypeDescriptor) -> Result<TypeId, FrontendError> {
        self.work
            .canonical_descriptor_type_id(descriptor)
            .map_err(|error| frontend_error("<tool-stage>", error.to_string()))
    }

    fn native_decorator_type(&self) -> Option<TypeId> {
        self.main.native_decorator_type()
    }

    fn has_type_property(&self, target: TypeId, property: TypeId) -> bool {
        HeapView {
            current: &self.work,
            background: Some(self.main),
        }
        .type_property(target, property)
        .is_some()
    }

    fn property_marker(&mut self, property_type: TypeId) -> Val {
        self.work.empty_record_with_type(property_type)
    }

    fn publish_type_properties(
        &mut self,
        native_decorator_type: Option<TypeId>,
        properties: &[(TypeId, TypeId, Val)],
    ) -> Result<(), FrontendError> {
        publish_type_properties(
            self.main,
            &self.work,
            native_decorator_type,
            properties,
        )
        .map_err(|error| frontend_error("<tool-stage>", error.to_string()))
    }

    fn decode_type_graph(
        &self,
        value: Val,
        path: &str,
    ) -> Result<(TypeGraph, AnalysisTypeId), String> {
        let mut graph = TypeGraph::default();
        let root = graph.decode_persistent(
            ValueRef::work(value, &self.work, self.main),
            path,
            &mut HashMap::new(),
        )?;
        Ok((graph, root))
    }

    fn create_type_family(
        &mut self,
        metadata: Val,
        arity: usize,
        constructor: Option<&NominalTypeConstructor>,
    ) -> Result<(Val, PersistentValue, PersistentValue), FrontendError> {
        let arity_value = i64::try_from(arity)
            .map_err(|_| frontend_error("<tool-stage>", "type-family arity exceeds Int"))?;
        let template = publish_root(self.main, &self.work, metadata)
            .map_err(|error| frontend_error("<tool-stage>", error.to_string()))?;
        let module = constructor.map_or(-1, |constructor| i64::from(constructor.id.module.raw()));
        let local = constructor.map_or(0, |constructor| i64::from(constructor.id.local));
        let name = constructor.map_or("", |constructor| constructor.name.as_str());
        let work_name = Val::unknown(self.work.string(Some(self.main), name));
        let main_name = Val::unknown(self.main.string(None, name));
        let family = self.work.native_closure(
            NativeFunction::new("type-family.apply", arity, native_apply_type_family),
            vec![
                metadata,
                self.work.int(arity_value),
                self.work.int(module),
                self.work.int(local),
                work_name,
            ],
        );
        let persistent_family = self.main.native_closure(
            NativeFunction::new("type-family.apply", arity, native_apply_type_family),
            vec![
                template.runtime(),
                self.main.int(arity_value),
                self.main.int(module),
                self.main.int(local),
                main_name,
            ],
        );
        let root = self
            .main
            .persistent(persistent_family)
            .map_err(|error| frontend_error("<tool-stage>", error.to_string()))?;
        Ok((family, template, root))
    }

    fn reserve_recursive_type_family(
        &mut self,
        constructor: &NominalTypeConstructor,
        parameters: &[TypeParameter],
    ) -> Result<(Val, Val), FrontendError> {
        let arguments = parameters
            .iter()
            .map(|parameter| TypeDescriptor::Bound(parameter.id))
            .collect::<Vec<_>>();
        let id = crate::value::DeclaredTypeId::applied(
            constructor.id.module,
            constructor.id.local,
            &arguments,
        );
        let placeholder = self.descriptor(&TypeDescriptor::Any)?;
        let root = self
            .work
            .reserve_symbolic_type_ref(id, constructor.name.as_str(), placeholder)
            .map_err(|error| frontend_error("<tool-stage>", error.to_string()))?;
        let family = self.work.native_closure(
            NativeFunction::new(
                "recursive-type-family.apply",
                parameters.len(),
                native_apply_recursive_type_family,
            ),
            vec![root],
        );
        Ok((root, family))
    }
}

struct RecursiveTypeFamilyBuild {
    family_value: Val,
    family: TypeFamilyTemplate,
    scheme: TypeScheme,
}

#[allow(clippy::too_many_arguments)]
fn build_recursive_type_family(
    source_name: &str,
    module_id: crate::ModuleId,
    declaration: u32,
    binding: &Binding,
    base_bindings: &BTreeMap<String, Val>,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator<'_>,
) -> Result<RecursiveTypeFamilyBuild, FrontendError> {
    let mut evaluation_bindings = base_bindings.clone();
    let mut parameters = Vec::new();
    let mut parameter_names = HashSet::new();
    for (index, parameter) in binding.value.type_parameters.iter().enumerate() {
        if !parameter_names.insert(parameter.value.as_str()) {
            return Err(FrontendError::from_diagnostic(
                sources,
                Diagnostic::error(
                    format!("duplicate type parameter {:?}", parameter.value),
                    parameter.location,
                ),
            ));
        }
        let id = TypeParameterId(
            u32::try_from(index)
                .map_err(|_| frontend_error(source_name, "type family has too many parameters"))?,
        );
        parameters.push(TypeParameter {
            id,
            name: parameter.value.clone(),
            location: parameter.location,
        });
        evaluation_bindings.insert(
            parameter.value.clone(),
            evaluator.descriptor(&TypeDescriptor::Bound(id))?,
        );
    }
    let constructor = NominalTypeConstructor {
        id: crate::TypeConstructorId {
            module: module_id,
            local: declaration,
        },
        name: binding.value.name.value.clone(),
    };
    let (symbolic_root, self_family) =
        evaluator.reserve_recursive_type_family(&constructor, &parameters)?;
    evaluation_bindings.insert(binding.value.name.value.clone(), self_family);
    let body = evaluate_tool_expression(
        source_name,
        &binding.value.value,
        &evaluation_bindings,
        account,
        sources,
        evaluator,
    )?;
    validate_declared_metadata(source_name, binding, body, evaluator)?;
    evaluator
        .work
        .seal_type_ref(symbolic_root, body)
        .map_err(|error| frontend_error(source_name, error.to_string()))?;
    let (graph, root) = evaluator
        .decode_type_graph(symbolic_root, "Type")
        .map_err(|message| {
            frontend_error(
                source_name,
                format!(
                    "type family {} produced invalid metadata: {message}",
                    binding.value.name.value
                ),
            )
        })?;
    let descriptor = graph.descriptor(root).map_err(|message| {
        frontend_error(
            source_name,
            format!(
                "type family {} produced invalid metadata: {message}",
                binding.value.name.value
            ),
        )
    })?;
    let mut bounds = Vec::new();
    collect_bound_parameters(&descriptor, &mut bounds);
    if let Some(foreign) = bounds
        .iter()
        .find(|bound| !parameters.iter().any(|parameter| parameter.id == **bound))
    {
        return Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(
                format!(
                    "type family {} produced foreign bound parameter T{}",
                    binding.value.name.value, foreign.0
                ),
                binding.value.value.location,
            ),
        ));
    }
    let (family_value, template, root) =
        evaluator.create_type_family(symbolic_root, parameters.len(), None)?;
    let family = TypeFamilyTemplate {
        parameters: parameters.clone(),
        template,
        root,
        rebuild_at_runtime: contains_named_type(&descriptor),
        constructor: Some(constructor),
    };
    let scheme = TypeScheme {
        parameters,
        body: TypeDescriptor::Function {
            parameters: family
                .parameters
                .iter()
                .map(|parameter| {
                    TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Bound(parameter.id)))
                })
                .collect(),
            result: Box::new(TypeDescriptor::TypeOf(Box::new(descriptor.clone()))),
        },
    };
    Ok(RecursiveTypeFamilyBuild {
        family_value,
        family,
        scheme,
    })
}

fn native_apply_recursive_type_family(
    context: &mut CallContext<'_, '_>,
) -> Result<(), NativeError> {
    for index in 0..context.argument_count() {
        let argument = native_type_argument_descriptor(context, context.argument(index)?, index)?;
        if argument
            != TypeDescriptor::Bound(TypeParameterId(
                u32::try_from(index)
                    .map_err(|_| NativeError::new("type-family parameter index exceeds u32"))?,
            ))
        {
            return Err(NativeError::new(
                "recursive type-family application must use its bound parameters unchanged and in declaration order",
            ));
        }
    }
    context.copy(context.result(), context.upvalue(0)?)?;
    context.mark_at_call_site(context.result())
}

fn native_apply_type_family(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let template = context.upvalue(0)?;
    let arity = context
        .value(context.upvalue(1)?)?
        .as_int()
        .and_then(|arity| usize::try_from(arity).ok())
        .ok_or_else(|| NativeError::new("invalid type-family arity"))?;
    let mut argument_registers = Vec::with_capacity(arity);
    let mut argument_descriptors = Vec::with_capacity(arity);
    for index in 0..arity {
        let register = context.argument(index)?;
        let argument = native_type_argument_descriptor(context, register, index)?;
        argument_registers.push(register);
        argument_descriptors.push(argument);
    }
    let result = context.result();
    context.instantiate_type_family(
        result,
        template,
        &argument_registers,
        &argument_descriptors,
    )?;
    let module = context
        .value(context.upvalue(2)?)?
        .as_int()
        .ok_or_else(|| NativeError::new("invalid type-constructor module ID"))?;
    if module >= 0 {
        let module = u32::try_from(module)
            .map(crate::ModuleId::from_raw)
            .map_err(|_| NativeError::new("invalid type-constructor module ID"))?;
        let local = context
            .value(context.upvalue(3)?)?
            .as_int()
            .and_then(|local| u32::try_from(local).ok())
            .ok_or_else(|| NativeError::new("invalid type-constructor local ID"))?;
        let name = context
            .value(context.upvalue(4)?)?
            .as_str()
            .ok_or_else(|| NativeError::new("invalid type-constructor name"))?
            .to_string();
        let id = crate::value::DeclaredTypeId::applied(module, local, &argument_descriptors);
        context.make_declared_type_application(result, id, name, result, &argument_registers)?;
    }
    context.mark_at_call_site(result)
}

pub(crate) fn native_declare_type_family(
    context: &mut CallContext<'_, '_>,
) -> Result<(), NativeError> {
    let body = context.argument(0)?;
    let module = context
        .value(context.argument(1)?)?
        .as_int()
        .and_then(|module| u32::try_from(module).ok())
        .map(crate::ModuleId::from_raw)
        .ok_or_else(|| NativeError::new("invalid type-constructor module ID"))?;
    let local = context
        .value(context.argument(2)?)?
        .as_int()
        .and_then(|local| u32::try_from(local).ok())
        .ok_or_else(|| NativeError::new("invalid type-constructor local ID"))?;
    let name = context
        .value(context.argument(3)?)?
        .as_str()
        .ok_or_else(|| NativeError::new("invalid type-constructor name"))?
        .to_string();
    let arity = context.argument_count().saturating_sub(4);
    let mut argument_registers = Vec::with_capacity(arity);
    let mut argument_descriptors = Vec::with_capacity(arity);
    for index in 0..arity {
        let register = context.argument(index + 4)?;
        let argument = native_type_argument_descriptor(context, register, index)?;
        argument_registers.push(register);
        argument_descriptors.push(argument);
    }
    let id = crate::value::DeclaredTypeId::applied(module, local, &argument_descriptors);
    context.make_declared_type_application(
        context.result(),
        id,
        name,
        body,
        &argument_registers,
    )?;
    context.mark_at_call_site(context.result())
}

fn native_type_argument_descriptor(
    context: &CallContext<'_, '_>,
    register: RegisterId,
    index: usize,
) -> Result<TypeDescriptor, NativeError> {
    decode_native_type(context.value(register)?).map_err(|error| {
        NativeError::new(format!(
            "type-family argument {} is not valid TypeMetadata: {}",
            index + 1,
            error.message
        ))
    })
}

#[allow(clippy::too_many_arguments)]
fn collect_nested_annotation_types(
    source_name: &str,
    expression: &Expr,
    bindings: &BTreeMap<String, Val>,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    debug_sink: &mut ToolEvaluator,
    annotations: &mut HashMap<crate::Location, TypeDescriptor>,
) -> Result<(), FrontendError> {
    match &expression.value {
        ExprKind::InterpolatedString(parts) => {
            for part in parts {
                if let StringPartKind::Expression(expression) = &part.value {
                    collect_nested_annotation_types(
                        source_name,
                        expression,
                        bindings,
                        account,
                        sources,
                        debug_sink,
                        annotations,
                    )?;
                }
            }
        }
        ExprKind::Array(items) | ExprKind::Tuple(items) => {
            for item in items {
                collect_nested_annotation_types(
                    source_name,
                    item,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::Spread(operand) => collect_nested_annotation_types(
            source_name,
            operand,
            bindings,
            account,
            sources,
            debug_sink,
            annotations,
        )?,
        ExprKind::Dict(fields) => {
            for field in fields {
                collect_nested_annotation_types(
                    source_name,
                    &field.value.value,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::Block(block) => {
            collect_block_annotation_types(
                source_name,
                block,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
        }
        ExprKind::Closure {
            parameters,
            result_annotation,
            body,
        } => {
            for annotation in parameters
                .iter()
                .filter_map(|parameter| parameter.annotation.as_ref())
                .chain(result_annotation.as_deref())
            {
                let metadata = evaluate_tool_expression(
                    source_name,
                    annotation,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                )?;
                let descriptor = debug_sink
                    .decode_type(metadata, "Type")
                    .map_err(|message| {
                        FrontendError::from_diagnostic(
                            sources,
                            Diagnostic::error(
                                format!("closure annotation is invalid: {message}"),
                                annotation.location,
                            ),
                        )
                    })?;
                annotations.insert(annotation.location, descriptor);
            }
            collect_block_annotation_types(
                source_name,
                body,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
        }
        ExprKind::Unary { operand, .. }
        | ExprKind::Propagate { operand }
        | ExprKind::Field {
            receiver: operand, ..
        }
        | ExprKind::TupleProjection {
            receiver: operand, ..
        } => {
            collect_nested_annotation_types(
                source_name,
                operand,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
        }
        ExprKind::Return { value } => collect_nested_annotation_types(
            source_name,
            value,
            bindings,
            account,
            sources,
            debug_sink,
            annotations,
        )?,
        ExprKind::Panic { message } => collect_nested_annotation_types(
            source_name,
            message,
            bindings,
            account,
            sources,
            debug_sink,
            annotations,
        )?,
        ExprKind::Raise { error } => collect_nested_annotation_types(
            source_name,
            error,
            bindings,
            account,
            sources,
            debug_sink,
            annotations,
        )?,
        ExprKind::Debug { value, .. } => collect_nested_annotation_types(
            source_name,
            value,
            bindings,
            account,
            sources,
            debug_sink,
            annotations,
        )?,
        ExprKind::TypeAscription { value, target } | ExprKind::CheckedCast { value, target } => {
            collect_nested_annotation_types(
                source_name,
                value,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
            let metadata = evaluate_tool_expression(
                source_name,
                target,
                bindings,
                account,
                sources,
                debug_sink,
            )?;
            let descriptor = debug_sink
                .decode_type(metadata, "Type")
                .map_err(|message| {
                    FrontendError::from_diagnostic(
                        sources,
                        Diagnostic::error(
                            format!("type target is invalid: {message}"),
                            target.location,
                        ),
                    )
                })?;
            annotations.insert(target.location, descriptor);
        }
        ExprKind::DynProject {
            namespace,
            target,
            value,
        } => {
            for expression in [namespace.as_ref(), value.as_ref()] {
                collect_nested_annotation_types(
                    source_name,
                    expression,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
            let metadata = evaluate_tool_expression(
                source_name,
                target,
                bindings,
                account,
                sources,
                debug_sink,
            )?;
            let descriptor = debug_sink
                .decode_type(metadata, "Type")
                .map_err(|message| {
                    FrontendError::from_diagnostic(
                        sources,
                        Diagnostic::error(
                            format!("Dyn projection target is invalid: {message}"),
                            target.location,
                        ),
                    )
                })?;
            annotations.insert(target.location, descriptor);
        }
        ExprKind::Binary { left, right, .. } => {
            for expression in [left.as_ref(), right.as_ref()] {
                collect_nested_annotation_types(
                    source_name,
                    expression,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::Index { receiver, index } => {
            for expression in [receiver.as_ref(), index.as_ref()] {
                collect_nested_annotation_types(
                    source_name,
                    expression,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::Call { callee, arguments } => {
            collect_nested_annotation_types(
                source_name,
                callee,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
            for argument in arguments {
                collect_nested_annotation_types(
                    source_name,
                    argument,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::TypeApply { callee, arguments } => {
            collect_nested_annotation_types(
                source_name,
                callee,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
            for argument in arguments {
                let TypeArgumentKind::Explicit(expression) = &argument.value else {
                    continue;
                };
                let metadata = evaluate_tool_expression(
                    source_name,
                    expression,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                )?;
                let descriptor = debug_sink
                    .decode_type(metadata, "Type")
                    .map_err(|message| {
                        FrontendError::from_diagnostic(
                            sources,
                            Diagnostic::error(
                                format!("type argument is invalid: {message}"),
                                expression.location,
                            ),
                        )
                    })?;
                annotations.insert(expression.location, descriptor);
            }
        }
        ExprKind::Interpreter { operand, .. } => collect_nested_annotation_types(
            source_name,
            operand,
            bindings,
            account,
            sources,
            debug_sink,
            annotations,
        )?,
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_nested_annotation_types(
                source_name,
                condition,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
            for block in [then_branch, else_branch] {
                collect_block_annotation_types(
                    source_name,
                    block,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::IfLet {
            value,
            then_branch,
            else_branch,
            ..
        } => {
            collect_nested_annotation_types(
                source_name,
                value,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
            for block in [then_branch, else_branch] {
                collect_block_annotation_types(
                    source_name,
                    block,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::LetElse {
            value,
            else_branch,
            body,
            ..
        } => {
            collect_nested_annotation_types(
                source_name,
                value,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
            for block in [else_branch, body] {
                collect_block_annotation_types(
                    source_name,
                    block,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::Match { value, arms } => {
            collect_nested_annotation_types(
                source_name,
                value,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
            for arm in arms {
                if let Some(guard) = &arm.value.guard {
                    collect_nested_annotation_types(
                        source_name,
                        guard,
                        bindings,
                        account,
                        sources,
                        debug_sink,
                        annotations,
                    )?;
                }
                collect_nested_annotation_types(
                    source_name,
                    &arm.value.value,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Bytes(_)
        | ExprKind::Atom(_)
        | ExprKind::Variable(_) => {}
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn collect_block_annotation_types(
    source_name: &str,
    block: &Block,
    bindings: &BTreeMap<String, Val>,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    debug_sink: &mut ToolEvaluator,
    annotations: &mut HashMap<crate::Location, TypeDescriptor>,
) -> Result<(), FrontendError> {
    for binding in &block.value.bindings {
        if let Some(annotation) = &binding.value.annotation {
            let metadata = evaluate_tool_expression(
                source_name,
                annotation,
                bindings,
                account,
                sources,
                debug_sink,
            )?;
            let descriptor = debug_sink
                .decode_type(metadata, "Type")
                .map_err(|message| {
                    FrontendError::from_diagnostic(
                        sources,
                        Diagnostic::error(
                            format!(
                                "annotation on {} is invalid: {message}",
                                binding.value.name.value
                            ),
                            annotation.location,
                        ),
                    )
                })?;
            annotations.insert(annotation.location, descriptor);
        }
        collect_nested_annotation_types(
            source_name,
            &binding.value.value,
            bindings,
            account,
            sources,
            debug_sink,
            annotations,
        )?;
    }
    collect_nested_annotation_types(
        source_name,
        &block.value.result,
        bindings,
        account,
        sources,
        debug_sink,
        annotations,
    )
}
