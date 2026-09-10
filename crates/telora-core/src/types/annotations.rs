#[cfg(test)]
mod static_annotation_tests {
    use super::*;

    #[test]
    fn declarations_never_fall_back_to_executing_value_helpers() {
        for declaration in [
            "type T = make();",
            "type F(T) = make();",
            "type Node = struct { next: Node, bad: make() };",
        ] {
            let source = format!("def make = fn() {{ Int.type }}; {declaration}");
            let error = analyze_source_with_fuel("static-only-declarations", &source, 0).unwrap_err();
            assert!(error.message.contains("expected a static type declaration"),
                "{declaration}: {}", error.message);
            assert!(!error.message.contains("fuel"));
        }
    }

    #[test]
    fn tool_dependency_contracts_resolve_without_runtime_metadata() {
        for (text, expected) in [
            ("Int", Some(TypeDescriptor::Int)),
            ("Array(Int)", Some(TypeDescriptor::Array(Box::new(TypeDescriptor::Int)))),
            ("make_type()", None),
        ] {
            let mut sources = SourceDatabase::default();
            let source = sources.add("static-check-contract", text);
            let program = parse_registered(&sources, source).program.unwrap();
            let mut prelude = BootstrapPrelude::new();
            prelude.types.insert("make_type".into(), TypeDescriptor::Function {
                parameters: Vec::new(), result: Box::new(TypeDescriptor::Type),
            });
            let hir = HirProgram::resolve(&program, prelude.types.keys().cloned());
            let context = ToolInferenceContext::new(TypeGraph::default(), &hir, BTreeMap::new(), prelude.types,
                prelude.schemes, BTreeMap::new(), true, HashSet::new());
            let expression = &program.value.body.value.result;
            let mut graph = TypeGraph::default();
            let result = with_tool_annotation_context(expression, &context, &mut graph,
                |static_types| static_types.elaborate(expression, &sources));
            match expected {
                Some(expected) => assert_eq!(graph.descriptor(result.unwrap()).unwrap(), expected),
                None => assert!(result.is_err(), "a value-level type factory must not run"),
            }
        }
    }

    #[test]
    fn local_annotations_use_the_static_graph_without_execution_fuel() {
        for source in [
            "def f: Fn(Int) -> Int = fn(x: Int) -> Int { let y: Int = x; y };",
            "def f: for(T) Fn(T) -> T = fn(x: T) -> T { let y: T = x; y };",
            "type Node = struct {value: Int}; def f: Fn(Node) -> Node = fn(x) { let y: Node = x; y };",
            "type Pair(T) = (T, T); def f: Fn(Pair(Int)) -> Pair(Int) = fn(x) { let y: Pair(Int) = x; y };",
        ] {
            analyze_source_with_fuel("static-local-annotations", source, 0).unwrap();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_program_annotations(
    program: &Program,
    sources: &SourceDatabase,
    scope: StaticContractScope<'_>,
    graph: &mut TypeGraph,
) -> Result<HashMap<crate::Location, AnalysisTypeId>, FrontendError> {
    let mut annotations = HashMap::new();
    for binding in &program.value.body.value.bindings {
        let parameters = static_contract_parameters(binding, sources)?;
        let mut context = StaticAnnotationContext {
            scope: StaticContractScope {
                parameters: &parameters,
                ..scope
            },
            graph,
        };
        collect_nested_annotation_types(
            &binding.value.value,
            sources,
            &mut annotations,
            &mut context,
        )?;
        for decorator in &binding.value.decorators {
            collect_nested_annotation_types(&decorator.value.callee, sources, &mut annotations, &mut context)?;
            for argument in &decorator.value.arguments {
                collect_nested_annotation_types(argument, sources, &mut annotations, &mut context)?;
            }
        }
    }
    let mut context = StaticAnnotationContext { scope, graph };
    collect_nested_annotation_types(
        &program.value.body.value.result,
        sources,
        &mut annotations,
        &mut context,
    )?;
    Ok(annotations)
}

fn collect_tool_annotations(
    expression: &Expr,
    context: &ToolInferenceContext<'_>,
    sources: &SourceDatabase,
    annotations: &mut HashMap<crate::Location, AnalysisTypeId>,
    graph: &mut TypeGraph,
) -> Result<(), FrontendError> {
    with_tool_annotation_context(expression, context, graph, |static_types| {
        collect_nested_annotation_types(expression, sources, annotations, static_types)
    })
}

fn with_tool_annotation_context<R>(
    expression: &Expr,
    context: &ToolInferenceContext<'_>,
    graph: &mut TypeGraph,
    consume: impl FnOnce(&mut StaticAnnotationContext<'_, '_>) -> Result<R, FrontendError>,
) -> Result<R, FrontendError> {
    let mut families = static_type_families(
        graph,
        &context.interfaces,
    );
    for (name, scheme) in &context.schemes {
        if let Some(family) = StaticTypeFamily::from_scheme(scheme, graph) {
            families.insert(name.clone(), family);
        }
    }
    let parameters = context
        .hir
        .definitions()
        .iter()
        .filter_map(|definition| {
            let value = context.hir.expression(definition.value?)?;
            (value.location.source == expression.location.source
                && value.location.start <= expression.location.start
                && expression.location.end <= value.location.end
                && !definition.type_parameters.is_empty())
            .then_some((value.location.end - value.location.start, definition))
        })
        .min_by_key(|(length, _)| *length)
        .map(|(_, definition)| {
            definition
                .type_parameters
                .iter()
                .enumerate()
                .map(|(index, parameter)| TypeParameter {
                    id: TypeParameterId(index as u32),
                    name: parameter.name.clone(),
                    location: parameter.location,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let external_names = context.interfaces.keys().map(String::as_str).collect();
    let mut static_types = StaticAnnotationContext {
        scope: StaticContractScope {
            hir: context.hir,
            environment: &context.environment,
            external_names: &external_names,
            interfaces: &context.interfaces,
            parameters: &parameters,
            families: &families,
        },
        graph,
    };
    consume(&mut static_types)
}

// Annotation elaboration shares the caller's solved type graph. Descriptor output
// remains a compatibility boundary for the current strict inference API.
struct StaticAnnotationContext<'a, 'graph> {
    scope: StaticContractScope<'a>,
    graph: &'graph mut TypeGraph,
}

impl StaticAnnotationContext<'_, '_> {
    fn elaborate(
        &mut self,
        expression: &Expr,
        sources: &SourceDatabase,
    ) -> Result<AnalysisTypeId, FrontendError> {
        let invalid = |message: String| {
            FrontendError::from_diagnostic(sources, Diagnostic::error(message, expression.location))
        };
        let root = self
            .scope
            .elaborate(expression, self.graph)
            .ok_or_else(|| invalid("type syntax has no solved static type".into()))?;
        Ok(root)
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_nested_annotation_types(
    expression: &Expr,
    sources: &SourceDatabase,
    annotations: &mut HashMap<crate::Location, AnalysisTypeId>,
    static_types: &mut StaticAnnotationContext<'_, '_>,
) -> Result<(), FrontendError> {
    match &expression.value {
        ExprKind::InterpolatedString(parts) => {
            for part in parts {
                if let StringPartKind::Expression(expression) = &part.value {
                    collect_nested_annotation_types(
                        expression,
                        sources,
                        annotations,
                        static_types,
                    )?;
                }
            }
        }
        ExprKind::Array(items) | ExprKind::Tuple(items) => {
            for item in items {
                collect_nested_annotation_types(item, sources, annotations, static_types)?;
            }
        }
        ExprKind::TypeMetadata(operand) => {
            let descriptor = static_types.elaborate(operand, sources)?;
            annotations.insert(expression.location, descriptor);
        }
        ExprKind::TypeSyntax(operand) | ExprKind::Spread(operand) => {
            collect_nested_annotation_types(operand, sources, annotations, static_types)?
        }
        ExprKind::Dict(fields) => {
            for field in fields {
                collect_nested_annotation_types(
                    &field.value.value,
                    sources,
                    annotations,
                    static_types,
                )?;
            }
        }
        ExprKind::Block(block) => {
            collect_block_annotation_types(block, sources, annotations, static_types)?;
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
                let descriptor = static_types.elaborate(annotation, sources)?;
                annotations.insert(annotation.location, descriptor);
            }
            collect_block_annotation_types(body, sources, annotations, static_types)?;
        }
        ExprKind::Unary { operand, .. }
        | ExprKind::FieldProjection {
            receiver: operand, ..
        }
        | ExprKind::Propagate { operand }
        | ExprKind::Field {
            receiver: operand, ..
        }
        | ExprKind::TupleProjection {
            receiver: operand, ..
        } => {
            collect_nested_annotation_types(operand, sources, annotations, static_types)?;
        }
        ExprKind::Return { value } => {
            collect_nested_annotation_types(value, sources, annotations, static_types)?
        }
        ExprKind::Panic { message } => {
            collect_nested_annotation_types(message, sources, annotations, static_types)?
        }
        ExprKind::Raise {
            message, subjects, ..
        } => {
            for value in std::iter::once(message.as_ref()).chain(subjects.iter()) {
                collect_nested_annotation_types(value, sources, annotations, static_types)?;
            }
        }
        ExprKind::Debug { value, .. } => {
            collect_nested_annotation_types(value, sources, annotations, static_types)?
        }
        ExprKind::TypeAscription { value, target } | ExprKind::CheckedCast { value, target } => {
            collect_nested_annotation_types(value, sources, annotations, static_types)?;
            let descriptor = static_types.elaborate(target, sources)?;
            annotations.insert(target.location, descriptor);
        }
        ExprKind::DynProject {
            namespace,
            target,
            value,
        } => {
            for expression in [namespace.as_ref(), value.as_ref()] {
                collect_nested_annotation_types(expression, sources, annotations, static_types)?;
            }
            let descriptor = static_types.elaborate(target, sources)?;
            annotations.insert(target.location, descriptor);
        }
        ExprKind::Binary { left, right, .. } => {
            for expression in [left.as_ref(), right.as_ref()] {
                collect_nested_annotation_types(expression, sources, annotations, static_types)?;
            }
        }
        ExprKind::Index { receiver, index } => {
            for expression in [receiver.as_ref(), index.as_ref()] {
                collect_nested_annotation_types(expression, sources, annotations, static_types)?;
            }
        }
        ExprKind::Call { callee, arguments } => {
            collect_nested_annotation_types(callee, sources, annotations, static_types)?;
            for argument in arguments {
                collect_nested_annotation_types(argument, sources, annotations, static_types)?;
            }
        }
        ExprKind::TypeApply { callee, arguments } => {
            collect_nested_annotation_types(callee, sources, annotations, static_types)?;
            for argument in arguments {
                let TypeArgumentKind::Explicit(expression) = &argument.value else {
                    continue;
                };
                let descriptor = static_types.elaborate(expression, sources)?;
                annotations.insert(expression.location, descriptor);
            }
        }
        ExprKind::Interpreter { operand, .. } => {
            collect_nested_annotation_types(operand, sources, annotations, static_types)?
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_nested_annotation_types(condition, sources, annotations, static_types)?;
            for block in [then_branch, else_branch] {
                collect_block_annotation_types(block, sources, annotations, static_types)?;
            }
        }
        ExprKind::IfLet {
            value,
            then_branch,
            else_branch,
            ..
        } => {
            collect_nested_annotation_types(value, sources, annotations, static_types)?;
            for block in [then_branch, else_branch] {
                collect_block_annotation_types(block, sources, annotations, static_types)?;
            }
        }
        ExprKind::LetElse {
            value,
            else_branch,
            body,
            ..
        } => {
            collect_nested_annotation_types(value, sources, annotations, static_types)?;
            for block in [else_branch, body] {
                collect_block_annotation_types(block, sources, annotations, static_types)?;
            }
        }
        ExprKind::Match { value, arms } => {
            collect_nested_annotation_types(value, sources, annotations, static_types)?;
            for arm in arms {
                if let Some(guard) = &arm.value.guard {
                    collect_nested_annotation_types(guard, sources, annotations, static_types)?;
                }
                collect_nested_annotation_types(
                    &arm.value.value,
                    sources,
                    annotations,
                    static_types,
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
    block: &Block,
    sources: &SourceDatabase,
    annotations: &mut HashMap<crate::Location, AnalysisTypeId>,
    static_types: &mut StaticAnnotationContext<'_, '_>,
) -> Result<(), FrontendError> {
    for binding in &block.value.bindings {
        if let Some(annotation) = &binding.value.annotation {
            let descriptor = static_types.elaborate(annotation, sources)?;
            annotations.insert(annotation.location, descriptor);
        }
        collect_nested_annotation_types(&binding.value.value, sources, annotations, static_types)?;
    }
    collect_nested_annotation_types(&block.value.result, sources, annotations, static_types)
}
