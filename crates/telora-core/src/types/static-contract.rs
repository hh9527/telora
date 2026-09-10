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

#[cfg(test)]
mod static_contract_tests {
    use super::*;

    #[test]
    fn selected_import_contract_uses_only_the_static_interface() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("contract", "import \"pkg\" { Item as Renamed }; decl f: Fn(Dict(Renamed)) -> (); ");
        let program = parse_registered(&sources, source).program.unwrap();
        let environment = BootstrapPrelude::new().types;
        let hir = HirProgram::resolve(&program, environment.keys().cloned());
        let annotation = program.value.body.value.bindings.last().unwrap()
            .value.annotation.as_ref().unwrap();
        let mut interfaces = BTreeMap::from([("Renamed".into(), ModuleInterface {
            value_binding: Some("Item".into()),
            type_declarations: BTreeSet::from(["Item".into()]),
            exports: BTreeMap::from([("Item".into(), TypeScheme {
                parameters: Vec::new(), constraints: Vec::new(),
                body: TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Int)),
            })]),
            ..Default::default()
        })]);
        let external_names = HashSet::from(["Renamed"]);
        let families = BTreeMap::new();
        let mut graph = TypeGraph::default();
        let scope = StaticContractScope {
            hir: &hir, environment: &environment, external_names: &external_names,
            interfaces: &interfaces, parameters: &[], families: &families,
        };
        let root = scope.elaborate(annotation, &mut graph).unwrap();
        assert_eq!(graph.descriptor(root).unwrap(), TypeDescriptor::Function {
            parameters: vec![TypeDescriptor::Dict(Box::new(TypeDescriptor::Int))],
            result: Box::new(TypeDescriptor::Tuple(Vec::new())),
        });
        interfaces.get_mut("Renamed").unwrap().type_declarations.clear();
        let scope = StaticContractScope {
            hir: &hir, environment: &environment, external_names: &external_names,
            interfaces: &interfaces, parameters: &[], families: &families,
        };
        assert!(scope.elaborate(annotation, &mut graph).is_none(),
            "imported metadata data does not establish a type declaration");
    }

    #[test]
    fn recursive_nominal_bodies_require_no_execution_fuel() {
        for source in [
            "type Node = struct {children: Array(Node)};",
            "type Left = struct {children: Array(Right)}; type Right = struct {children: Array(Left)};",
        ] {
            analyze_source_with_fuel("static-recursion", source, 0).unwrap();
        }
    }

    #[test]
    fn static_contract_errors_precede_construction_value_preparation() {
        let source = "def prepare: Fn() -> Never = fn() { panic!(\"check dependency executed too early\") }; let validator = prepare(); @check(validator) type Number = struct(Int); decl run: Fn(Int) -> Int; decl run: Fn(Int) -> Int;";
        let error = analyze_source_with_fuel("static-before-values", source, 0).unwrap_err();
        assert!(error.message.contains("duplicate declaration"), "{error}");
        assert!(!error.message.contains("fuel"), "{error}");
    }

    #[test]
    fn declared_bodies_elaborate_without_evaluating_model_constructors() {
        let bound = TypeDescriptor::Bound(TypeParameterId(0));
        for (source_text, expected) in [
            (
                "type Shape(T) = struct {item: T};",
                TypeDescriptor::Struct(BTreeMap::from([("item".into(), bound.clone())])),
            ),
            (
                "type Shape(T) = enum {Empty, Item(T)};",
                TypeDescriptor::Enum(BTreeMap::from([
                    ("Empty".into(), None),
                    ("Item".into(), Some(Box::new(bound.clone()))),
                ])),
            ),
            (
                "type Shape(T) = struct(T);",
                TypeDescriptor::Newtype(Box::new(bound)),
            ),
        ] {
            let mut sources = SourceDatabase::default();
            let source = sources.add("body", source_text);
            let program = parse_registered(&sources, source).program.unwrap();
            let binding = &program.value.body.value.bindings[0];
            let parameters = static_contract_parameters(binding, &sources).unwrap();
            let environment = BootstrapPrelude::new().types;
            let hir = HirProgram::resolve(&program, environment.keys().cloned());
            let scope = StaticContractScope {
                hir: &hir,
                environment: &environment,
                external_names: &HashSet::new(),
                interfaces: &BTreeMap::new(),
                parameters: &parameters,
                families: &BTreeMap::new(),
            };
            let mut graph = TypeGraph::default();
            let root = scope.elaborate(&binding.value.value, &mut graph).unwrap();
            assert_eq!(graph.descriptor(root).unwrap(), expected, "{source_text}");
        }
    }

    #[test]
    fn qualified_contract_uses_only_declared_namespace_types() {
        let mut sources = SourceDatabase::default();
        let source = sources.add(
            "contract",
            "import \"./pkg\" as pkg; export def f: Fn(pkg.inner.Item) -> () = fn(x) { () };",
        );
        let program = parse_registered(&sources, source).program.unwrap();
        let environment = BootstrapPrelude::new().types;
        let hir = HirProgram::resolve(&program, environment.keys().cloned().chain(["pkg".into()]));
        let annotation = program.value.body.value.bindings[1]
            .value
            .annotation
            .as_ref()
            .unwrap();
        let leaf = ModuleInterface {
            type_declarations: BTreeSet::from(["Item".into()]),
            exports: BTreeMap::from([(
                "Item".into(),
                TypeScheme {
                    parameters: Vec::new(),
                    constraints: Vec::new(),
                    body: TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Int)),
                },
            )]),
            ..Default::default()
        };
        let mut interfaces = BTreeMap::from([(
            "pkg".into(),
            ModuleInterface {
                namespaces: BTreeMap::from([("inner".into(), leaf)]),
                ..Default::default()
            },
        )]);
        let external_names = HashSet::from(["pkg"]);
        let mut graph = TypeGraph::default();
        let scope = StaticContractScope {
            hir: &hir,
            environment: &environment,
            external_names: &external_names,
            interfaces: &interfaces,
            parameters: &[],
            families: &BTreeMap::new(),
        };
        let root = scope.elaborate(annotation, &mut graph).unwrap();
        assert_eq!(
            graph.descriptor(root).unwrap(),
            TypeDescriptor::Function {
                parameters: vec![TypeDescriptor::Int],
                result: Box::new(TypeDescriptor::Tuple(Vec::new())),
            }
        );
        let parameters = [TypeParameter {
            id: TypeParameterId(0),
            name: "pkg".into(),
            location: annotation.location,
        }];
        let shadowed = StaticContractScope {
            parameters: &parameters,
            ..scope
        };
        assert!(shadowed.elaborate(annotation, &mut graph).is_none());
        interfaces
            .get_mut("pkg")
            .unwrap()
            .namespaces
            .get_mut("inner")
            .unwrap()
            .type_declarations
            .clear();
        let scope = StaticContractScope {
            hir: &hir,
            environment: &environment,
            external_names: &external_names,
            interfaces: &interfaces,
            parameters: &[],
            families: &BTreeMap::new(),
        };
        assert!(
            scope.elaborate(annotation, &mut graph).is_none(),
            "runtime metadata values are not type declarations"
        );
    }

    #[test]
    fn later_nominal_body_refines_existing_contract_references() {
        let identity = crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 43);
        let stub = TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: identity.clone(),
            name: "Item".into(),
            body: Arc::new(TypeDescriptor::Never),
        });
        let mut graph = TypeGraph::default();
        let slot = graph.intern_descriptor(&stub);
        let array = graph.intern_node(TypeNode::Array(slot));
        let full = TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: identity,
            name: "Item".into(),
            body: Arc::new(TypeDescriptor::Struct(BTreeMap::from([(
                "value".into(),
                TypeDescriptor::Int,
            )]))),
        });
        assert_eq!(graph.intern_descriptor(&full), slot);
        assert_eq!(
            graph.descriptor(array).unwrap(),
            TypeDescriptor::Array(Box::new(full.clone()))
        );
        assert_eq!(graph.intern_descriptor(&full), slot);
        assert_eq!(graph.intern_node(TypeNode::Array(slot)), array);
    }

    #[test]
    fn shared_structure_can_recur_through_a_nominal_boundary() {
        let mut graph = TypeGraph::default();
        let declaration = graph.push(TypeNode::Pending);
        let items = graph.intern_node(TypeNode::Array(declaration));
        let body = graph.intern_node(TypeNode::Struct(BTreeMap::from([(
            "children".into(),
            items,
        )])));
        let identity = crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 42);
        graph.nodes[declaration.index()] = TypeNode::Declared {
            id: identity.clone(),
            name: "Tree".into(),
            body,
        };
        let TypeDescriptor::Array(tree) = graph.descriptor(items).unwrap() else {
            panic!("array root");
        };
        let TypeDescriptor::Declared(tree) = *tree else {
            panic!("nominal element");
        };
        assert_eq!(tree.id, identity);
        let TypeDescriptor::Struct(fields) = tree.body.as_ref() else {
            panic!("nominal body");
        };
        let TypeDescriptor::Array(children) = &fields["children"] else {
            panic!("recursive array");
        };
        let TypeDescriptor::Declared(child) = children.as_ref() else {
            panic!("nominal recursion");
        };
        assert_eq!(child.id, identity);
        assert_eq!(child.body.as_ref(), &TypeDescriptor::Never);
    }

    #[test]
    fn structural_cycle_without_nominal_boundary_still_fails() {
        let mut graph = TypeGraph::default();
        let root = graph.push(TypeNode::Pending);
        graph.nodes[root.index()] = TypeNode::Array(root);
        assert_eq!(
            graph.descriptor(root).unwrap_err(),
            "recursive structural type has no nominal identity"
        );
    }

    #[test]
    fn elaborates_generic_function_tuple_and_unit_without_runtime_inputs() {
        let mut sources = SourceDatabase::default();
        let source = sources.add(
            "contract",
            "export def f: for(T) Fn((T, Int), Array(T)) -> () = fn(pair, items) { () };",
        );
        let program = parse_registered(&sources, source).program.unwrap();
        let binding = &program.value.body.value.bindings[0];
        let environment = BootstrapPrelude::new().types;
        let hir = HirProgram::resolve(&program, environment.keys().cloned());
        let parameters = [TypeParameter {
            id: TypeParameterId(0),
            name: "T".into(),
            location: binding.location,
        }];
        let external_names = HashSet::new();
        let scope = StaticContractScope {
            hir: &hir,
            environment: &environment,
            external_names: &external_names,
            interfaces: &BTreeMap::new(),
            parameters: &parameters,
            families: &BTreeMap::new(),
        };
        let mut graph = TypeGraph::default();
        let root = scope
            .elaborate(binding.value.annotation.as_ref().unwrap(), &mut graph)
            .unwrap();
        assert_eq!(
            graph.descriptor(root).unwrap(),
            TypeDescriptor::Function {
                parameters: vec![
                    TypeDescriptor::Tuple(vec![
                        TypeDescriptor::Bound(TypeParameterId(0)),
                        TypeDescriptor::Int
                    ]),
                    TypeDescriptor::Array(Box::new(TypeDescriptor::Bound(TypeParameterId(0))))
                ],
                result: Box::new(TypeDescriptor::Tuple(Vec::new())),
            }
        );
        let TypeNode::Function { parameters, .. } = graph.node(root) else {
            panic!("function contract");
        };
        let TypeNode::Tuple(pair) = graph.node(parameters[0]) else {
            panic!("tuple input");
        };
        let TypeNode::Array(item) = graph.node(parameters[1]) else {
            panic!("array input");
        };
        assert_eq!(
            pair[0], *item,
            "both uses refer to the same symbolic parameter"
        );
    }

    #[test]
    fn does_not_mistake_a_shadowed_container_family_for_a_builtin() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("contract", "type Array(T) = struct {value: T}; export def f: Fn(Array(Int)) -> Int = fn(x) { x.value };");
        let program = parse_registered(&sources, source).program.unwrap();
        let environment = BootstrapPrelude::new().types;
        let hir = HirProgram::resolve(&program, environment.keys().cloned());
        let external_names = HashSet::new();
        let scope = StaticContractScope {
            hir: &hir,
            environment: &environment,
            external_names: &external_names,
            interfaces: &BTreeMap::new(),
            parameters: &[],
            families: &BTreeMap::new(),
        };
        let binding = program
            .value
            .body
            .value
            .bindings
            .iter()
            .find(|binding| binding.value.name.value == "f")
            .unwrap();
        assert!(
            scope
                .elaborate(
                    binding.value.annotation.as_ref().unwrap(),
                    &mut TypeGraph::default()
                )
                .is_none()
        );
    }
}
