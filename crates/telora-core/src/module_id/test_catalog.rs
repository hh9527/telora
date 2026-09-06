use super::*;
use std::fs;

#[derive(Clone, Debug)]
pub(super) struct TestCatalog {
    root: PathBuf,
    modules: BTreeMap<PathBuf, ResolvedModule>,
}

impl TestCatalog {
    pub(super) fn discover(
        crate_root: &Path,
        owner: &str,
        workspace: &ResolvedWorkspace,
    ) -> Result<Self, ResolveModuleError> {
        let root = crate_root.join("tests");
        reject_symlink(&root)?;
        let mut catalog = Self {
            root: resolve_physical(&root)?,
            modules: BTreeMap::new(),
        };
        catalog.scan(&catalog.root.clone(), owner)?;
        for path in catalog.modules.keys() {
            let selector = format!("@src/tests/{}", path.to_string_lossy().replace('\\', "/"));
            if workspace.module(owner, &selector).is_some() {
                return Err(ResolveModuleError::InvalidImport(format!(
                    "{owner}/tests/{}; test identity conflicts with a declared source module",
                    path.display()
                )));
            }
        }
        Ok(catalog)
    }

    fn scan(&mut self, directory: &Path, owner: &str) -> Result<(), ResolveModuleError> {
        let mut entries = fs::read_dir(directory)
            .map_err(|error| ResolveModuleError::Io(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| ResolveModuleError::Io(error.to_string()))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let kind = reject_symlink(&path)?;
            if kind.is_dir() {
                self.scan(&path, owner)?;
            } else if kind.is_file() {
                let Ok(format) = ModuleFormat::from_path(&path) else {
                    continue;
                };
                let relative = path.strip_prefix(&self.root).expect("catalog child");
                let logical = canonical_path_for_physical(relative)?;
                for component in logical.components() {
                    let name = component
                        .as_os_str()
                        .to_str()
                        .ok_or(ResolveModuleError::NonUtf8Path)?;
                    if name.contains(['\\', ':']) {
                        return Err(ResolveModuleError::InvalidImport(name.into()));
                    }
                }
                self.modules.insert(
                    logical.clone(),
                    ResolvedModule {
                        id: ModuleCName::Test {
                            owner: owner.into(),
                            path: logical,
                        },
                        format,
                        vendor: ModuleVendor::Configured,
                        physical_path: Some(path),
                    },
                );
            }
        }
        Ok(())
    }

    pub(super) fn resolve(
        &self,
        path: &Path,
        original: &str,
    ) -> Result<ResolvedModule, ResolveModuleError> {
        if path.extension().and_then(|extension| extension.to_str()) == Some("telora") {
            return Err(ResolveModuleError::InvalidImport(format!(
                "{original}; Telora module selectors must omit .telora"
            )));
        }
        let module = self
            .modules
            .get(path)
            .ok_or_else(|| ResolveModuleError::ModuleNotFound(original.into()))?;
        // Recheck the captured path without admitting newly created catalog entries.
        let physical = module.path().expect("test module has a physical path");
        reject_symlink(&self.root)?;
        let mut checked = self.root.clone();
        for component in physical
            .strip_prefix(&self.root)
            .expect("catalog child")
            .components()
        {
            checked.push(component);
            reject_symlink(&checked)?;
        }
        if !physical.is_file() {
            return Err(ResolveModuleError::ModuleNotFound(original.into()));
        }
        Ok(module.clone())
    }
}

fn reject_symlink(path: &Path) -> Result<fs::FileType, ResolveModuleError> {
    let kind = fs::symlink_metadata(path)
        .map_err(|error| ResolveModuleError::Io(error.to_string()))?
        .file_type();
    if kind.is_symlink() {
        return Err(ResolveModuleError::InvalidImport(format!(
            "{}; test catalogs do not allow symlinks",
            path.display()
        )));
    }
    Ok(kind)
}
