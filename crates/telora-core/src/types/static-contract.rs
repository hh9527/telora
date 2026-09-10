// Static type syntax elaboration. This API deliberately has no evaluator, heap,
// runtime metadata or value bindings. Unsupported forms are a migration boundary,
// not permission to interpret arbitrary expressions as types.
struct StaticContractScope<'a> {
    hir: &'a HirProgram,
    environment: &'a HashMap<String, TypeDescriptor>,
    external_names: &'a HashSet<&'a str>,
    interfaces: &'a BTreeMap<String, ModuleInterface>,
    parameters: &'a [TypeParameter],
    families: &'a BTreeMap<String, StaticTypeFamily>,
}

// Visit every sibling even when an earlier edge is unknown or conflicted.
// Option's FromIterator would stop before discovering the remaining evidence.
fn collect_static_results<T, C: FromIterator<T>>(
    items: impl Iterator<Item = Option<T>>,
) -> Option<C> {
    let mut complete = true;
    let result = items.filter_map(|item| {
        complete &= item.is_some();
        item
    }).collect();
    complete.then_some(result)
}

impl StaticContractScope<'_> {
    fn unknown_diagnostic(&self, expression: &Expr) -> Diagnostic {
        if let Some(reference) = self.hir.references().iter().find(|reference| {
            reference.resolution.is_unresolved()
                && reference.location.source == expression.location.source
                && expression.location.start <= reference.location.start
                && reference.location.end <= expression.location.end
        }) {
            self.hir.resolution_diagnostic(reference)
        } else {
            Diagnostic::error("type remains unknown after static solving", expression.location)
        }
    }

    fn family_conflict(&self, expression: &Expr, callee: &Expr, message: impl Into<String>) -> Diagnostic {
        let diagnostic = Diagnostic::error(message, expression.location);
        let ExprKind::Variable(name) = &callee.value else { return diagnostic; };
        let Some(HirResolution::Definition(id)) = self.hir
            .reference_at(name.location, &name.value).map(|reference| reference.resolution)
            else { return diagnostic; };
        let Some(definition) = self.hir.definition(id) else { return diagnostic; };
        diagnostic.with_secondary("type family declared here", definition.location)
    }

    fn member_import(&self, expression: &Expr) -> Option<&Expr> {
        let ExprKind::Variable(name) = &expression.value else { return None; };
        let HirResolution::Definition(id) = self.hir
            .reference_at(name.location, &name.value)?.resolution else { return None; };
        self.hir.definition(id)?.member_import.as_ref()
    }

    fn family_name(&self, expression: &Expr) -> Option<String> {
        if let Some(import) = self.member_import(expression) {
            return self.family_name(import);
        }
        match &expression.value {
            ExprKind::Variable(name) => {
                if self
                    .parameters
                    .iter()
                    .any(|parameter| parameter.name == name.value)
                    || matches!(self.hir.reference_at(name.location, &name.value)
                        .map(|reference| reference.resolution), Some(HirResolution::Definition(id))
                        if self.hir.definition(id).is_none_or(|definition|
                            !matches!(definition.kind, HirDefinitionKind::Type | HirDefinitionKind::Import)))
                {
                    return None;
                }
                Some(name.value.clone())
            }
            ExprKind::Field { receiver, field } => {
                self.namespace(receiver)?;
                Some(format!("{}.{}", self.family_name(receiver)?, field.value))
            }
            _ => None,
        }
    }

    fn namespace(&self, expression: &Expr) -> Option<&ModuleInterface> {
        if let Some(import) = self.member_import(expression) {
            return self.namespace(import);
        }
        match &expression.value {
            ExprKind::Variable(name) => {
                if self
                    .parameters
                    .iter()
                    .any(|parameter| parameter.name == name.value)
                    || matches!(
                        self.hir
                            .reference_at(name.location, &name.value)
                            .map(|reference| reference.resolution),
                        Some(HirResolution::Definition(id))
                            if self.hir.definition(id).is_none_or(|definition|
                                definition.kind != HirDefinitionKind::Import)
                    )
                {
                    return None;
                }
                self.interfaces
                    .get(&name.value)
                    .filter(|interface| interface.value_binding.is_none())
            }
            ExprKind::Field { receiver, field } => {
                self.namespace(receiver)?.namespaces.get(&field.value)
            }
            _ => None,
        }
    }

    fn builtin<'a>(&self, expression: &'a Expr) -> Option<&'a str> {
        let ExprKind::Variable(name) = &expression.value else {
            return None;
        };
        if name.value.starts_with('\0') {
            return Some(&name.value);
        }
        if self.external_names.contains(name.value.as_str())
            || self
                .parameters
                .iter()
                .any(|parameter| parameter.name == name.value)
            || matches!(
                self.hir
                    .reference_at(name.location, &name.value)
                    .map(|reference| reference.resolution),
                Some(HirResolution::Definition(_))
            )
        {
            return None;
        }
        Some(&name.value)
    }

    fn elaborate(&self, expression: &Expr, graph: &mut TypeGraph) -> Option<AnalysisTypeId> {
        if let Some(import) = self.member_import(expression) {
            return self.elaborate(import, graph);
        }
        let node = match &expression.value {
            ExprKind::TypeSyntax(inner) => return self.elaborate(inner, graph),
            ExprKind::Field { receiver, field } => {
                let interface = self.namespace(receiver)?;
                if !interface.type_declarations.contains(&field.value) {
                    return None;
                }
                let scheme = interface.exports.get(&field.value)?;
                if !scheme.parameters.is_empty() || !scheme.constraints.is_empty() {
                    return None;
                }
                let TypeDescriptor::TypeOf(instance) = &scheme.body else {
                    return None;
                };
                if contains_type_variable(instance) {
                    return None;
                }
                return Some(graph.intern_descriptor(instance));
            }
            ExprKind::Variable(name) => {
                if let Some(parameter) = self
                    .parameters
                    .iter()
                    .find(|parameter| parameter.name == name.value)
                {
                    TypeNode::Bound(parameter.id)
                } else {
                    let imported = self.hir.reference_at(name.location, &name.value)
                        .is_some_and(|reference| matches!(reference.resolution,
                            HirResolution::Definition(id) if self.hir.definition(id)
                                .is_some_and(|definition| definition.kind == HirDefinitionKind::Import)));
                    let descriptor = if imported {
                        let interface = self.interfaces.get(&name.value)?;
                        if !interface.type_declarations.contains(interface.value_binding.as_ref()?) {
                            return None;
                        }
                        let scheme = interface.binding_scheme()?;
                        if !scheme.parameters.is_empty() || !scheme.constraints.is_empty() {
                            return None;
                        }
                        &scheme.body
                    } else {
                        self.environment.get(&name.value)?
                    };
                    let TypeDescriptor::TypeOf(instance) = descriptor
                    else {
                        return None;
                    };
                    if contains_type_variable(instance) {
                        return None;
                    }
                    return Some(graph.intern_descriptor(instance));
                }
            }
            ExprKind::Call { callee, arguments } => {
                if let Some(family) = self
                    .family_name(callee)
                    .and_then(|name| self.families.get(&name))
                {
                    let solved_arguments: Option<Vec<_>> = collect_static_results(arguments
                        .iter().map(|argument| self.elaborate(argument, graph)));
                    if arguments.len() != family.arity {
                        graph.elaboration_conflicts.push(self.family_conflict(expression, callee,
                            format!("expected {} arguments, got {}", family.arity, arguments.len()),
                        ));
                        return None;
                    }
                    let arguments = solved_arguments?;
                    if family.recursive_pending {
                        let unchanged = arguments.iter().enumerate().all(|(index, argument)|
                            matches!(graph.node(*argument), TypeNode::Bound(parameter) if parameter.0 as usize == index));
                        if !unchanged {
                            graph.elaboration_conflicts.push(self.family_conflict(expression, callee,
                                "recursive type-family application must use its bound parameters unchanged and in declaration order",
                            ));
                        }
                        return unchanged.then_some(family.root);
                    }
                    return Some(graph.apply_static_family(family.root, &arguments));
                }
                let builtin = self.builtin(callee)?;
                let arity = match builtin {
                    "Array" | "Dict" | "TypeOf" | "Unchecked" | "Option" | "Tuple"
                        | "\0telora_tuple_type" => Some(1),
                    "Result" | "FoldControl" | "Func" | "\0telora_function_type"
                        | "\0telora_struct" | "\0telora_enum" | "\0telora_newtype" => Some(2),
                    _ => None,
                };
                if let Some(arity) = arity.filter(|arity| *arity != arguments.len()) {
                    graph.elaboration_conflicts.push(self.family_conflict(expression, callee,
                        format!("expected {arity} arguments, got {}", arguments.len())));
                    return None;
                }
                match (builtin, arguments.as_slice()) {
                    ("\0telora_struct", [_, members]) => {
                        let ExprKind::Dict(members) = &members.value else {
                            return None;
                        };
                        TypeNode::Struct(
                            collect_static_results(members
                                .iter()
                                .map(|member| {
                                    Some((
                                        member.value.name.as_ref()?.value.clone(),
                                        self.elaborate(&member.value.value, graph)?,
                                    ))
                                }))?,
                        )
                    }
                    ("\0telora_enum", [_, members]) => {
                        let ExprKind::Dict(members) = &members.value else {
                            return None;
                        };
                        TypeNode::Enum(collect_static_results(members.iter().map(|member| {
                            let payload = if matches!(&member.value.value.value, ExprKind::Atom(name) if name == "None") {
                                None
                            } else { Some(self.elaborate(&member.value.value, graph)?) };
                            Some((member.value.name.as_ref()?.value.clone(), payload))
                        }))?)
                    }
                    ("\0telora_newtype", [_, members]) => {
                        let ExprKind::Dict(members) = &members.value else {
                            return None;
                        };
                        let [payload] = members.as_slice() else {
                            return None;
                        };
                        TypeNode::Newtype(self.elaborate(&payload.value.value, graph)?)
                    }
                    ("Array", [item]) => TypeNode::Array(self.elaborate(item, graph)?),
                    ("Dict", [item]) => TypeNode::Dict(self.elaborate(item, graph)?),
                    ("TypeOf", [item]) => TypeNode::TypeOf(self.elaborate(item, graph)?),
                    ("Unchecked", [item]) => {
                        let root = self.elaborate(item, graph)?;
                        let target = graph.descriptor(root).ok()?;
                        if !matches!(&target, TypeDescriptor::Declared(declared)
                            if matches!(declared.body.as_ref(), TypeDescriptor::Struct(_))) {
                            graph.elaboration_conflicts.push(Diagnostic::error(
                                "Unchecked expects a named-field struct type", expression.location));
                            return None;
                        }
                        return Some(graph.intern_descriptor(&unchecked_descriptor(target)));
                    }
                    ("Option", [item]) => TypeNode::Enum(BTreeMap::from([
                        ("None".into(), None),
                        ("Some".into(), Some(self.elaborate(item, graph)?)),
                    ])),
                    ("Result", [ok, error]) => {
                        let ok = self.elaborate(ok, graph);
                        let error = self.elaborate(error, graph);
                        TypeNode::Enum(BTreeMap::from([
                            ("Err".into(), Some(error?)), ("Ok".into(), Some(ok?)),
                        ]))
                    }
                    ("FoldControl", [state, result]) => {
                        let state = self.elaborate(state, graph);
                        let result = self.elaborate(result, graph);
                        TypeNode::Enum(BTreeMap::from([
                            ("Break".into(), Some(result?)), ("Continue".into(), Some(state?)),
                        ]))
                    }
                    ("Tuple" | "\0telora_tuple_type", [items]) => {
                        let ExprKind::Array(items) = &items.value else {
                            return None;
                        };
                        TypeNode::Tuple(
                            collect_static_results(items
                                .iter()
                                .map(|item| self.elaborate(item, graph)))?,
                        )
                    }
                    ("Func" | "\0telora_function_type", [parameters, result]) => {
                        let ExprKind::Array(parameters) = &parameters.value else {
                            return None;
                        };
                        let parameters = collect_static_results(parameters
                                .iter()
                                .map(|item| self.elaborate(item, graph)));
                        let result = self.elaborate(result, graph);
                        TypeNode::Function { parameters: parameters?, result: result? }
                    }
                    _ => return None,
                }
            }
            _ => return None,
        };
        Some(graph.intern_node(node))
    }
}
