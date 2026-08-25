use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};

use crate::DataWorld;
use crate::ast::{Expr, ExprKind};
use crate::heap::{DecodedValue, Heap, Object, Val};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ModuleFormat {
    Telora,
    Json,
    Toml,
    Yaml,
}

impl ModuleFormat {
    pub fn from_path(path: &Path) -> Result<Self, ResolveModuleError> {
        match path.extension().and_then(|extension| extension.to_str()) {
            Some("telora") => Ok(Self::Telora),
            Some("json") => Ok(Self::Json),
            Some("toml") => Ok(Self::Toml),
            Some("yaml" | "yml") => Ok(Self::Yaml),
            Some(extension) => Err(ResolveModuleError::UnknownExtension(extension.into())),
            None => Err(ResolveModuleError::MissingExtension),
        }
    }

    pub fn parse(name: &str) -> Result<Self, ResolveModuleError> {
        match name {
            "telora" => Ok(Self::Telora),
            "json" => Ok(Self::Json),
            "toml" => Ok(Self::Toml),
            "yaml" => Ok(Self::Yaml),
            _ => Err(ResolveModuleError::UnknownFormat(name.into())),
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Telora => "telora",
            Self::Json => "json",
            Self::Toml => "toml",
            Self::Yaml => "yaml",
        }
    }

    pub const fn is_supported(self) -> bool {
        matches!(self, Self::Telora | Self::Json | Self::Toml | Self::Yaml)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ModuleVendor {
    Configured,
    Builtin,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ModuleCatalogOrigin {
    Crate,
    Dependency,
    Builtin,
}

impl ModuleCatalogOrigin {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Crate => "crate",
            Self::Dependency => "dependency",
            Self::Builtin => "builtin",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ModuleVisibility {
    Public,
    Private,
}

impl ModuleVisibility {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ModuleCatalogEntry {
    pub id: ModuleCName,
    pub format: ModuleFormat,
    pub origin: ModuleCatalogOrigin,
    pub visibility: ModuleVisibility,
}

/// Stable identity assigned after the complete module graph has been discovered.
///
/// The numeric value is the module's position in the graph sorted by canonical
/// module name. It is deliberately distinct from [`ModuleCName`], which is a
/// resolver-level name and may contain paths.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ModuleId(u32);

impl ModuleId {
    pub(crate) const ANONYMOUS: Self = Self(0);
    pub const FIRST_DYNAMIC: u32 = 16;

    pub(crate) fn from_index(index: usize) -> Self {
        let index = u32::try_from(index).expect("module graph exceeds u32 ID space");
        Self(
            Self::FIRST_DYNAMIC
                .checked_add(index)
                .expect("module graph exceeds u32 ID space"),
        )
    }

    pub(crate) const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    pub const fn index(self) -> usize {
        (self.0 - Self::FIRST_DYNAMIC) as usize
    }

    pub const fn raw(self) -> u32 {
        self.0
    }
}

pub const FIRST_DYNAMIC_MODULE_LOCAL: u32 = 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FuncId {
    pub module: ModuleId,
    pub local: u32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TypeConstructorId {
    pub module: ModuleId,
    pub local: u32,
}

/// Stable identity of a static trait declaration.
///
/// A trait dictionary is a nominal type family, so its trait and constructor
/// identities deliberately share the same module-local slot.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TraitId {
    pub module: ModuleId,
    pub local: u32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TraitImplId {
    pub module: ModuleId,
    pub local: u32,
}

impl From<TypeConstructorId> for TraitId {
    fn from(value: TypeConstructorId) -> Self {
        Self {
            module: value.module,
            local: value.local,
        }
    }
}

impl From<TraitId> for TypeConstructorId {
    fn from(value: TraitId) -> Self {
        Self {
            module: value.module,
            local: value.local,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ModuleCName {
    Source { owner: String, path: PathBuf },
    Binary { owner: String, path: PathBuf },
    Entry { owner: String, path: PathBuf },
    Test { owner: String, path: PathBuf },
    Builtin(String),
    Dependency { name: String, path: PathBuf },
}

impl ModuleCName {
    pub fn builtin(name: impl Into<String>) -> Self {
        Self::Builtin(name.into())
    }

    pub fn owner(&self) -> &str {
        match self {
            Self::Source { owner, .. }
            | Self::Binary { owner, .. }
            | Self::Entry { owner, .. }
            | Self::Test { owner, .. } => owner,
            Self::Builtin(name) => name.split_once('/').map_or(name, |(owner, _)| owner),
            Self::Dependency { name, .. } => name,
        }
    }
}

impl fmt::Display for ModuleCName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source { owner, path } => write!(formatter, "{owner}/{}", path.display()),
            Self::Binary { owner, path } => write!(formatter, "{owner}/bin/{}", path.display()),
            Self::Entry { owner, path } => write!(formatter, "{owner}/entry/{}", path.display()),
            Self::Test { owner, path } => write!(formatter, "{owner}/tests/{}", path.display()),
            Self::Builtin(name) => formatter.write_str(name),
            Self::Dependency { name, path } => write!(formatter, "{name}/{}", path.display()),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ResolvedModule {
    pub id: ModuleCName,
    pub format: ModuleFormat,
    pub vendor: ModuleVendor,
    physical_path: Option<PathBuf>,
}

impl ResolvedModule {
    pub fn path(&self) -> Option<&Path> {
        self.physical_path.as_deref()
    }
}

impl fmt::Display for ResolvedModule {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.id.fmt(formatter)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolveModuleError {
    EmptyPath,
    MissingExtension,
    NonUtf8Path,
    UnknownExtension(String),
    UnknownFormat(String),
    UnknownDependency(String),
    ModuleNotFound(String),
    InvalidImport(String),
    InvalidModuleSuffix(String),
    CrateEscape(String),
    PrivateModuleAccess(String),
    PrivateModuleRoot,
    EntryModuleAccess(String),
    EntryModuleRoot,
    Manifest(String),
    Io(String),
}

impl fmt::Display for ResolveModuleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPath => formatter.write_str("module path is empty"),
            Self::MissingExtension => formatter.write_str("module path has no extension"),
            Self::NonUtf8Path => formatter.write_str("module path is not valid UTF-8"),
            Self::UnknownExtension(extension) => {
                write!(formatter, "unknown module extension .{extension}")
            }
            Self::UnknownFormat(format) => write!(formatter, "unknown module format {format:?}"),
            Self::UnknownDependency(name) => write!(formatter, "unknown dependency {name:?}"),
            Self::ModuleNotFound(module) => write!(formatter, "module {module:?} not found"),
            Self::InvalidImport(request) => write!(formatter, "invalid module import {request:?}"),
            Self::InvalidModuleSuffix(name) => write!(
                formatter,
                "invalid module filename {name:?}; expected a Telora module or an exact .json, .yaml, .yml, or .toml suffix"
            ),
            Self::CrateEscape(request) => {
                write!(formatter, "module import escapes its crate root: {request}")
            }
            Self::PrivateModuleAccess(request) => write!(
                formatter,
                "private module {request:?} can only be imported from the same crate or by the selected Entry"
            ),
            Self::PrivateModuleRoot => {
                formatter.write_str("a private module cannot be used as the root module")
            }
            Self::EntryModuleAccess(request) => write!(
                formatter,
                "Entry module {request:?} can only be selected by telora run-with"
            ),
            Self::EntryModuleRoot => {
                formatter.write_str("an entry module cannot be used as an ordinary root")
            }
            Self::Manifest(message) | Self::Io(message) => formatter.write_str(message),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ModuleResolver {
    crate_name: String,
    standalone: bool,
    workspace_root: PathBuf,
    source_root: PathBuf,
    root_path: PathBuf,
    root_id: ModuleCName,
    dependencies: BTreeMap<String, PathBuf>,
    builtins: BTreeMap<String, u32>,
    selected_entry: Option<ModuleCName>,
}

impl ModuleResolver {
    pub fn standalone(root_module: &Path) -> Result<Self, ResolveModuleError> {
        Self::standalone_with_source(root_module, None)
    }

    fn standalone_with_source(
        root_module: &Path,
        root_source: Option<String>,
    ) -> Result<Self, ResolveModuleError> {
        let root_path = absolute_normalized(root_module)?;
        let root_path = if root_source.is_some() {
            root_path
        } else {
            resolve_physical(&root_path)?
        };
        let workspace_root = root_path
            .parent()
            .ok_or_else(|| ResolveModuleError::Io("standalone module has no parent".into()))?
            .to_owned();
        let name = root_path.file_name().ok_or_else(|| {
            ResolveModuleError::InvalidImport("standalone root has no module file name".to_owned())
        })?;
        let canonical_name = canonical_path_for_physical(Path::new(name))?;
        let mut resolver = Self {
            crate_name: String::new(),
            standalone: true,
            workspace_root: workspace_root.clone(),
            source_root: workspace_root,
            root_path: root_path.clone(),
            root_id: ModuleCName::Binary {
                owner: String::new(),
                path: canonical_name.clone(),
            },
            dependencies: BTreeMap::new(),
            builtins: BTreeMap::new(),
            selected_entry: None,
        };
        let source = root_source.map_or_else(
            || {
                std::fs::read_to_string(&root_path).map_err(|error| {
                    ResolveModuleError::Io(format!("cannot read {}: {error}", root_path.display()))
                })
            },
            Ok,
        )?;
        let mut sources = crate::SourceDatabase::default();
        let source_id = sources.add(resolver.root_id.to_string(), source);
        let parsed = crate::parser::parse_registered(&sources, source_id);
        for option in parsed
            .options
            .iter()
            .filter(|option| option.key.value.starts_with("crate."))
        {
            let value = immediate_value(&option.value)?;
            resolver.apply_standalone_option(&option.key.value, value.value())?;
        }
        if resolver.crate_name.is_empty() {
            resolver.crate_name = "standalone".into();
        }
        resolver.root_id = ModuleCName::Binary {
            owner: resolver.crate_name.clone(),
            path: canonical_name,
        };
        Ok(resolver)
    }

    pub fn from_cwd(cwd: &Path, root_id: &str) -> Result<Self, ResolveModuleError> {
        let (manifest, workspace_root, source_root) = workspace_layout(cwd)?;
        let config = read_manifest_config(&workspace_root, &manifest)?;
        let (id, path) = logical_root(&workspace_root, &source_root, &config.name, root_id)?;
        let root_path = resolve_physical(&path)?;
        let expected_root = match &id {
            ModuleCName::Source { .. } => source_root.clone(),
            ModuleCName::Binary { .. } => resolve_physical(&source_root.join("bin"))?,
            ModuleCName::Entry { .. } => resolve_physical(&source_root.join("entry"))?,
            ModuleCName::Test { .. } => resolve_physical(&workspace_root.join("tests"))?,
            _ => workspace_root.clone(),
        };
        if !root_path.starts_with(&expected_root) {
            return Err(ResolveModuleError::CrateEscape(root_id.into()));
        }
        if is_private_file_name(&root_path) {
            return Err(ResolveModuleError::PrivateModuleRoot);
        }
        let mut resolver = Self {
            crate_name: config.name,
            standalone: false,
            workspace_root,
            source_root,
            root_path,
            root_id: id,
            dependencies: config.dependencies,
            builtins: BTreeMap::new(),
            selected_entry: None,
        };
        // Resolve dependency IDs after loading aliases.
        if !root_id.starts_with('@') {
            let resolved = resolver.resolve_dependency(root_id, root_id, true)?;
            resolver.root_id = resolved.id;
            resolver.root_path = resolved.physical_path.expect("dependency root has a path");
        }
        if !resolver.root_path.is_file() {
            return Err(ResolveModuleError::ModuleNotFound(
                resolver.root_id.to_string(),
            ));
        }
        Ok(resolver)
    }

    pub fn catalog_from_cwd(
        cwd: &Path,
        builtins: impl IntoIterator<Item = (String, u32)>,
    ) -> Result<Vec<ModuleCatalogEntry>, ResolveModuleError> {
        let (manifest, workspace_root, source_root) = workspace_layout(cwd)?;
        let config = read_manifest_config(&workspace_root, &manifest)?;
        let builtins = builtins.into_iter().collect::<Vec<_>>();
        let builtin_owners = builtins
            .iter()
            .map(|(name, _)| logical_owner(name).to_owned())
            .collect::<BTreeSet<_>>();
        let mut modules = BTreeMap::new();

        if !builtin_owners.contains(&config.name) {
            for (path, physical) in module_files(&source_root)? {
                let canonical = canonical_path_for_physical(&path)?;
                if canonical.starts_with("bin") || canonical.starts_with("entry") {
                    continue;
                }
                let id = ModuleCName::Source {
                    owner: config.name.clone(),
                    path: canonical,
                };
                insert_catalog_file(&mut modules, id, physical, ModuleCatalogOrigin::Crate, true);
            }
        }

        for (name, root) in &config.dependencies {
            if builtin_owners.contains(name) || name == &config.name {
                continue;
            }
            let source_candidate = root.join("src");
            let source_root = if source_candidate.is_dir() {
                resolve_physical(&source_candidate)?
            } else {
                root.clone()
            };
            for (path, physical) in module_files(&source_root)? {
                if path.components().next().is_some_and(|part| {
                    matches!(part.as_os_str().to_str(), Some("bin" | "entry" | "tests"))
                }) || is_private_module_path(&path)
                {
                    continue;
                }
                insert_catalog_file(
                    &mut modules,
                    ModuleCName::Dependency {
                        name: name.clone(),
                        path: canonical_path_for_physical(&path)?,
                    },
                    physical,
                    ModuleCatalogOrigin::Dependency,
                    false,
                );
            }
        }

        for (name, _) in builtins {
            if !is_public_builtin_name(&name) {
                continue;
            }
            let id = ModuleCName::builtin(name);
            modules.insert(
                id.to_string(),
                ModuleCatalogEntry {
                    id,
                    format: ModuleFormat::Telora,
                    origin: ModuleCatalogOrigin::Builtin,
                    visibility: ModuleVisibility::Public,
                },
            );
        }

        Ok(modules.into_values().collect())
    }

    pub fn selected_root(&self) -> Result<ResolvedModule, ResolveModuleError> {
        self.resolve_root(&self.root_path)
    }

    pub fn is_root(&self, id: &ModuleCName) -> bool {
        id == &self.root_id
    }

    pub fn is_standalone(&self) -> bool {
        self.standalone
    }

    pub fn root_path(&self) -> &Path {
        &self.root_path
    }
    pub fn for_root(root_module: &Path) -> Result<Self, ResolveModuleError> {
        Self::for_root_source(root_module, None)
    }

    pub fn for_root_with_source(
        root_module: &Path,
        source: &crate::document::DocumentText,
    ) -> Result<Self, ResolveModuleError> {
        Self::for_root_source(root_module, Some(source.to_string()))
    }

    fn for_root_source(
        root_module: &Path,
        root_source: Option<String>,
    ) -> Result<Self, ResolveModuleError> {
        let root = absolute_normalized(root_module)?;
        let start = root
            .parent()
            .ok_or_else(|| ResolveModuleError::Io("root module has no parent directory".into()))?;
        let manifest = start
            .ancestors()
            .map(|directory| directory.join("telora-deps.json"))
            .find(|candidate| candidate.is_file());
        if manifest.is_none() {
            return Self::standalone_with_source(&root, root_source);
        }
        let inferred_root = infer_embedding_root(start);
        let workspace_root = manifest
            .as_ref()
            .and_then(|manifest| manifest.parent())
            .unwrap_or(inferred_root)
            .to_owned();
        let source_candidate = workspace_root.join("src");
        let source_root = if source_candidate.is_dir() {
            resolve_physical(&source_candidate)?
        } else {
            resolve_physical(start)?
        };
        let resolved_root = if root_source.is_some() {
            root
        } else {
            resolve_physical(&root)?
        };
        let config = read_manifest_config(
            &workspace_root,
            manifest.as_ref().expect("manifest checked above"),
        )?;
        let (root_id, _root_is_entry) = module_id_for_physical_root(
            &resolved_root,
            &workspace_root,
            &source_root,
            &config.name,
        )?;
        let resolver = Self {
            crate_name: config.name,
            standalone: false,
            workspace_root,
            source_root,
            root_path: resolved_root,
            root_id,
            dependencies: config.dependencies,
            builtins: BTreeMap::new(),
            selected_entry: None,
        };
        Ok(resolver)
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn with_builtins(mut self, builtins: impl IntoIterator<Item = (String, u32)>) -> Self {
        for (name, id) in builtins {
            self.builtins.entry(name).or_insert(id);
        }
        self
    }

    pub fn with_entry_context(mut self, entry: ModuleCName) -> Self {
        self.selected_entry = Some(entry);
        self
    }

    pub fn resolve_root(&self, path: &Path) -> Result<ResolvedModule, ResolveModuleError> {
        let path = if path.is_file() {
            resolve_physical(path)?
        } else {
            absolute_normalized(path)?
        };
        if path != self.root_path {
            return Err(ResolveModuleError::InvalidImport(format!(
                "resolver root is {}, not {}",
                self.root_path.display(),
                path.display()
            )));
        }
        if matches!(self.root_id, ModuleCName::Entry { .. }) {
            return Err(ResolveModuleError::EntryModuleRoot);
        }
        if is_private_file_name(&path) {
            return Err(ResolveModuleError::PrivateModuleRoot);
        }
        if self.builtin_owns(self.root_id.owner()) {
            return Err(ResolveModuleError::InvalidImport(format!(
                "crate {:?} is provided by an earlier resolver vendor",
                self.root_id.owner()
            )));
        }
        let format = self.format_for(&path)?;
        Ok(ResolvedModule {
            id: self.root_id.clone(),
            format,
            vendor: ModuleVendor::Configured,
            physical_path: Some(path),
        })
    }

    pub fn resolve_import(
        &self,
        importer: &ModuleCName,
        target: &str,
    ) -> Result<ResolvedModule, ResolveModuleError> {
        if target.is_empty() {
            return Err(ResolveModuleError::EmptyPath);
        }
        let privileged = self.selected_entry.as_ref() == Some(importer);
        if !target.starts_with(['.', '@']) && self.builtins.contains_key(target) {
            match special_root_of_logical_name(target) {
                Some("entry") => {
                    return Err(ResolveModuleError::EntryModuleAccess(target.into()));
                }
                Some(_) => return Err(ResolveModuleError::InvalidImport(target.into())),
                None => {}
            }
            if is_private_logical_name(target)
                && !privileged
                && importer.owner() != logical_owner(target)
            {
                return Err(ResolveModuleError::PrivateModuleAccess(target.into()));
            }
            return Ok(ResolvedModule {
                id: ModuleCName::builtin(target),
                format: ModuleFormat::Telora,
                vendor: ModuleVendor::Builtin,
                physical_path: None,
            });
        }
        if !target.starts_with(['.', '@'])
            && let Some((owner, _)) = target.split_once('/')
            && self.builtin_owns(owner)
        {
            return Err(ResolveModuleError::ModuleNotFound(target.into()));
        }
        if target == self.root_id.to_string() {
            if !privileged {
                return Err(ResolveModuleError::InvalidImport(target.into()));
            }
            let format = self.format_for(&self.root_path)?;
            return Ok(ResolvedModule {
                id: self.root_id.clone(),
                format,
                vendor: ModuleVendor::Configured,
                physical_path: Some(self.root_path.clone()),
            });
        }
        if let Some(path) = target.strip_prefix("@bin/") {
            if !privileged {
                return Err(ResolveModuleError::InvalidImport(target.into()));
            }
            let expected = match &self.root_id {
                ModuleCName::Binary { path, .. } => path,
                _ => return Err(ResolveModuleError::InvalidImport(target.into())),
            };
            if Path::new(path) == expected {
                return Ok(ResolvedModule {
                    id: self.root_id.clone(),
                    format: self.format_for(&self.root_path)?,
                    vendor: ModuleVendor::Configured,
                    physical_path: Some(self.root_path.clone()),
                });
            }
            return Err(ResolveModuleError::InvalidImport(target.into()));
        }
        if target.starts_with("@test/") {
            return Err(ResolveModuleError::InvalidImport(target.into()));
        }
        if target.starts_with("@src/entry/") {
            return Err(ResolveModuleError::EntryModuleAccess(target.into()));
        }
        if let Some(path) = target.strip_prefix("@src/") {
            return self.resolve_in_owner(importer, Path::new(path), target);
        }
        if target.starts_with("./") || target.starts_with("../") {
            return match importer {
                ModuleCName::Binary { .. } if self.standalone => {
                    let logical = lexical_normalize_relative(Path::new(target))
                        .ok_or_else(|| ResolveModuleError::CrateEscape(target.into()))?;
                    self.resolve_source(logical, target)
                }
                ModuleCName::Binary { .. }
                | ModuleCName::Entry { .. }
                | ModuleCName::Test { .. } => {
                    return Err(ResolveModuleError::InvalidImport(format!(
                        "{target}; binary and test roots must import crate sources with @src/..."
                    )));
                }
                ModuleCName::Source { path, .. } => {
                    let logical = lexical_normalize_relative(
                        &path.parent().unwrap_or_else(|| Path::new("")).join(target),
                    )
                    .ok_or_else(|| ResolveModuleError::CrateEscape(target.into()))?;
                    self.resolve_source(logical, target)
                }
                ModuleCName::Dependency { name, path } => {
                    let logical = lexical_normalize_relative(
                        &path.parent().unwrap_or_else(|| Path::new("")).join(target),
                    )
                    .ok_or_else(|| ResolveModuleError::CrateEscape(target.into()))?;
                    self.resolve_dependency_parts(name, logical, target, true)
                }
                ModuleCName::Builtin(_) => Err(ResolveModuleError::InvalidImport(
                    "built-in modules cannot use relative imports".into(),
                )),
            };
        }
        if target.starts_with('@') {
            return Err(ResolveModuleError::InvalidImport(target.into()));
        }
        let (owner, path) = target
            .split_once('/')
            .ok_or_else(|| ResolveModuleError::InvalidImport(target.into()))?;
        if owner == self.crate_name {
            if !privileged && importer.owner() != self.crate_name {
                return Err(ResolveModuleError::PrivateModuleAccess(target.into()));
            }
            if let Some(path) = path.strip_prefix("bin/") {
                if !privileged {
                    return Err(ResolveModuleError::PrivateModuleAccess(target.into()));
                }
                let ModuleCName::Binary { path: selected, .. } = &self.root_id else {
                    return Err(ResolveModuleError::InvalidImport(target.into()));
                };
                if Path::new(path) != selected {
                    return Err(ResolveModuleError::InvalidImport(target.into()));
                }
                return Ok(ResolvedModule {
                    id: self.root_id.clone(),
                    format: self.format_for(&self.root_path)?,
                    vendor: ModuleVendor::Configured,
                    physical_path: Some(self.root_path.clone()),
                });
            }
            if path.starts_with("entry/") {
                return Err(ResolveModuleError::EntryModuleAccess(target.into()));
            }
            if path.starts_with("tests/") {
                return Err(ResolveModuleError::InvalidImport(target.into()));
            }
            return self.resolve_source(PathBuf::from(path), target);
        }
        self.resolve_dependency(target, target, privileged || importer.owner() == owner)
    }

    pub fn resolve_entry(&self, target: &str) -> Result<ResolvedModule, ResolveModuleError> {
        let (owner, selector, root) = if let Some(path) = target.strip_prefix("@src/entry/") {
            (
                self.crate_name.as_str(),
                path,
                self.source_root.join("entry"),
            )
        } else {
            let (owner, path) = target.split_once("/entry/").ok_or_else(|| {
                ResolveModuleError::InvalidImport(format!(
                    "Entry selector {target:?} must be @src/entry/<name> or <crate>/entry/<name>"
                ))
            })?;
            if self.builtin_owns(owner) {
                return Err(ResolveModuleError::ModuleNotFound(target.into()));
            }
            let root = if owner == self.crate_name {
                self.source_root.join("entry")
            } else {
                let dependency = self
                    .dependencies
                    .get(owner)
                    .ok_or_else(|| ResolveModuleError::UnknownDependency(owner.into()))?;
                let source = dependency.join("src");
                if source.is_dir() {
                    source
                } else {
                    dependency.clone()
                }
                .join("entry")
            };
            (owner, path, root)
        };
        if selector.contains('/') {
            return Err(ResolveModuleError::InvalidImport(
                "Entry roots support files only".into(),
            ));
        }
        let (path, physical_path) = resolve_selector(&root, PathBuf::from(selector), target)?;
        Ok(ResolvedModule {
            id: ModuleCName::Entry {
                owner: owner.to_owned(),
                path,
            },
            format: ModuleFormat::Telora,
            vendor: ModuleVendor::Configured,
            physical_path: Some(physical_path),
        })
    }

    fn apply_standalone_option(
        &mut self,
        key: &str,
        value: crate::ValueRef<'_>,
    ) -> Result<(), ResolveModuleError> {
        match key {
            "crate.name" => {
                let name = value.as_str().ok_or_else(|| {
                    ResolveModuleError::Manifest("option \"crate.name\" must be a String".into())
                })?;
                validate_crate_name(name.as_str())?;
                if !self.crate_name.is_empty() {
                    return Err(ResolveModuleError::Manifest(
                        "option \"crate.name\" may be declared only once".into(),
                    ));
                }
                self.crate_name = name.as_str().to_owned();
            }
            "crate.dependency" => {
                if value.kind() != crate::ValueKind::Dict {
                    return Err(ResolveModuleError::Manifest(
                        "option \"crate.dependency\" must contain a Dict".into(),
                    ));
                }
                let name = value
                    .get("name")
                    .and_then(crate::ValueRef::as_str)
                    .map(|value| value.as_str().to_owned())
                    .ok_or_else(|| {
                        ResolveModuleError::Manifest(
                            "crate.dependency field \"name\" must be a String".into(),
                        )
                    })?;
                validate_crate_name(&name)?;
                let source = value
                    .get("source")
                    .and_then(crate::ValueRef::tagged_parts)
                    .filter(|(tag, _)| tag.as_atom().as_deref() == Some("Path"))
                    .map(|(_, payload)| payload)
                    .ok_or_else(|| {
                        ResolveModuleError::Manifest(
                            "crate.dependency field \"source\" must be 'Path({path: String})"
                                .into(),
                        )
                    })?;
                let path = source
                    .get("path")
                    .and_then(crate::ValueRef::as_str)
                    .map(|value| value.as_str().to_owned())
                    .ok_or_else(|| {
                        ResolveModuleError::Manifest(
                            "crate.dependency Path field \"path\" must be a String".into(),
                        )
                    })?;
                if !self.dependencies.contains_key(&name) {
                    let root = resolve_physical(&self.workspace_root.join(&path))?;
                    self.dependencies.insert(name, root);
                }
            }
            _ => {
                return Err(ResolveModuleError::Manifest(format!(
                    "unknown standalone resolver option {key:?}"
                )));
            }
        }
        Ok(())
    }

    fn resolve_dependency(
        &self,
        rest: &str,
        original: &str,
        allow_private: bool,
    ) -> Result<ResolvedModule, ResolveModuleError> {
        let (name, path) = rest
            .split_once('/')
            .ok_or_else(|| ResolveModuleError::InvalidImport(original.into()))?;
        if name.is_empty() || path.is_empty() {
            return Err(ResolveModuleError::InvalidImport(original.into()));
        }
        let path = lexical_normalize_relative(Path::new(path))
            .ok_or_else(|| ResolveModuleError::CrateEscape(original.into()))?;
        self.resolve_dependency_parts(name, path, original, allow_private)
    }

    fn resolve_in_owner(
        &self,
        importer: &ModuleCName,
        path: &Path,
        original: &str,
    ) -> Result<ResolvedModule, ResolveModuleError> {
        let path = lexical_normalize_relative(path)
            .ok_or_else(|| ResolveModuleError::CrateEscape(original.into()))?;
        match importer {
            ModuleCName::Source { .. } => self.resolve_source(path, original),
            ModuleCName::Binary { owner, .. }
            | ModuleCName::Entry { owner, .. }
            | ModuleCName::Test { owner, .. } => {
                if owner == &self.crate_name {
                    self.resolve_source(path, original)
                } else {
                    self.resolve_dependency_parts(owner, path, original, true)
                }
            }
            ModuleCName::Dependency { name, .. } => {
                self.resolve_dependency_parts(name, path, original, true)
            }
            ModuleCName::Builtin(_) => Err(ResolveModuleError::InvalidImport(
                "@src is unavailable to built-in modules".into(),
            )),
        }
    }

    fn resolve_source(
        &self,
        path: PathBuf,
        original: &str,
    ) -> Result<ResolvedModule, ResolveModuleError> {
        let (path, physical) = resolve_selector(&self.source_root, path, original)?;
        if !physical.starts_with(&self.source_root) || physical == self.root_path {
            return Err(ResolveModuleError::CrateEscape(original.into()));
        }
        reject_special_source_path(&path, original)?;
        let id = ModuleCName::Source {
            owner: self.crate_name.clone(),
            path,
        };
        let format = self.format_for(&physical)?;
        Ok(ResolvedModule {
            id,
            format,
            vendor: ModuleVendor::Configured,
            physical_path: Some(physical),
        })
    }

    fn resolve_dependency_parts(
        &self,
        name: &str,
        path: PathBuf,
        original: &str,
        allow_private: bool,
    ) -> Result<ResolvedModule, ResolveModuleError> {
        let root = self
            .dependencies
            .get(name)
            .ok_or_else(|| ResolveModuleError::UnknownDependency(name.into()))?;
        let source_candidate = root.join("src");
        let source_root = if source_candidate.is_dir() {
            resolve_physical(&source_candidate)?
        } else {
            root.clone()
        };
        let (path, physical) = resolve_selector(&source_root, path, original)?;
        if !physical.starts_with(&source_root) {
            return Err(ResolveModuleError::CrateEscape(original.into()));
        }
        reject_special_source_path(&path, original)?;
        let private = is_private_module_path(&path);
        let id = ModuleCName::Dependency {
            name: name.into(),
            path,
        };
        let format = self.format_for(&physical)?;
        let module = ResolvedModule {
            id,
            format,
            vendor: ModuleVendor::Configured,
            physical_path: Some(physical),
        };
        if private && !allow_private {
            return Err(ResolveModuleError::PrivateModuleAccess(original.into()));
        }
        Ok(module)
    }

    fn format_for(&self, physical: &Path) -> Result<ModuleFormat, ResolveModuleError> {
        ModuleFormat::from_path(physical)
    }

    fn builtin_owns(&self, owner: &str) -> bool {
        self.builtins
            .keys()
            .any(|module| logical_owner(module) == owner)
    }
}

struct ManifestConfig {
    name: String,
    dependencies: BTreeMap<String, PathBuf>,
}

fn workspace_layout(cwd: &Path) -> Result<(PathBuf, PathBuf, PathBuf), ResolveModuleError> {
    let cwd = absolute_normalized(cwd)?;
    let manifest = cwd
        .ancestors()
        .map(|directory| directory.join("telora-deps.json"))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            ResolveModuleError::Manifest(format!(
                "cannot find telora-deps.json from {} or its ancestors",
                cwd.display()
            ))
        })?;
    let workspace_root = manifest.parent().expect("manifest has parent").to_owned();
    let source_root = resolve_physical(&workspace_root.join("src"))?;
    Ok((manifest, workspace_root, source_root))
}

fn read_manifest_config(
    workspace_root: &Path,
    manifest: &Path,
) -> Result<ManifestConfig, ResolveModuleError> {
    let source = std::fs::read_to_string(manifest).map_err(|error| {
        ResolveModuleError::Manifest(format!("cannot read {}: {error}", manifest.display()))
    })?;
    let value =
        crate::json::parse_json(&manifest.display().to_string(), &source).map_err(|error| {
            ResolveModuleError::Manifest(format!("invalid {}: {error}", manifest.display()))
        })?;
    let name = value
        .value()
        .get("name")
        .and_then(crate::ValueRef::as_str)
        .ok_or_else(|| {
            ResolveModuleError::Manifest("manifest field \"name\" must be a String".into())
        })?;
    validate_crate_name(name.as_str())?;
    let mut config = ManifestConfig {
        name: name.as_str().to_owned(),
        dependencies: BTreeMap::new(),
    };
    apply_manifest(workspace_root, value.value(), &mut config.dependencies)?;
    Ok(config)
}

fn validate_crate_name(name: &str) -> Result<(), ResolveModuleError> {
    if name.is_empty()
        || name.starts_with(['@', '_'])
        || name.contains(['/', '.', '\\'])
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(ResolveModuleError::Manifest(format!(
            "invalid crate name {name:?}; expected ASCII letters, digits, and '-'"
        )));
    }
    Ok(())
}

fn apply_manifest(
    workspace_root: &Path,
    value: crate::ValueRef<'_>,
    dependencies_out: &mut BTreeMap<String, PathBuf>,
) -> Result<(), ResolveModuleError> {
    if value.kind() != crate::ValueKind::Dict {
        return Err(ResolveModuleError::Manifest(
            "dependency manifest must be a JSON object".into(),
        ));
    }
    if let Some(dependencies) = value.get("dependencies") {
        let Some(names) = dependencies.dict_fields() else {
            return Err(ResolveModuleError::Manifest(
                "manifest field \"dependencies\" must be an object".into(),
            ));
        };
        for name in names {
            validate_crate_name(name)?;
            let specification = dependencies.get(name).expect("Dict field exists");
            let path = specification
                .get("path")
                .and_then(crate::ValueRef::as_str)
                .ok_or_else(|| {
                    ResolveModuleError::Manifest(format!(
                        "dependency {name:?} must have a String path"
                    ))
                })?;
            let root = resolve_physical(&workspace_root.join(path.as_str()))?;
            dependencies_out.insert(name.to_owned(), root);
        }
    }
    if value.get("formats").is_some() {
        return Err(ResolveModuleError::Manifest(
            "manifest field \"formats\" is not supported; module format is determined by its file suffix"
                .into(),
        ));
    }
    Ok(())
}

fn insert_catalog_file(
    modules: &mut BTreeMap<String, ModuleCatalogEntry>,
    id: ModuleCName,
    physical: PathBuf,
    origin: ModuleCatalogOrigin,
    include_private: bool,
) {
    let visibility = visibility_for_path(&physical);
    if !include_private && visibility != ModuleVisibility::Public {
        return;
    }
    let Ok(format) = ModuleFormat::from_path(&physical) else {
        return;
    };
    modules.insert(
        id.to_string(),
        ModuleCatalogEntry {
            id,
            format,
            origin,
            visibility,
        },
    );
}

fn module_files(root: &Path) -> Result<Vec<(PathBuf, PathBuf)>, ResolveModuleError> {
    let mut files = Vec::new();
    collect_module_files(root, root, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

fn collect_module_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), ResolveModuleError> {
    let mut entries = std::fs::read_dir(directory)
        .map_err(|error| {
            ResolveModuleError::Io(format!("cannot read {}: {error}", directory.display()))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            ResolveModuleError::Io(format!("cannot read {}: {error}", directory.display()))
        })?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            ResolveModuleError::Io(format!("cannot inspect {}: {error}", path.display()))
        })?;
        if file_type.is_dir() {
            collect_module_files(root, &path, files)?;
            continue;
        }
        if !file_type.is_file() && !file_type.is_symlink() {
            continue;
        }
        let Ok(physical) = resolve_physical(&path) else {
            continue;
        };
        if !physical.starts_with(root) || !physical.is_file() {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .expect("walked path remains under root")
            .to_owned();
        files.push((relative, physical));
    }
    Ok(())
}

fn visibility_for_path(path: &Path) -> ModuleVisibility {
    if is_private_file_name(path) {
        ModuleVisibility::Private
    } else {
        ModuleVisibility::Public
    }
}

fn canonical_path_for_physical(path: &Path) -> Result<PathBuf, ResolveModuleError> {
    validate_module_filename(path)?;
    if path.extension().and_then(|extension| extension.to_str()) == Some("telora") {
        let mut canonical = path.to_owned();
        canonical.set_extension("");
        Ok(canonical)
    } else {
        Ok(path.to_owned())
    }
}

fn resolve_selector(
    root: &Path,
    selector: PathBuf,
    original: &str,
) -> Result<(PathBuf, PathBuf), ResolveModuleError> {
    let extension = selector
        .extension()
        .and_then(|extension| extension.to_str());
    let (canonical, relative) = match extension {
        None => {
            let mut physical = selector.clone();
            physical.set_extension("telora");
            (selector, physical)
        }
        Some("telora") => {
            return Err(ResolveModuleError::InvalidImport(format!(
                "{original}; Telora module selectors must omit .telora"
            )));
        }
        Some("json" | "yaml" | "yml" | "toml") => (selector.clone(), selector),
        Some(_) => return Err(ResolveModuleError::InvalidModuleSuffix(original.into())),
    };
    validate_module_filename(&relative)?;
    Ok((canonical, resolve_physical(&root.join(relative))?))
}

fn validate_module_filename(path: &Path) -> Result<(), ResolveModuleError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(ResolveModuleError::NonUtf8Path)?;
    if name.is_empty() || name.starts_with('.') || name.matches('.').count() > 1 {
        return Err(ResolveModuleError::InvalidModuleSuffix(name.into()));
    }
    Ok(())
}

fn logical_root(
    workspace_root: &Path,
    source_root: &Path,
    crate_name: &str,
    value: &str,
) -> Result<(ModuleCName, PathBuf), ResolveModuleError> {
    let parse = |prefix: &str| -> Result<PathBuf, ResolveModuleError> {
        let raw = value
            .strip_prefix(prefix)
            .ok_or_else(|| ResolveModuleError::InvalidImport(value.into()))?;
        if raw.is_empty() || raw.starts_with('/') {
            return Err(ResolveModuleError::InvalidImport(value.into()));
        }
        let path = lexical_normalize_relative(Path::new(raw))
            .ok_or_else(|| ResolveModuleError::InvalidImport(value.into()))?;
        if path.to_string_lossy() != raw {
            return Err(ResolveModuleError::InvalidImport(value.into()));
        }
        Ok(path)
    };
    if value.starts_with("@src/") {
        let selector = parse("@src/")?;
        let (path, physical) = resolve_selector(source_root, selector, value)?;
        if path
            .components()
            .next()
            .is_some_and(|part| part.as_os_str() == "bin")
        {
            return Err(ResolveModuleError::InvalidImport(value.into()));
        }
        if path
            .components()
            .next()
            .is_some_and(|part| part.as_os_str() == "entry")
        {
            return Err(ResolveModuleError::EntryModuleRoot);
        }
        return Ok((
            ModuleCName::Source {
                owner: crate_name.to_owned(),
                path: path.clone(),
            },
            physical,
        ));
    }
    if value.starts_with("@bin/") {
        let selector = parse("@bin/")?;
        if selector.components().count() != 1 {
            return Err(ResolveModuleError::InvalidImport(
                "binary roots support files only".into(),
            ));
        }
        let binary_root = source_root.join("bin");
        let (path, physical) = resolve_selector(&binary_root, selector, value)?;
        return Ok((
            ModuleCName::Binary {
                owner: crate_name.to_owned(),
                path: path.clone(),
            },
            physical,
        ));
    }
    if value.starts_with("@test/") {
        let selector = parse("@test/")?;
        if selector.components().count() != 1 {
            return Err(ResolveModuleError::InvalidImport(
                "test roots support files only".into(),
            ));
        }
        let tests_root = workspace_root.join("tests");
        let (path, physical) = resolve_selector(&tests_root, selector, value)?;
        return Ok((
            ModuleCName::Test {
                owner: crate_name.to_owned(),
                path: path.clone(),
            },
            physical,
        ));
    }
    if value.starts_with('@') || value.starts_with(['.', '/']) || value.contains("//") {
        return Err(ResolveModuleError::InvalidImport(value.into()));
    }
    // Dependency path is resolved after the manifest is loaded.
    Ok((
        ModuleCName::builtin("<pending>"),
        workspace_root.join("<pending>"),
    ))
}

pub fn resolve_root_module(path: &Path) -> Result<ResolvedModule, ResolveModuleError> {
    ModuleResolver::for_root(path)?.resolve_root(path)
}

pub(crate) fn immediate_value(expression: &Expr) -> Result<DataWorld, ResolveModuleError> {
    fn lower(expression: &Expr, heap: &mut Heap) -> Result<Val, ResolveModuleError> {
        let value = match &expression.value {
            ExprKind::Int(value) => DecodedValue::Int(*value),
            ExprKind::Float(value) if value.is_finite() => DecodedValue::Float(*value),
            ExprKind::String(value) => heap.string(None, value.as_str()),
            ExprKind::Atom(name) => heap.atom(None, name),
            ExprKind::Array(values) => {
                let values = values
                    .iter()
                    .map(|value| lower(value, heap))
                    .collect::<Result<Vec<_>, _>>()?;
                DecodedValue::Array(heap.allocate(Object::Array(values.into_boxed_slice())))
            }
            ExprKind::Dict(fields) => {
                let mut entries = fields
                    .iter()
                    .map(|field| {
                        lower(&field.value.value, heap).map(|value| {
                            (
                                field
                                    .value
                                    .name
                                    .as_ref()
                                    .expect("manifest fields have names")
                                    .value
                                    .clone(),
                                value,
                            )
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                entries.sort_by(|left, right| left.0.cmp(&right.0));
                let names = entries.iter().map(|(name, _)| heap.intern(name)).collect();
                let values = entries
                    .into_iter()
                    .map(|(_, value)| value)
                    .collect::<Vec<_>>();
                let shape = heap.intern_shape(names);
                DecodedValue::Dict(heap.allocate(Object::Dict {
                    shape,
                    values: values.into_boxed_slice(),
                }))
            }
            ExprKind::Call { callee, arguments }
                if matches!(callee.value, ExprKind::Atom(_)) && arguments.len() == 1 =>
            {
                let ExprKind::Atom(tag) = &callee.value else {
                    unreachable!("guarded above")
                };
                let tag = Val::new(heap.atom(None, tag), Some(callee.location.into()));
                let payload = lower(&arguments[0], heap)?;
                DecodedValue::Tagged(heap.allocate(Object::Tagged { tag, payload }))
            }
            _ => {
                return Err(ResolveModuleError::Manifest(
                    "option accepts only immediate values".into(),
                ));
            }
        };
        Ok(Val::new(value, Some(expression.location.into())))
    }
    let mut heap = Heap::work();
    let root = lower(expression, &mut heap)?;
    Ok(DataWorld::new(heap, root))
}

fn absolute_normalized(path: &Path) -> Result<PathBuf, ResolveModuleError> {
    if path.as_os_str().is_empty() {
        return Err(ResolveModuleError::EmptyPath);
    }
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|error| ResolveModuleError::Io(error.to_string()))?
            .join(path)
    };
    Ok(lexical_normalize(&absolute))
}

fn infer_embedding_root(start: &Path) -> &Path {
    if start.file_name().is_some_and(|name| name == "bin")
        && start
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "src")
    {
        return start.parent().and_then(Path::parent).unwrap_or(start);
    }
    if start.file_name().is_some_and(|name| name == "tests") {
        return start.parent().unwrap_or(start);
    }
    start
}

fn resolve_physical(path: &Path) -> Result<PathBuf, ResolveModuleError> {
    let absolute = absolute_normalized(path)?;
    let resolved = match std::fs::canonicalize(&absolute) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => absolute,
        Err(error) => return Err(ResolveModuleError::Io(error.to_string())),
    };
    if resolved.to_str().is_none() {
        return Err(ResolveModuleError::NonUtf8Path);
    }
    Ok(resolved)
}

fn module_id_for_physical_root(
    root: &Path,
    workspace_root: &Path,
    source_root: &Path,
    crate_name: &str,
) -> Result<(ModuleCName, bool), ResolveModuleError> {
    let tests_root = resolve_physical(&workspace_root.join("tests"))?;
    let binary_root = resolve_physical(&source_root.join("bin"))?;
    let entry_root = resolve_physical(&source_root.join("entry"))?;
    if let Ok(path) = root.strip_prefix(&binary_root) {
        let path = validate_special_root_relative(path, root, "binary")?;
        return Ok((
            ModuleCName::Binary {
                owner: crate_name.to_owned(),
                path: canonical_path_for_physical(&path)?,
            },
            true,
        ));
    }
    if let Ok(path) = root.strip_prefix(&entry_root) {
        let path = validate_special_root_relative(path, root, "Entry")?;
        return Ok((
            ModuleCName::Entry {
                owner: crate_name.to_owned(),
                path: canonical_path_for_physical(&path)?,
            },
            true,
        ));
    }
    if let Ok(path) = root.strip_prefix(&tests_root) {
        let path = validate_special_root_relative(path, root, "test")?;
        return Ok((
            ModuleCName::Test {
                owner: crate_name.to_owned(),
                path: canonical_path_for_physical(&path)?,
            },
            true,
        ));
    }
    if let Ok(path) = root.strip_prefix(source_root) {
        let path = canonical_path_for_physical(&validate_root_relative(path, root)?)?;
        if path
            .components()
            .next()
            .is_some_and(|part| part.as_os_str() == "bin")
        {
            return Err(ResolveModuleError::InvalidImport(
                root.display().to_string(),
            ));
        }
        return Ok((
            ModuleCName::Source {
                owner: crate_name.to_owned(),
                path,
            },
            false,
        ));
    }
    Err(ResolveModuleError::InvalidImport(
        root.display().to_string(),
    ))
}

fn validate_root_relative(path: &Path, original: &Path) -> Result<PathBuf, ResolveModuleError> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(ResolveModuleError::InvalidImport(
            original.display().to_string(),
        ));
    }
    lexical_normalize_relative(path)
        .ok_or_else(|| ResolveModuleError::CrateEscape(original.display().to_string()))
}

fn validate_special_root_relative(
    path: &Path,
    original: &Path,
    kind: &str,
) -> Result<PathBuf, ResolveModuleError> {
    let path = validate_root_relative(path, original)?;
    if path.components().count() != 1 {
        return Err(ResolveModuleError::InvalidImport(format!(
            "{kind} roots support files only"
        )));
    }
    Ok(path)
}

fn is_private_file_name(path: &Path) -> bool {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.starts_with('_'))
}

fn logical_owner(name: &str) -> &str {
    name.split_once('/').map_or(name, |(owner, _)| owner)
}

fn is_private_logical_name(name: &str) -> bool {
    name.rsplit_once('/')
        .map_or(name, |(_, basename)| basename)
        .starts_with('_')
}

fn special_root_of_logical_name(name: &str) -> Option<&str> {
    let (_, path) = name.split_once('/')?;
    match path.split('/').next() {
        Some(root @ ("bin" | "entry" | "tests")) => Some(root),
        _ => None,
    }
}

pub(crate) fn is_public_builtin_name(name: &str) -> bool {
    !is_private_module_path(Path::new(name)) && special_root_of_logical_name(name).is_none()
}

fn reject_special_source_path(path: &Path, original: &str) -> Result<(), ResolveModuleError> {
    match path
        .components()
        .next()
        .and_then(|part| part.as_os_str().to_str())
    {
        Some("entry") => Err(ResolveModuleError::EntryModuleAccess(original.into())),
        Some("bin" | "tests") => Err(ResolveModuleError::InvalidImport(original.into())),
        _ => Ok(()),
    }
}

fn is_private_module_path(path: &Path) -> bool {
    is_private_file_name(path)
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    normalized.pop();
                }
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn lexical_normalize_relative(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    return None;
                }
                normalized.pop();
            }
            Component::Normal(component) => normalized.push(component),
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(normalized)
}

#[cfg(test)]
#[path = "module_id/tests/mod.rs"]
mod tests;
