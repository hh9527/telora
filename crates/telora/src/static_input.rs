//! Workspace input for the three MIR passes. No legacy module/symbol/type resolver.
use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};
use telora_core::{
    ModuleFormat, ResolvedWorkspace,
    mir::{Mir, ModuleKind},
    module_resolve::{self, ModuleSpec},
    static_sources::{BUILTINS, native_module},
};

enum Source {
    File(PathBuf),
    Embedded(&'static str),
    Generated(String),
}

pub fn normalize_lf(text: String) -> String {
    if !text.contains('\r') {
        return text;
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

pub fn read_limited(
    reader: impl std::io::Read,
    max_bytes: usize,
    description: &str,
) -> Result<Vec<u8>, String> {
    let max_read = u64::try_from(max_bytes)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024));
    reader
        .take(max_read)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {description}: {error}"))?;
    if bytes.len() > max_bytes {
        return Err(format!(
            "{description} exceeds file_size limit ({} > {max_bytes})",
            bytes.len()
        ));
    }
    Ok(bytes)
}

pub struct Entry {
    pub name: String,
    pub origin: &'static str,
    pub visibility: &'static str,
    pub format: ModuleFormat,
    source: Source,
    test: bool,
}
pub struct Inventory {
    pub entries: BTreeMap<String, Entry>,
    workspace: Option<Arc<ResolvedWorkspace>>,
    owner: String,
    compiler: telora_core::CompilerOptions,
    runtime: telora_core::RuntimeOptions,
    normalize_eol: bool,
    check_all_crate_modules: bool,
}

fn private(name: &str) -> bool {
    name.split('/').any(|part| part.starts_with('_'))
}

impl Inventory {
    /// Published artifacts use one textual input representation on every OS.
    pub fn normalize_eol(&mut self) {
        self.normalize_eol = true;
    }

    pub fn runtime_options(&self) -> telora_core::RuntimeOptions {
        self.runtime
    }

    /// Editor roots may be private modules. Identity still comes exclusively
    /// from the workspace catalog (or its normal test-module inventory).
    pub fn document_name(&mut self, path: &Path) -> Result<String, String> {
        let matches_path = |entry: &Entry| {
            matches!(&entry.source,
            Source::File(file) if file == path || file.canonicalize().ok().as_deref() == Some(path))
        };
        if let Some((name, _)) = self.entries.iter().find(|(_, entry)| matches_path(entry)) {
            return Ok(name.clone());
        }
        let workspace = self
            .workspace
            .as_ref()
            .cloned()
            .ok_or("editor documents require a workspace")?;
        let owner = workspace
            .crate_for_path(path)
            .map_err(|e| e.to_string())?
            .to_owned();
        let test_root = workspace
            .crate_root(&owner)
            .ok_or("missing declaring crate")?
            .join("tests");
        if path.starts_with(&test_root) {
            self.scan_tests_for(&owner, &test_root, &test_root)?;
        }
        let source_root = workspace
            .crate_root(&owner)
            .ok_or("missing declaring crate")?
            .join("src");
        if let Ok(relative) = path.strip_prefix(&source_root) {
            let mut logical = relative.to_path_buf();
            if logical.extension().and_then(|value| value.to_str()) == Some("telora") {
                logical.set_extension("");
                let logical = logical.to_string_lossy().replace('\\', "/");
                let name = if logical == "lib" {
                    owner.clone()
                } else {
                    format!("{owner}/{logical}")
                };
                if !self.entries.contains_key(&name) {
                    if let Some(declaration) = workspace
                        .discover_module(&owner, &name)
                        .map_err(|error| error.to_string())?
                    {
                        self.entries.insert(
                            name.clone(),
                            Entry {
                                name: name.clone(),
                                origin: "crate",
                                visibility: if private(&name) { "private" } else { "public" },
                                format: declaration.format,
                                source: Source::File(declaration.physical_path),
                                test: false,
                            },
                        );
                    }
                }
            }
        }
        self.entries
            .iter()
            .find(|(_, entry)| matches_path(entry))
            .map(|(name, _)| name.clone())
            .ok_or_else(|| format!("document {} is not in the module catalog", path.display()))
    }

    pub fn solve_documents(
        &mut self,
        roots: &[String],
        overlays: &BTreeMap<String, telora_core::DocumentText>,
        context: &telora_core::QueryContext,
    ) -> Result<Mir, telora_core::QueryError> {
        let mut error = None;
        let mir = self.solve_inputs_cancellable(roots, None, overlays, &mut || {
            error = context.check().err();
            error.is_some()
        });
        mir.ok_or_else(|| error.expect("interrupted query records its cause"))
    }

    pub fn workspace(&self) -> Option<Arc<ResolvedWorkspace>> {
        self.workspace.clone()
    }

    pub fn module_paths(&self) -> std::collections::HashMap<String, PathBuf> {
        self.entries
            .iter()
            .filter_map(|(name, entry)| match &entry.source {
                Source::File(path) => Some((name.clone(), path.clone())),
                _ => None,
            })
            .collect()
    }

    /// Read a catalog data module without depending on an execution linker.
    pub fn read_data_text(
        &self,
        name: &str,
        max_bytes: usize,
    ) -> Result<(telora_core::data_plan::Format, String), String> {
        let entry = self
            .entries
            .get(name)
            .ok_or("unknown data module identity")?;
        let Source::File(path) = &entry.source else {
            return Err("data module has no file source".into());
        };
        let format = match entry.format {
            ModuleFormat::Json => telora_core::data_plan::Format::Json,
            ModuleFormat::Yaml => telora_core::data_plan::Format::Yaml,
            ModuleFormat::Toml => telora_core::data_plan::Format::Toml,
            ModuleFormat::Telora => {
                return Err("source module cannot fill a data relocation".into());
            }
        };
        let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let bytes = read_limited(file, max_bytes, &path.display().to_string())?;
        let text = String::from_utf8(bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok((
            format,
            if self.normalize_eol {
                normalize_lf(text)
            } else {
                text
            },
        ))
    }
    pub fn new(context: &Path, builtin_only: bool) -> Result<Self, String> {
        let workspace = if builtin_only {
            None
        } else {
            Some(crate::package_host::prepare(context)?)
        };
        let (compiler, runtime) = if let Some(workspace) = &workspace {
            (workspace.compiler_options(), workspace.runtime_options())
        } else if let Some(config) =
            telora_core::WorkspaceConfig::discover_optional(context).map_err(|e| e.to_string())?
        {
            (config.compiler, config.runtime)
        } else {
            Default::default()
        };
        let owner = workspace
            .as_ref()
            .map(|w| {
                w.crate_for_path(context)
                    .map(str::to_owned)
                    .map_err(|e| e.to_string())
            })
            .transpose()?
            .unwrap_or_else(|| "std".into());
        let mut entries = BTreeMap::new();
        if let Some(w) = &workspace {
            for (name, _) in w.crates() {
                if name == "std" {
                    continue;
                }
                for module in w.modules(name).expect("known crate") {
                    let logical = module.logical_path.to_string_lossy().replace('\\', "/");
                    let cname = if logical == "lib" {
                        name.to_owned()
                    } else {
                        format!("{name}/{logical}")
                    };
                    entries.insert(
                        cname.clone(),
                        Entry {
                            visibility: if private(&cname) { "private" } else { "public" },
                            name: cname,
                            origin: if name == owner { "crate" } else { "dependency" },
                            format: module.format,
                            source: Source::File(module.physical_path.clone()),
                            test: false,
                        },
                    );
                }
            }
        }
        for &(name, text) in BUILTINS {
            entries.insert(
                name.into(),
                Entry {
                    name: name.into(),
                    origin: "builtin",
                    visibility: if private(name) { "private" } else { "public" },
                    format: ModuleFormat::Telora,
                    source: Source::Embedded(text),
                    test: false,
                },
            );
        }
        Ok(Self {
            entries,
            workspace,
            owner,
            compiler,
            runtime,
            normalize_eol: false,
            check_all_crate_modules: false,
        })
    }

    pub fn catalog(&self) -> impl Iterator<Item = &Entry> {
        self.entries
            .values()
            .filter(|e| !e.test && (e.origin == "crate" || e.visibility == "public"))
    }

    pub fn select(&mut self, selector: &str) -> Result<String, String> {
        let name = if let Some(path) = selector.strip_prefix("@src/") {
            if path == "lib" && self.workspace.is_some() {
                self.owner.clone()
            } else {
                format!("{}/{path}", self.owner)
            }
        } else if let Some(path) = selector.strip_prefix("@test/") {
            let root = self
                .workspace
                .as_ref()
                .and_then(|w| w.crate_root(&self.owner))
                .ok_or("test selector requires a workspace")?
                .join("tests");
            self.scan_tests(&root, &root)?;
            format!("{}/tests/{path}", self.owner)
        } else {
            selector.to_owned()
        };
        if private(&name) {
            if name.starts_with("std/") {
                return Err(format!("unknown built-in module {name:?}"));
            }
            return Err(format!(
                "private module {name:?} cannot be a query/check root"
            ));
        }
        if let Some((owner, _)) = name.split_once('/') {
            if owner != self.owner
                && owner != "std"
                && !self
                    .workspace
                    .as_ref()
                    .is_some_and(|w| w.declares_dependency(&self.owner, owner))
            {
                return Err(format!(
                    "crate {:?} does not declare dependency {owner:?}",
                    self.owner
                ));
            }
        }
        if !self.entries.contains_key(&name)
            && let Some(workspace) = &self.workspace
        {
            let target_owner = name
                .split_once('/')
                .map_or(name.as_str(), |(owner, _)| owner);
            if let Ok(Some(declaration)) = workspace.discover_module(target_owner, &name) {
                self.entries.insert(
                    name.clone(),
                    Entry {
                        visibility: if private(&name) { "private" } else { "public" },
                        name: name.clone(),
                        origin: if target_owner == self.owner {
                            "crate"
                        } else {
                            "dependency"
                        },
                        format: declaration.format,
                        source: Source::File(declaration.physical_path),
                        test: false,
                    },
                );
            }
        }
        Ok(name)
    }

    fn scan_tests(&mut self, root: &Path, path: &Path) -> Result<(), String> {
        self.scan_tests_for(&self.owner.clone(), root, path)
    }

    fn scan_tests_for(&mut self, owner: &str, root: &Path, path: &Path) -> Result<(), String> {
        let meta = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if meta.file_type().is_symlink() {
            return Err(format!(
                "test catalogs do not allow symlinks: {}",
                path.display()
            ));
        }
        if meta.is_dir() {
            let mut children = fs::read_dir(path)
                .map_err(|e| e.to_string())?
                .map(|e| e.map(|e| e.path()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            children.sort();
            for child in children {
                self.scan_tests_for(owner, root, &child)?;
            }
        } else if meta.is_file()
            && let Ok(format) = ModuleFormat::from_path(path)
        {
            let mut relative = path.strip_prefix(root).expect("test child").to_owned();
            if format == ModuleFormat::Telora {
                relative.set_extension("");
            }
            let name = format!(
                "{}/tests/{}",
                owner,
                relative.to_string_lossy().replace('\\', "/")
            );
            if self.entries.contains_key(&name) {
                return Err(format!("duplicate source/test module {name}"));
            }
            self.entries.insert(
                name.clone(),
                Entry {
                    visibility: if private(&name) { "private" } else { "public" },
                    name,
                    origin: if owner == self.owner {
                        "crate"
                    } else {
                        "dependency"
                    },
                    format,
                    source: Source::File(path.to_owned()),
                    test: true,
                },
            );
        }
        Ok(())
    }
}

fn request_name(
    entries: &BTreeMap<String, Entry>,
    workspace: Option<&ResolvedWorkspace>,
    importer: &str,
    request: &str,
) -> Option<String> {
    let owner = importer
        .split_once('/')
        .map_or(importer, |(owner, _)| owner);
    let name = if let Some(path) = request.strip_prefix("@src/") {
        format!("{owner}/{path}")
    } else if let Some(path) = request.strip_prefix("@test/") {
        format!("{owner}/tests/{path}")
    } else if request.starts_with("./") || request.starts_with("../") {
        let mut parts = importer.split('/').collect::<Vec<_>>();
        parts.pop();
        let floor = if entries.get(importer).is_some_and(|e| e.test) {
            2
        } else {
            1
        };
        for part in request.split('/') {
            match part {
                "." | "" => {}
                ".." => {
                    if parts.len() <= floor {
                        return None;
                    }
                    parts.pop();
                }
                part => parts.push(part),
            }
        }
        parts.join("/")
    } else {
        request.to_owned()
    };
    // Inventory lookup is authoritative: no file probing or alternate candidates.
    let target_owner = name
        .split_once('/')
        .map_or(name.as_str(), |(owner, _)| owner);
    let entry = entries.get(&name);
    if entry.is_some_and(|entry| entry.test) && !entries.get(importer).is_some_and(|e| e.test) {
        return None;
    }
    if target_owner != owner {
        if entry.is_some_and(|entry| entry.visibility == "private" || entry.test) {
            return None;
        }
        if target_owner != "std"
            && !workspace.is_some_and(|w| w.declares_dependency(owner, target_owner))
        {
            return None;
        }
    }
    if entry.is_none()
        && !workspace.is_some_and(|workspace| workspace.crate_root(target_owner).is_some())
    {
        return None;
    }
    Some(name)
}

impl Inventory {
    pub fn solve(&mut self, root: &str) -> Mir {
        self.solve_with_entry(root, None)
    }

    /// Batch roots belong to the current crate; dependencies join through imports.
    pub fn check_roots(&mut self, lib: bool, tests: bool) -> Result<Vec<String>, String> {
        self.check_all_crate_modules = lib;
        if tests {
            let root = self
                .workspace
                .as_ref()
                .and_then(|w| w.crate_root(&self.owner))
                .ok_or("test selection requires a workspace")?
                .join("tests");
            match fs::symlink_metadata(&root) {
                Ok(_) => self.scan_tests(&root, &root)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("{}: {error}", root.display())),
            }
        }
        Ok(self
            .entries
            .values()
            .filter(|entry| entry.origin == "crate" && if entry.test { tests } else { lib })
            .map(|entry| entry.name.clone())
            .collect())
    }

    pub fn solve_roots(&mut self, roots: &[String]) -> Mir {
        self.solve_inputs(roots, None, &BTreeMap::new())
    }

    /// Compiler-owned entry sources share the application's graph and passes.
    pub fn solve_transform(&mut self, application: &str) -> Result<Mir, String> {
        let name = "std/_entry/adapter";
        self.entries.insert(
            name.into(),
            Entry {
                name: name.into(),
                origin: "builtin",
                visibility: "private",
                format: ModuleFormat::Telora,
                source: Source::Generated(telora_core::entry_plan::transform_adapter(application)?),
                test: false,
            },
        );
        Ok(self.solve_with_entry(name, Some(application)))
    }

    fn solve_with_entry(&mut self, root: &str, application: Option<&str>) -> Mir {
        self.solve_inputs(&[root.to_owned()], application, &BTreeMap::new())
    }

    fn solve_inputs(
        &mut self,
        roots: &[String],
        application: Option<&str>,
        overlays: &BTreeMap<String, telora_core::DocumentText>,
    ) -> Mir {
        self.solve_inputs_cancellable(roots, application, overlays, &mut || false)
            .expect("uncancelled compilation")
    }

    fn solve_inputs_cancellable(
        &mut self,
        roots: &[String],
        application: Option<&str>,
        overlays: &BTreeMap<String, telora_core::DocumentText>,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> Option<Mir> {
        if cancelled() {
            return None;
        }
        let mut graph_roots = roots.to_vec();
        if !roots.is_empty()
            && self.entries.contains_key(&self.owner)
            && !graph_roots.contains(&self.owner)
        {
            graph_roots.push(self.owner.clone());
        }
        let specs = self
            .entries
            .values()
            // Embedded sources are a packaging inventory, not a semantic
            // module catalog. `std/lib.telora` is the sole std root; its
            // `mod` declarations discover the remaining embedded modules.
            .filter(|entry| {
                entry.origin != "builtin"
                    || entry.name == "std"
                    || graph_roots.contains(&entry.name)
            })
            .map(|e| ModuleSpec {
                native: if e.origin == "builtin" {
                    native_module(&e.name)
                } else {
                    None
                },
                name: e.name.clone(),
                kind: if e.format == ModuleFormat::Telora {
                    ModuleKind::Source
                } else {
                    ModuleKind::Data
                },
                implicit_imports: if e.name == "std/prelude" {
                    vec![]
                } else {
                    vec!["std/prelude".into()]
                },
            })
            .collect();
        let workspace = self.workspace.clone();
        let owner = self.owner.clone();
        let normalize_eol = self.normalize_eol;
        let entries = RefCell::new(&mut self.entries);
        let mut mir = module_resolve::resolve_with_discovery_cancellable(
            specs,
            &graph_roots,
            |_, name| {
                let text = if let Some(text) = overlays.get(name) {
                    Ok(text.to_string())
                } else {
                    match &entries.borrow()[name].source {
                        Source::Embedded(text) => Ok((*text).into()),
                        Source::Generated(text) => Ok(text.clone()),
                        Source::File(path) => {
                            fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
                        }
                    }
                };
                text.map(|text| {
                    if normalize_eol {
                        normalize_lf(text)
                    } else {
                        text
                    }
                })
            },
            |owner, request| {
                // The compiler-owned adapter exposes the selected application
                // through one fixed private module declaration. The authored
                // crate name never has to be representable as an identifier.
                if owner == "std/_entry/adapter"
                    && request == "std/_entry/adapter/application"
                    && application.is_some()
                {
                    let request = application.unwrap();
                    entries
                        .borrow()
                        .contains_key(request)
                        .then(|| request.to_owned())
                } else {
                    request_name(&entries.borrow(), workspace.as_deref(), owner, request)
                }
            },
            |_, name| {
                if entries
                    .borrow()
                    .get(name)
                    .is_some_and(|entry| entry.origin == "builtin")
                {
                    return Some(ModuleSpec {
                        native: native_module(name),
                        name: name.to_owned(),
                        kind: ModuleKind::Source,
                        implicit_imports: if name == "std/prelude" {
                            vec![]
                        } else {
                            vec!["std/prelude".into()]
                        },
                    });
                }
                let target_owner = name.split_once('/').map_or(name, |(owner, _)| owner);
                let declaration = workspace
                    .as_ref()?
                    .discover_module(target_owner, name)
                    .ok()??;
                let kind = if declaration.format == ModuleFormat::Telora {
                    ModuleKind::Source
                } else {
                    ModuleKind::Data
                };
                entries.borrow_mut().insert(
                    name.to_owned(),
                    Entry {
                        name: name.to_owned(),
                        origin: if target_owner == owner {
                            "crate"
                        } else {
                            "dependency"
                        },
                        visibility: if private(name) { "private" } else { "public" },
                        format: declaration.format,
                        source: Source::File(declaration.physical_path),
                        test: false,
                    },
                );
                Some(ModuleSpec {
                    native: None,
                    name: name.to_owned(),
                    kind,
                    implicit_imports: vec!["std/prelude".into()],
                })
            },
            cancelled,
        )?;
        if self.check_all_crate_modules {
            mir.roots = mir
                .modules
                .iter()
                .enumerate()
                .filter(|(_, module)| {
                    module.name == owner || module.name.starts_with(&format!("{owner}/"))
                })
                .filter(|(_, module)| {
                    !matches!(module.state, telora_core::mir::ModuleState::Unloaded)
                })
                .map(|(index, _)| {
                    telora_core::mir::ModuleTarget::Bound(telora_core::mir::ModuleId::from_index(
                        index,
                    ))
                })
                .collect();
        } else {
            mir.roots = roots
                .iter()
                .map(|root| {
                    mir.modules
                        .iter()
                        .position(|module| module.name == *root)
                        .map(|index| {
                            telora_core::mir::ModuleTarget::Bound(
                                telora_core::mir::ModuleId::from_index(index),
                            )
                        })
                        .unwrap_or_else(|| telora_core::mir::ModuleTarget::Unresolved(root.clone()))
                })
                .collect();
        }
        module_resolve::validate_source_modules(&mut mir, |name| {
            entries.borrow()[name].origin == "builtin"
        });
        if cancelled() {
            return None;
        }
        telora_core::symbol_resolve::resolve(&mut mir);
        if cancelled() {
            return None;
        }
        telora_core::type_resolve::resolve_with_options(&mut mir, self.compiler);
        if cancelled() { None } else { Some(mir) }
    }
}
