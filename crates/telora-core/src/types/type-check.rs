// Static module preparation for the types-only entry. Runtime values and heaps
// are intentionally absent; execution consumers will use the resulting interface.
#[derive(Debug)]
pub(crate) struct CheckedModuleTypes {
    pub(crate) interface: ModuleInterface,
    pub(crate) types: TypeGraph,
}

pub(crate) fn check_module_types(
    module_id: crate::ModuleId,
    sources: &SourceDatabase,
    source_id: crate::SourceId,
    program: &Program,
    hir: HirProgram,
    imported_types: HashMap<String, TypeDescriptor>,
    mut interfaces: BTreeMap<String, ModuleInterface>,
    dependency_facts: &[ModuleTypeFacts<'_>],
    type_store: &mut TypeStore,
) -> Result<CheckedModuleTypes, FrontendError> {
    let source_name = &sources.get(source_id).name;
    for (name, body) in imported_types {
        interfaces.insert(name.clone(), ModuleInterface {
            value_binding: Some(name.clone()),
            exports: BTreeMap::from([(name, TypeScheme {
                parameters: Vec::new(), constraints: Vec::new(), body,
            })]),
            ..Default::default()
        });
    }
    let names = interfaces.keys().cloned().collect();
    let context = if source_name.starts_with("std/") {
        ModuleAnalysisContext::Builtin { defines_display_trait: source_name.as_ref() == "std/fmt" }
    } else { ModuleAnalysisContext::Ordinary };
    let solved = solve_module_plan(source_name, module_id, context, program, &hir, &names,
        sources, &BTreeMap::new(), &interfaces, dependency_facts, None, type_store)?;
    Ok(CheckedModuleTypes { interface: solved.module_interface, types: solved.types })
}

#[cfg(test)]
mod static_check_tests {
    use super::*;

    #[test]
    fn exports_builtin_type_aliases() {
        let mut sources = SourceDatabase::default();
        let id = sources.add("type-alias", "export { Type as TypeDesc };");
        let parsed = parse_registered(&sources, id);
        let interface = check_module_types(
            crate::ModuleId::ANONYMOUS,
            &sources,
            id,
            parsed.program.as_ref().unwrap(),
            resolve_module_hir(parsed.program.as_ref().unwrap(), &BTreeSet::new(), HashSet::new()),
            HashMap::new(),
            BTreeMap::new(),
            &[],
            &mut TypeStore::default(),
        )
        .unwrap().interface;
        assert!(
            interface.type_declarations.contains("TypeDesc"),
            "{interface:?}"
        );
    }

    #[test]
    fn checks_values_without_executing_them() {
        for (source, valid) in [
            ("let x = 1 / 0; export {x};", true),
            ("let x: Int = \"wrong\"; export {x};", false),
            ("def f = fn(x: Int) { x + \"wrong\" }; export {f};", false),
            ("if Bool.True { 1 } else { \"wrong\" }", false),
            ("do { if Bool.True { 1 } else { \"wrong\" }; () }", false),
        ] {
            let mut sources = SourceDatabase::default();
            let id = sources.add("static-check", source);
            let parsed = parse_registered(&sources, id);
            assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
            let result = check_module_types(
                crate::ModuleId::ANONYMOUS,
                &sources,
                id,
                parsed.program.as_ref().unwrap(),
                resolve_module_hir(parsed.program.as_ref().unwrap(), &BTreeSet::new(), HashSet::new()),
                HashMap::new(),
                BTreeMap::new(),
                &[],
                &mut TypeStore::default(),
            );
            assert_eq!(result.is_ok(), valid, "{source}: {result:?}");
        }
    }
}
