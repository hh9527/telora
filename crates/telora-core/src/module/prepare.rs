// Dependency preparation may recurse and execute legacy module values. Keep it
// separate from compilation, which borrows the session's syntax.
struct PreparedDependencies {
    external_provenance: BTreeMap<String, Provenance>,
    external_roots: HashMap<String, PersistentValue>,
    semantic_imports: Vec<SemanticImport>,
    external_interfaces: BTreeMap<String, ModuleInterface>,
    open_candidates: BTreeMap<String, Vec<OpenImportCandidate>>,
}

impl ModuleLoader {
    fn prepare_telora_dependencies(
        &mut self,
        module_id: &ModuleCName,
        external_bindings: &BTreeMap<String, crate::DataWorld>,
        skeleton: ModuleId,
    ) -> Result<PreparedDependencies, ModuleError> {
        let source_name = module_id.to_string();
        let mut external_provenance = BTreeMap::new();
        let mut external_roots = HashMap::new();
        let mut external_interfaces = BTreeMap::new();
        for (name, value) in external_bindings {
            let interface = value.static_interface(name).ok_or_else(|| ModuleError::new(
                format!("Host binding {name:?} requires an explicit type interface"),
            ))?;
            external_interfaces.insert(name.clone(), interface);
            let root = value
                .publish(&mut self.main.heap)
                .map_err(|error| ModuleError::new(error.to_string()))?;
            external_roots.insert(name.clone(), root);
        }
        let mut semantic_imports = Vec::new();
        let mut open_candidates: BTreeMap<String, Vec<OpenImportCandidate>> = BTreeMap::new();
        let mut direct_import_names = external_bindings.keys().cloned().collect::<HashSet<_>>();

        let mut next_binding = 0;
        let mut imports_fmt = false;
        loop {
            // Keep only the import operands across recursive loading. The syntax
            // remains owned by the session, with no AST clone or retained borrow.
            let (kind, name, imported_name, relative, location) = {
                let prepared = self.main.modules.module(skeleton).prepared.as_ref();
                let program = prepared
                    .expect("session syntax")
                    .program
                    .as_ref()
                    .expect("validated program");
                let Some((index, binding)) = program
                    .value
                    .body
                    .value
                    .bindings
                    .iter()
                    .enumerate()
                    .skip(next_binding)
                    .find(|(_, binding)| {
                        matches!(
                            binding.value.kind,
                            BindingKind::Import | BindingKind::OpenImport
                        )
                    })
                else {
                    break;
                };
                next_binding = index + 1;
                let ExprKind::String(relative) = &binding.value.value.value else {
                    return Err(ModuleError::new("import path must be a string"));
                };
                (
                    binding.value.kind,
                    binding.value.name.clone(),
                    binding.value.imported_name.clone(),
                    relative.clone(),
                    binding.value.value.location,
                )
            };
            imports_fmt |= relative == FMT_MODULE;
            if kind == BindingKind::Import && !direct_import_names.insert(name.value.clone()) {
                return Err(ModuleError::new(format!(
                    "duplicate module binding {:?} in {source_name}",
                    name.value
                )));
            }
            let imported = self
                .main
                .modules
                .resolve_import(location)
                .map_err(|error| {
                    ModuleError::new(
                        self.sources
                            .render(&Diagnostic::error(error.to_string(), location)),
                    )
                })?;
            if imported.vendor == ModuleVendor::Builtin {
                let module = self.load_native_module(&relative).map_err(|error| {
                    ModuleError::new(
                        self.sources
                            .render(&Diagnostic::error(error.to_string(), location)),
                    )
                })?;
                self.install_trait_impl_roots(&module, &mut external_roots)?;
                self.install_type_property_roots(&module, &mut external_roots)?;
                semantic_imports.push(SemanticImport {
                    name: if kind == BindingKind::OpenImport {
                        "*".into()
                    } else {
                        name.value.clone()
                    },
                    location: name.location,
                    target: imported.id.clone(),
                    namespace: kind != BindingKind::OpenImport && imported_name.is_none(),
                });
                if kind == BindingKind::OpenImport {
                    for (name, candidate) in open_import_exports(
                        &imported.id,
                        module.root,
                        &module.interface,
                        &self.main.heap,
                        module.provenance.as_ref(),
                    )? {
                        open_candidates.entry(name).or_default().push(candidate);
                    }
                    continue;
                }
                let (selected_root, interface) = select_import_root(
                    module.root,
                    module.interface,
                    imported_name.as_deref(),
                    &name.value,
                    &self.main.heap,
                )?;
                external_roots.insert(name.value.clone(), selected_root);
                external_interfaces.insert(name.value.clone(), interface);
                continue;
            }
            let imported_id = imported.id.clone();
            let artifact = self.load_resolved_value(imported)?;
            self.install_trait_impl_roots(&artifact, &mut external_roots)?;
            self.install_type_property_roots(&artifact, &mut external_roots)?;
            semantic_imports.push(SemanticImport {
                name: if kind == BindingKind::OpenImport {
                    "*".into()
                } else {
                    name.value.clone()
                },
                location: name.location,
                target: imported_id.clone(),
                namespace: kind != BindingKind::OpenImport && imported_name.is_none(),
            });
            if kind == BindingKind::OpenImport {
                for (name, candidate) in open_import_exports(
                    &imported_id,
                    artifact.root,
                    &artifact.interface,
                    &self.main.heap,
                    artifact.provenance.as_ref(),
                )? {
                    open_candidates.entry(name).or_default().push(candidate);
                }
                continue;
            }
            let (selected_root, mut selected_interface) = select_import_root(
                artifact.root,
                artifact.interface,
                imported_name.as_deref(),
                &name.value,
                &self.main.heap,
            )?;
            if imported_name.is_none()
                && let Some(scheme) = artifact.root_scheme
            {
                selected_interface.value_binding = Some(name.value.clone());
                selected_interface
                    .exports
                    .insert(name.value.clone(), scheme);
            }
            external_roots.insert(name.value.clone(), selected_root);
            external_interfaces.insert(name.value.clone(), selected_interface);
            if let Some(provenance) = artifact.provenance
                && !provenance.values.is_empty()
            {
                external_provenance.insert(name.value.clone(), provenance);
            }
        }
        if module_id.to_string() != PRELUDE_MODULE
            && let Some(module) = self.builtin_modules.get(PRELUDE_MODULE)
        {
            self.install_trait_impl_roots(module, &mut external_roots)?;
            self.install_type_property_roots(module, &mut external_roots)?;
            let provider = ModuleCName::Builtin(PRELUDE_MODULE.into());
            for (name, candidate) in open_import_exports(
                &provider,
                module.root,
                &module.interface,
                &self.main.heap,
                module.provenance.as_ref(),
            )? {
                open_candidates
                    .entry(name)
                    .or_insert_with(|| vec![candidate]);
            }
        }
        if !matches!(module_id, ModuleCName::Builtin(_))
            && !imports_fmt
            && let Some(module) = self.builtin_modules.get(FMT_MODULE)
        {
            self.install_trait_impl_roots(module, &mut external_roots)?;
            external_interfaces.insert(FMT_CAPABILITY_BINDING.into(), module.interface.clone());
        }
        Ok(PreparedDependencies {
            external_provenance,
            external_roots,
            semantic_imports,
            external_interfaces,
            open_candidates,
        })
    }
}
