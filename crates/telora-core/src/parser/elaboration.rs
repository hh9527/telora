fn synthesize_export_record(bindings: &[Binding], location: Location) -> Expr {
    let fields = bindings
        .iter()
        .filter(|binding| binding.value.kind == BindingKind::Export)
        .map(|binding| {
            let local = binding
                .value
                .imported_name
                .as_deref()
                .expect("export markers retain their local name")
                .clone();
            located(
                DictFieldKind {
                    decorators: Vec::new(),
                    name: Some(binding.value.name.clone()),
                    value: located(ExprKind::Variable(local), binding.location),
                },
                binding.location,
            )
        })
        .collect();
    located(ExprKind::Dict(fields), location)
}

fn push_unique_diagnostic(diagnostics: &mut Vec<Diagnostic>, diagnostic: Diagnostic) {
    let location = diagnostic.labels.first().map(|label| label.location);
    if diagnostics.iter().any(|existing| {
        existing.message == diagnostic.message
            && existing.labels.first().map(|label| label.location) == location
    }) {
        return;
    }
    diagnostics.push(diagnostic);
}

fn elaborate_pipeline(location: Location, left: Expr, right: Expr) -> Expr {
    located(
        ExprKind::Call {
            callee: Box::new(right),
            arguments: vec![left],
        },
        location,
    )
}

const MAX_PLACEHOLDER_PARAMETERS: usize = u16::MAX as usize;

fn elaborate_call_section(
    callee: Expr,
    arguments: Vec<CallArgument>,
    section_node: NodeRef,
    location: Location,
) -> Result<Expr, (NodeRef, String)> {
    let first_bare = arguments.iter().find_map(|argument| match argument {
        CallArgument::Bare { node, .. } => Some(*node),
        _ => None,
    });
    let first_indexed = arguments.iter().find_map(|argument| match argument {
        CallArgument::Indexed { node, .. } => Some(*node),
        _ => None,
    });
    if first_bare.is_some()
        && let Some(indexed) = first_indexed
    {
        return Err((
            indexed,
            "cannot mix '_' and indexed placeholders in one call".into(),
        ));
    }

    if first_bare.is_none() && first_indexed.is_none() {
        return Err((
            section_node,
            "call section requires at least one placeholder".into(),
        ));
    }

    let mut parameter_locations = Vec::new();
    if first_bare.is_some() {
        parameter_locations.extend(arguments.iter().filter_map(|argument| match argument {
            CallArgument::Bare { location, .. } => Some(Some(*location)),
            _ => None,
        }));
    } else {
        let max = arguments
            .iter()
            .filter_map(|argument| match argument {
                CallArgument::Indexed { index, .. } => Some(*index),
                _ => None,
            })
            .max()
            .expect("indexed placeholder exists");
        if max >= MAX_PLACEHOLDER_PARAMETERS {
            let node = arguments
                .iter()
                .find_map(|argument| match argument {
                    CallArgument::Indexed { node, index, .. } if *index == max => Some(*node),
                    _ => None,
                })
                .expect("maximum placeholder has a node");
            return Err((
                node,
                format!(
                    "placeholder index exceeds the limit of {} parameters",
                    MAX_PLACEHOLDER_PARAMETERS
                ),
            ));
        }
        parameter_locations.resize(max + 1, None);
        for argument in &arguments {
            if let CallArgument::Indexed {
                index, location, ..
            } = argument
            {
                parameter_locations[*index].get_or_insert(*location);
            }
        }
        if let Some(missing) = parameter_locations.iter().position(Option::is_none) {
            return Err((
                first_indexed.expect("indexed placeholder exists"),
                format!("indexed placeholders are missing _{missing}"),
            ));
        }
    }

    let parameter_locations = parameter_locations
        .into_iter()
        .map(|location| location.expect("placeholder location was assigned"))
        .collect::<Vec<_>>();
    let parameters = parameter_locations
        .iter()
        .enumerate()
        .map(|(index, location)| ClosureParameter {
            name: located(placeholder_parameter(index), *location),
            annotation: None,
        })
        .collect::<Vec<_>>();
    let mut next_bare = 0usize;
    let arguments = arguments
        .into_iter()
        .map(|argument| match argument {
            CallArgument::Expression(expression) => expression,
            CallArgument::Bare { location, .. } => {
                let index = next_bare;
                next_bare += 1;
                placeholder_variable(index, location)
            }
            CallArgument::Indexed {
                index, location, ..
            } => placeholder_variable(index, location),
        })
        .collect();
    let call = located(
        ExprKind::Call {
            callee: Box::new(callee),
            arguments,
        },
        location,
    );
    Ok(located(
        ExprKind::Closure {
            parameters,
            result_annotation: None,
            body: located(
                BlockKind {
                    bindings: Vec::new(),
                    result: Box::new(call),
                },
                location,
            ),
        },
        location,
    ))
}

fn placeholder_parameter(index: usize) -> String {
    format!("\0telora_placeholder_{index}")
}

struct InterpreterSyntaxPlan {
    witness_count: usize,
    parameters: Vec<Option<usize>>,
}

fn interpreter_syntax_plan(
    type_parameters: &[Identifier],
    contract: &Expr,
) -> Option<InterpreterSyntaxPlan> {
    let (outer_parameters, outer_result) = function_contract_parts(contract)?;
    let mut witnesses = HashMap::new();
    for (index, witness) in outer_parameters.iter().enumerate() {
        let ExprKind::Call { callee, arguments } = &witness.value else {
            return None;
        };
        if !is_variable(callee, "TypeOf") {
            return None;
        }
        let [argument] = arguments.as_slice() else {
            return None;
        };
        let ExprKind::Variable(parameter) = &argument.value else {
            return None;
        };
        if !type_parameters
            .iter()
            .any(|candidate| candidate.value == parameter.value)
            || witnesses.insert(parameter.value.clone(), index).is_some()
        {
            return None;
        }
    }
    if witnesses.len() != type_parameters.len() {
        return None;
    }
    let (inner_parameters, _) = function_contract_parts(outer_result)?;
    let parameters = inner_parameters
        .iter()
        .map(|parameter| match &parameter.value {
            ExprKind::Variable(name) => witnesses.get(&name.value).copied(),
            _ => None,
        })
        .collect();
    Some(InterpreterSyntaxPlan {
        witness_count: outer_parameters.len(),
        parameters,
    })
}

fn function_contract_parts(contract: &Expr) -> Option<(&[Expr], &Expr)> {
    let ExprKind::Call { callee, arguments } = &contract.value else {
        return None;
    };
    if !is_variable(callee, "Func") {
        return None;
    }
    let [parameters, result] = arguments.as_slice() else {
        return None;
    };
    let ExprKind::Array(parameters) = &parameters.value else {
        return None;
    };
    Some((parameters, result))
}

fn is_variable(expression: &Expr, expected: &str) -> bool {
    matches!(&expression.value, ExprKind::Variable(name) if name.value == expected)
}

fn interpreter_expansion(operand: Expr, location: Location, plan: &InterpreterSyntaxPlan) -> Expr {
    let variable = |name: &str| {
        located(
            ExprKind::Variable(located(name.to_owned(), location)),
            location,
        )
    };
    let pack = |witness_index: usize, value_name: &str| {
        located(
            ExprKind::Call {
                callee: Box::new(variable("\0telora_pack_dyn")),
                arguments: vec![
                    variable(&format!("\0telora_interpreter_type_{witness_index}")),
                    variable(value_name),
                ],
            },
            location,
        )
    };
    let value_names = (0..plan.parameters.len())
        .map(|index| format!("\0telora_interpreter_value_{index}"))
        .collect::<Vec<_>>();
    let call = located(
        ExprKind::Call {
            callee: Box::new(operand),
            arguments: plan
                .parameters
                .iter()
                .zip(&value_names)
                .map(|(witness, value)| {
                    witness.map_or_else(|| variable(value), |index| pack(index, value))
                })
                .collect(),
        },
        location,
    );
    let parameter = |name: &str| ClosureParameter {
        name: located(name.to_owned(), location),
        annotation: None,
    };
    let inner = located(
        ExprKind::Closure {
            parameters: value_names.iter().map(|name| parameter(name)).collect(),
            result_annotation: None,
            body: located(
                BlockKind {
                    bindings: Vec::new(),
                    result: Box::new(call),
                },
                location,
            ),
        },
        location,
    );
    located(
        ExprKind::Closure {
            parameters: (0..plan.witness_count)
                .map(|index| parameter(&format!("\0telora_interpreter_type_{index}")))
                .collect(),
            result_annotation: None,
            body: located(
                BlockKind {
                    bindings: Vec::new(),
                    result: Box::new(inner),
                },
                location,
            ),
        },
        location,
    )
}

fn placeholder_variable(index: usize, location: Location) -> Expr {
    located(
        ExprKind::Variable(located(placeholder_parameter(index), location)),
        location,
    )
}

