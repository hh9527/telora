//! Workspace input for the three MIR passes. No legacy module/symbol/type resolver.
use std::{
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

pub fn read_limited(reader: impl std::io::Read, max_bytes: usize, description: &str) -> Result<Vec<u8>, String> {
    let max_read = u64::try_from(max_bytes).unwrap_or(u64::MAX).saturating_add(1);
    let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024));
    reader.take(max_read).read_to_end(&mut bytes).map_err(|error| format!("cannot read {description}: {error}"))?;
    if bytes.len() > max_bytes {
        return Err(format!("{description} exceeds file_size limit ({} > {max_bytes})", bytes.len()));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inventory(family: &str) -> Inventory {
        let mut inventory = Inventory::new(Path::new("."), true).unwrap();
        inventory.entries.insert("app/main".into(), Entry {
            name: "app/main".into(), origin: "crate", visibility: "public",
            format: ModuleFormat::Telora, test: false,
            source: Source::Generated(format!(r#"
                import "std/entry" as entry;
                import "std/ees" as ees;
                export def main = entry.{family}(Int.type, {{sources: [], envs: [], args: False}}, ees.none,
                    fn(ctx) {{ (42, fn(state, event) {{ (state, []) }}) }});
            "#)),
        });
        inventory
    }

    #[test]
    fn generated_entry_policy_and_application_share_one_closed_graph() {
        for (mode, family) in [(telora_core::codegen::RunMode::Run, "run"), (telora_core::codegen::RunMode::Serve, "serve")] {
            let mir = inventory(family).solve_run("app/main", "main", mode).unwrap();
            let sealed = mir.seal().unwrap_or_else(|d| panic!("{family}: {d:?}\n{}", mir.diagnostics.iter().map(|d| mir.sources.render(d)).collect::<Vec<_>>().join("\n")));
            let telora_core::mir::ModuleTarget::Bound(root) = mir.roots[0] else { panic!("adapter root") };
            let entry = *mir.exports[root.index()].iter().find(|s| mir.symbols[s.index()].name == "configure").unwrap();
            let compiled = telora_core::codegen::compile_run(sealed, entry).unwrap_or_else(|d| panic!("{family}: {d:?}"));
            assert!(compiled.run_calls.is_some());
            assert_eq!(mir.modules.iter().filter(|m| m.name == "app/main").count(), 1);
            assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        }
    }

    #[test]
    fn generated_entry_rejects_wrong_nominal_family_and_unsafe_export_text() {
        let mut inventory = inventory("serve");
        let mir = inventory.solve_run("app/main", "main", telora_core::codegen::RunMode::Run).unwrap();
        assert!(mir.seal().is_err(), "Serve cannot satisfy Run's nominal contract");
        assert!(inventory.solve_run("app/main", "main }; fail!(\"injected\");", telora_core::codegen::RunMode::Run).is_err());
        assert!(inventory.request("app/main", "std/_entry/adapter").is_none());
    }
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
}

fn private(name: &str) -> bool {
    name.split('/').any(|part| part.starts_with('_'))
}

impl Inventory {
    /// Editor roots may be private modules. Identity still comes exclusively
    /// from the workspace catalog (or its normal test-module inventory).
    pub fn document_name(&mut self, path: &Path) -> Result<String, String> {
        let matches_path = |entry: &Entry| matches!(&entry.source,
            Source::File(file) if file == path || file.canonicalize().ok().as_deref() == Some(path));
        if let Some((name, _)) = self.entries.iter().find(|(_, entry)| matches_path(entry)) { return Ok(name.clone()); }
        let workspace = self.workspace.as_ref().ok_or("editor documents require a workspace")?;
        let owner = workspace.crate_for_path(path).map_err(|e| e.to_string())?.to_owned();
        let test_root = workspace.crate_root(&owner).ok_or("missing declaring crate")?.join("tests");
        if path.starts_with(&test_root) {
            self.scan_tests_for(&owner, &test_root, &test_root)?;
        }
        self.entries.iter().find(|(_, entry)| matches_path(entry)).map(|(name, _)| name.clone())
            .ok_or_else(|| format!("document {} is not in the module catalog", path.display()))
    }

    pub fn solve_documents(&self, roots: &[String], overlays: &BTreeMap<String, telora_core::DocumentText>) -> Mir {
        self.solve_inputs(roots, None, overlays)
    }

    pub fn workspace(&self) -> Option<Arc<ResolvedWorkspace>> {
        self.workspace.clone()
    }

    pub fn module_paths(&self) -> std::collections::HashMap<String, PathBuf> {
        self.entries.iter().filter_map(|(name, entry)| match &entry.source {
            Source::File(path) => Some((name.clone(), path.clone())),
            _ => None,
        }).collect()
    }

    pub fn undeclared_warnings(&self) -> Result<Vec<String>, String> {
        let mut warnings = vec![];
        if let Some(workspace) = &self.workspace {
            for (crate_name, _) in workspace.crates() {
                for module in workspace
                    .undeclared_modules(crate_name)
                    .map_err(|e| e.to_string())?
                {
                    warnings.push(format!(
                        "crate {:?} contains undeclared module file {}; add {:?} to telora-crate.json modules",
                        module.crate_name, module.relative_path.display(), module.selector,
                    ));
                }
            }
        }
        Ok(warnings)
    }
    /// Called by the execution linker after static solving and code generation.
    pub fn read_data(
        &self,
        link: &telora_core::codegen::DataLink,
        max_bytes: usize,
    ) -> Result<telora_core::EvalSource, String> {
        let entry = self
            .entries
            .get(&link.name)
            .ok_or("unknown data module identity")?;
        let Source::File(path) = &entry.source else {
            return Err("data module has no file source".into());
        };
        let format = match entry.format {
            ModuleFormat::Json => telora_core::SystemDataFormat::Json,
            ModuleFormat::Yaml => telora_core::SystemDataFormat::Yaml,
            ModuleFormat::Toml => telora_core::SystemDataFormat::Toml,
            ModuleFormat::Telora => {
                return Err("source module cannot fill a data relocation".into());
            }
        };
        let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let bytes = read_limited(file, max_bytes, &path.display().to_string())?;
        let text = String::from_utf8(bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(telora_core::EvalSource {
            source_name: link.name.clone(),
            format,
            text,
        })
    }
    pub fn new(context: &Path, builtin_only: bool) -> Result<Self, String> {
        let workspace = if builtin_only {
            None
        } else {
            Some(crate::package_host::prepare(context)?)
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
                    let cname = format!(
                        "{name}/{}",
                        module.logical_path.to_string_lossy().replace('\\', "/")
                    );
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
        })
    }

    pub fn catalog(&self) -> impl Iterator<Item = &Entry> {
        self.entries
            .values()
            .filter(|e| !e.test && (e.origin == "crate" || e.visibility == "public"))
    }

    pub fn select(&mut self, selector: &str) -> Result<String, String> {
        let name = if let Some(path) = selector.strip_prefix("@src/") {
            format!("{}/{path}", self.owner)
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
                    origin: if owner == self.owner { "crate" } else { "dependency" },
                    format,
                    source: Source::File(path.to_owned()),
                    test: true,
                },
            );
        }
        Ok(())
    }

    fn request(&self, importer: &str, request: &str) -> Option<String> {
        let owner = importer.split_once('/')?.0;
        let name = if let Some(path) = request.strip_prefix("@src/") {
            format!("{owner}/{path}")
        } else if let Some(path) = request.strip_prefix("@test/") {
            format!("{owner}/tests/{path}")
        } else if request.starts_with("./") || request.starts_with("../") {
            let mut parts = importer.split('/').collect::<Vec<_>>();
            parts.pop();
            let floor = if self.entries.get(importer).is_some_and(|e| e.test) {
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
        let entry = self.entries.get(&name)?;
        let target_owner = name.split_once('/')?.0;
        if entry.test && !self.entries.get(importer).is_some_and(|e| e.test) {
            return None;
        }
        if target_owner != owner {
            if entry.visibility == "private" || entry.test {
                return None;
            }
            if target_owner != "std"
                && !self
                    .workspace
                    .as_ref()
                    .is_some_and(|w| w.declares_dependency(owner, target_owner))
            {
                return None;
            }
        }
        Some(name)
    }

    pub fn solve(&self, root: &str) -> Mir {
        self.solve_with_entry(root, None)
    }

    /// Compiler-owned entry sources share the application's graph and passes.
    pub fn solve_run(&mut self, application: &str, export: &str, mode: telora_core::codegen::RunMode) -> Result<Mir, String> {
        let adapter = mode.adapter_source(application, export)?;
        for (name, source) in [
            (mode.policy_module(), Source::Embedded(mode.policy_source())),
            ("std/_entry/adapter", Source::Generated(adapter)),
        ] {
            self.entries.insert(name.into(), Entry {
                name: name.into(), origin: "builtin", visibility: "private",
                format: ModuleFormat::Telora, source, test: false,
            });
        }
        Ok(self.solve_with_entry("std/_entry/adapter", Some(application)))
    }

    fn solve_with_entry(&self, root: &str, application: Option<&str>) -> Mir {
        self.solve_inputs(&[root.to_owned()], application, &BTreeMap::new())
    }

    fn solve_inputs(&self, roots: &[String], application: Option<&str>, overlays: &BTreeMap<String, telora_core::DocumentText>) -> Mir {
        let specs = self
            .entries
            .values()
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
        let mut mir = module_resolve::resolve_with_requests(
            specs,
            roots,
            |_, name| if let Some(text) = overlays.get(name) { Ok(text.to_string()) } else { match &self.entries[name].source {
                Source::Embedded(text) => Ok((*text).into()),
                Source::Generated(text) => Ok(text.clone()),
                Source::File(path) => {
                    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
                }
            } },
            |owner, request| {
                // The user selected this application before the compiler-owned
                // adapter was inserted. Only its exact import gets this edge;
                // ordinary application/dependency imports retain normal policy.
                if owner == "std/_entry/adapter" && application == Some(request) {
                    self.entries.contains_key(request).then(|| request.to_owned())
                } else {
                    self.request(owner, request)
                }
            },
        );
        telora_core::symbol_resolve::resolve(&mut mir);
        telora_core::type_resolve::resolve(&mut mir);
        mir
    }
}
