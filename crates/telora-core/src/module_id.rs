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
        if has_penultimate_suffix(path, "native") && path.extension() != Some("telora".as_ref()) {
            return Err(ResolveModuleError::InvalidModuleSuffix(
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("<non-UTF-8>")
                    .into(),
            ));
        }
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
pub enum ModuleAuthority {
    Ordinary,
    PackageSystem,
    RuntimeSystem,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ModuleCatalogOrigin {
    Crate,
    Dependency,
    Host,
}

impl ModuleCatalogOrigin {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Crate => "crate",
            Self::Dependency => "dependency",
            Self::Host => "host",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ModuleVisibility {
    Public,
    Private,
    Native,
    Entry,
}

impl ModuleVisibility {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
            Self::Native => "native",
            Self::Entry => "entry",
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
    Source(PathBuf),
    Binary(PathBuf),
    Test(PathBuf),
    Standalone(PathBuf),
    Builtin(String),
    Dependency { name: String, path: PathBuf },
}

impl ModuleCName {
    pub fn builtin(name: impl Into<String>) -> Self {
        Self::Builtin(name.into())
    }
}

impl fmt::Display for ModuleCName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(path) => write!(formatter, "@src/{}", path.display()),
            Self::Binary(path) => write!(formatter, "@bin/{}", path.display()),
            Self::Test(path) => write!(formatter, "@test/{}", path.display()),
            Self::Standalone(path) => write!(formatter, "@standalone/{}", path.display()),
            Self::Builtin(name) => formatter.write_str(name),
            Self::Dependency { name, path } => write!(formatter, "{name}/{}", path.display()),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ResolvedModule {
    pub id: ModuleCName,
    pub format: ModuleFormat,
    pub authority: ModuleAuthority,
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
    FormatConflict {
        configured: ModuleFormat,
        extension: ModuleFormat,
    },
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
                "module suffix .native is reserved for Telora source, not {name:?}"
            ),
            Self::CrateEscape(request) => {
                write!(formatter, "module import escapes its crate root: {request}")
            }
            Self::PrivateModuleAccess(request) => write!(
                formatter,
                "private module {request:?} can only be imported from the same crate"
            ),
            Self::PrivateModuleRoot => {
                formatter.write_str("a private module cannot be used as the root module")
            }
            Self::EntryModuleAccess(request) => write!(
                formatter,
                "Entry module {request:?} can only be selected by telora run-with"
            ),
            Self::EntryModuleRoot => {
                formatter.write_str("an .entry.telora module cannot be used as an ordinary root")
            }
            Self::Manifest(message) | Self::Io(message) => formatter.write_str(message),
            Self::FormatConflict {
                configured,
                extension,
            } => write!(
                formatter,
                "configured format {} conflicts with extension format {}",
                configured.name(),
                extension.name()
            ),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ModuleResolver {
    workspace_root: PathBuf,
    source_root: PathBuf,
    root_path: PathBuf,
    root_id: ModuleCName,
    dependencies: BTreeMap<String, PathBuf>,
    formats: BTreeMap<String, ModuleFormat>,
    builtins: BTreeMap<String, u32>,
    selected_entry: Option<ModuleCName>,
    injected_modules: BTreeSet<String>,
}

impl ModuleResolver {
    pub fn standalone(root_module: &Path) -> Result<Self, ResolveModuleError> {
        let root_path = resolve_physical(&absolute_normalized(root_module)?)?;
        let workspace_root = root_path
            .parent()
            .ok_or_else(|| ResolveModuleError::Io("standalone module has no parent".into()))?
            .to_owned();
        let name = root_path
            .file_name()
            .ok_or_else(|| ResolveModuleError::InvalidImport(root_path.display().to_string()))?;
        let mut resolver = Self {
            workspace_root: workspace_root.clone(),
            source_root: workspace_root,
            root_path: root_path.clone(),
            root_id: ModuleCName::Standalone(PathBuf::from(name)),
            dependencies: BTreeMap::new(),
            formats: BTreeMap::new(),
            builtins: BTreeMap::new(),
            selected_entry: None,
            injected_modules: BTreeSet::new(),
        };
        let source = std::fs::read_to_string(&root_path).map_err(|error| {
            ResolveModuleError::Io(format!("cannot read {}: {error}", root_path.display()))
        })?;
        let mut sources = crate::SourceDatabase::default();
        let source_id = sources.add(root_path.display().to_string(), source);
        let parsed = crate::parser::parse_registered(&sources, source_id);
        for option in parsed
            .options
            .iter()
            .filter(|option| option.key.value.starts_with("crate."))
        {
            let value = immediate_value(&option.value)?;
            resolver.apply_standalone_option(&option.key.value, value.value())?;
        }
        Ok(resolver)
    }

    pub fn from_cwd(cwd: &Path, root_id: &str) -> Result<Self, ResolveModuleError> {
        let (manifest, workspace_root, source_root) = workspace_layout(cwd)?;
        let (id, path) = logical_root(&workspace_root, &source_root, root_id)?;
        let root_path = resolve_physical(&path)?;
        let expected_root = match &id {
            ModuleCName::Source(_) => source_root.clone(),
            ModuleCName::Binary(_) => resolve_physical(&source_root.join("bin"))?,
            ModuleCName::Test(_) => resolve_physical(&workspace_root.join("tests"))?,
            _ => workspace_root.clone(),
        };
        if !root_path.starts_with(&expected_root) {
            return Err(ResolveModuleError::CrateEscape(root_id.into()));
        }
        if is_private_file_name(&root_path) {
            return Err(ResolveModuleError::PrivateModuleRoot);
        }
        let mut resolver = Self {
            workspace_root,
            source_root,
            root_path,
            root_id: id,
            dependencies: BTreeMap::new(),
            formats: BTreeMap::new(),
            builtins: BTreeMap::new(),
            selected_entry: None,
            injected_modules: BTreeSet::new(),
        };
        resolver.load_manifest(&manifest)?;
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
        let mut modules = BTreeMap::new();

        for (path, physical) in module_files(&source_root)? {
            let id = if let Ok(binary) = path.strip_prefix("bin") {
                if binary.as_os_str().is_empty() {
                    continue;
                }
                ModuleCName::Binary(binary.to_owned())
            } else {
                ModuleCName::Source(path.clone())
            };
            insert_catalog_file(
                &mut modules,
                id,
                physical,
                ModuleCatalogOrigin::Crate,
                true,
                &config.formats,
            );
        }

        let tests = workspace_root.join("tests");
        if tests.is_dir() {
            let tests = resolve_physical(&tests)?;
            for (path, physical) in module_files(&tests)? {
                insert_catalog_file(
                    &mut modules,
                    ModuleCName::Test(path),
                    physical,
                    ModuleCatalogOrigin::Crate,
                    true,
                    &config.formats,
                );
            }
        }

        for (name, root) in &config.dependencies {
            let source_candidate = root.join("src");
            let source_root = if source_candidate.is_dir() {
                resolve_physical(&source_candidate)?
            } else {
                root.clone()
            };
            for (path, physical) in module_files(&source_root)? {
                if path
                    .components()
                    .next()
                    .is_some_and(|part| matches!(part.as_os_str().to_str(), Some("bin" | "tests")))
                    || is_private_module_path(&path)
                {
                    continue;
                }
                insert_catalog_file(
                    &mut modules,
                    ModuleCName::Dependency {
                        name: name.clone(),
                        path,
                    },
                    physical,
                    ModuleCatalogOrigin::Dependency,
                    false,
                    &config.formats,
                );
            }
        }

        for (name, _) in builtins {
            if is_private_module_path(Path::new(&name)) {
                continue;
            }
            let id = ModuleCName::builtin(name);
            modules.insert(
                id.to_string(),
                ModuleCatalogEntry {
                    id,
                    format: ModuleFormat::Telora,
                    origin: ModuleCatalogOrigin::Host,
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
        matches!(self.root_id, ModuleCName::Standalone(_))
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
        let resolved_root = resolve_physical(&root)?;
        let (root_id, _root_is_entry) = module_id_for_physical_root(
            &resolved_root,
            &workspace_root,
            &source_root,
            manifest.is_some(),
        )?;
        let mut resolver = Self {
            workspace_root,
            source_root,
            root_path: resolved_root,
            root_id,
            dependencies: BTreeMap::new(),
            formats: BTreeMap::new(),
            builtins: BTreeMap::new(),
            selected_entry: None,
            injected_modules: BTreeSet::new(),
        };
        if let Some(manifest) = manifest {
            resolver.load_manifest(&manifest)?;
        }
        let _ = root_source;
        Ok(resolver)
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn with_builtins(mut self, builtins: impl IntoIterator<Item = (String, u32)>) -> Self {
        self.builtins = builtins.into_iter().collect();
        self
    }

    pub fn with_entry_context(
        mut self,
        entry: ModuleCName,
        modules: impl IntoIterator<Item = String>,
    ) -> Self {
        self.selected_entry = Some(entry);
        self.injected_modules = modules.into_iter().collect();
        self
    }

    pub fn resolve_root(&self, path: &Path) -> Result<ResolvedModule, ResolveModuleError> {
        let path = resolve_physical(path)?;
        if path != self.root_path {
            if matches!(self.root_id, ModuleCName::Standalone(_)) {
                let name = path
                    .file_name()
                    .ok_or_else(|| ResolveModuleError::InvalidImport(path.display().to_string()))?;
                let id = ModuleCName::Standalone(PathBuf::from(name));
                return Ok(ResolvedModule {
                    format: self.format_for(&id, &path)?,
                    id,
                    authority: ModuleAuthority::Ordinary,
                    physical_path: Some(path),
                });
            }
            return Err(ResolveModuleError::InvalidImport(format!(
                "resolver root is {}, not {}",
                self.root_path.display(),
                path.display()
            )));
        }
        if is_entry_module_path(&path) {
            return Err(ResolveModuleError::EntryModuleRoot);
        }
        if is_private_file_name(&path) {
            return Err(ResolveModuleError::PrivateModuleRoot);
        }
        let format = self.format_for(&self.root_id, &path)?;
        Ok(ResolvedModule {
            id: self.root_id.clone(),
            format,
            authority: ModuleAuthority::Ordinary,
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
        if target.ends_with(".entry.telora") && !privileged {
            return Err(ResolveModuleError::EntryModuleAccess(target.into()));
        }
        if self.injected_modules.contains(target) {
            if !privileged {
                return Err(ResolveModuleError::PrivateModuleAccess(target.into()));
            }
            return Ok(ResolvedModule {
                id: ModuleCName::builtin(target),
                format: ModuleFormat::Telora,
                authority: ModuleAuthority::RuntimeSystem,
                physical_path: None,
            });
        }
        if !target.starts_with(['.', '@'])
            && self.builtins.contains_key(target)
            && target.ends_with(".native.telora")
        {
            self.resolve_dependency(target, target, privileged)?;
            return Ok(ResolvedModule {
                id: ModuleCName::builtin(target),
                format: ModuleFormat::Telora,
                authority: ModuleAuthority::RuntimeSystem,
                physical_path: None,
            });
        }
        if !target.starts_with(['.', '@']) && self.builtins.contains_key(target) {
            return Ok(ResolvedModule {
                id: ModuleCName::builtin(target),
                format: ModuleFormat::Telora,
                authority: ModuleAuthority::RuntimeSystem,
                physical_path: None,
            });
        }
        if target == self.root_id.to_string() {
            if !privileged {
                return Err(ResolveModuleError::InvalidImport(target.into()));
            }
            let format = self.format_for(&self.root_id, &self.root_path)?;
            return Ok(ResolvedModule {
                id: self.root_id.clone(),
                format,
                authority: ModuleAuthority::Ordinary,
                physical_path: Some(self.root_path.clone()),
            });
        }
        if target.starts_with("@bin/") || target.starts_with("@test/") {
            return Err(ResolveModuleError::InvalidImport(target.into()));
        }
        if let Some(path) = target.strip_prefix("@src/") {
            return self.resolve_in_owner(importer, Path::new(path), target);
        }
        if target.starts_with("./") || target.starts_with("../") {
            return match importer {
                ModuleCName::Binary(_) | ModuleCName::Test(_) => {
                    return Err(ResolveModuleError::InvalidImport(format!(
                        "{target}; binary and test roots must import crate sources with @src/..."
                    )));
                }
                ModuleCName::Standalone(_) => {
                    let logical = lexical_normalize_relative(Path::new(target))
                        .ok_or_else(|| ResolveModuleError::CrateEscape(target.into()))?;
                    self.resolve_source(logical, target)
                }
                ModuleCName::Source(path) => {
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
        self.resolve_dependency(target, target, privileged)
    }

    pub fn resolve_entry(&self, target: &str) -> Result<ResolvedModule, ResolveModuleError> {
        if !target.ends_with(".entry.telora") {
            return Err(ResolveModuleError::InvalidImport(format!(
                "Entry selector {target:?} must resolve to .entry.telora"
            )));
        }
        let importer = self.root_id.clone();
        self.clone()
            .with_entry_context(importer.clone(), std::iter::empty())
            .resolve_import(&importer, target)
    }

    fn load_manifest(&mut self, manifest: &Path) -> Result<(), ResolveModuleError> {
        let config = read_manifest_config(&self.workspace_root, manifest)?;
        self.dependencies = config.dependencies;
        self.formats = config.formats;
        Ok(())
    }

    fn apply_standalone_option(
        &mut self,
        key: &str,
        value: crate::ValueRef<'_>,
    ) -> Result<(), ResolveModuleError> {
        if value.kind() != crate::ValueKind::Dict {
            return Err(ResolveModuleError::Manifest(format!(
                "option {key:?} must contain a Dict"
            )));
        }
        match key {
            "crate.dependency" => {
                let name = value
                    .get("name")
                    .and_then(crate::ValueRef::as_str)
                    .map(|value| value.as_str().to_owned())
                    .ok_or_else(|| {
                        ResolveModuleError::Manifest(
                            "crate.dependency field \"name\" must be a String".into(),
                        )
                    })?;
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
                if self.dependencies.contains_key(&name) {
                    return Err(ResolveModuleError::Manifest(format!(
                        "duplicate crate dependency {name:?}"
                    )));
                }
                self.dependencies
                    .insert(name, resolve_physical(&self.workspace_root.join(&path))?);
            }
            "crate.format" => {
                let module = value
                    .get("module")
                    .and_then(crate::ValueRef::as_str)
                    .map(|value| value.as_str().to_owned())
                    .ok_or_else(|| {
                        ResolveModuleError::Manifest(
                            "crate.format field \"module\" must be a String".into(),
                        )
                    })?;
                let format = value
                    .get("format")
                    .and_then(crate::ValueRef::as_str)
                    .map(|value| ModuleFormat::parse(value.as_str()))
                    .transpose()?
                    .ok_or_else(|| {
                        ResolveModuleError::Manifest(
                            "crate.format field \"format\" must be a String".into(),
                        )
                    })?;
                if self.formats.insert(module.clone(), format).is_some() {
                    return Err(ResolveModuleError::Manifest(format!(
                        "duplicate crate format module {module:?}"
                    )));
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
            ModuleCName::Source(_)
            | ModuleCName::Binary(_)
            | ModuleCName::Test(_)
            | ModuleCName::Standalone(_) => self.resolve_source(path, original),
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
        let physical = resolve_physical(&self.source_root.join(&path))?;
        if !physical.starts_with(&self.source_root)
            || physical == self.root_path
            || path
                .components()
                .next()
                .is_some_and(|part| part.as_os_str() == "bin")
        {
            return Err(ResolveModuleError::CrateEscape(original.into()));
        }
        let id = ModuleCName::Source(path);
        let format = self.format_for(&id, &physical)?;
        Ok(ResolvedModule {
            id,
            format,
            authority: authority_for_path(&physical),
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
        let physical = resolve_physical(&source_root.join(&path))?;
        if !physical.starts_with(&source_root)
            || path
                .components()
                .next()
                .is_some_and(|part| part.as_os_str() == "bin")
        {
            return Err(ResolveModuleError::CrateEscape(original.into()));
        }
        let private = is_private_module_path(&path);
        let id = ModuleCName::Dependency {
            name: name.into(),
            path,
        };
        let format = self.format_for(&id, &physical)?;
        let module = ResolvedModule {
            id,
            format,
            authority: authority_for_path(&physical),
            physical_path: Some(physical),
        };
        if private && !allow_private {
            return Err(ResolveModuleError::PrivateModuleAccess(original.into()));
        }
        Ok(module)
    }

    fn format_for(
        &self,
        id: &ModuleCName,
        physical: &Path,
    ) -> Result<ModuleFormat, ResolveModuleError> {
        format_for(&self.formats, id, physical)
    }
}

#[derive(Default)]
struct ManifestConfig {
    dependencies: BTreeMap<String, PathBuf>,
    formats: BTreeMap<String, ModuleFormat>,
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
    let mut config = ManifestConfig::default();
    apply_manifest(
        workspace_root,
        value.value(),
        &mut config.dependencies,
        &mut config.formats,
    )?;
    Ok(config)
}

fn apply_manifest(
    workspace_root: &Path,
    value: crate::ValueRef<'_>,
    dependencies_out: &mut BTreeMap<String, PathBuf>,
    formats_out: &mut BTreeMap<String, ModuleFormat>,
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
    if let Some(formats) = value.get("formats") {
        let Some(modules) = formats.dict_fields() else {
            return Err(ResolveModuleError::Manifest(
                "manifest field \"formats\" must be an object".into(),
            ));
        };
        for module in modules {
            let format = formats
                .get(module)
                .and_then(crate::ValueRef::as_str)
                .ok_or_else(|| {
                    ResolveModuleError::Manifest(format!(
                        "format override {module:?} must be a String"
                    ))
                })?;
            formats_out.insert(module.to_owned(), ModuleFormat::parse(format.as_str())?);
        }
    }
    Ok(())
}

fn insert_catalog_file(
    modules: &mut BTreeMap<String, ModuleCatalogEntry>,
    id: ModuleCName,
    physical: PathBuf,
    origin: ModuleCatalogOrigin,
    include_private: bool,
    formats: &BTreeMap<String, ModuleFormat>,
) {
    let visibility = visibility_for_path(&physical);
    if !include_private && visibility != ModuleVisibility::Public {
        return;
    }
    let Ok(format) = format_for(formats, &id, &physical) else {
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
    if is_entry_module_path(path) {
        ModuleVisibility::Entry
    } else if is_package_system_source(path) {
        ModuleVisibility::Native
    } else if has_penultimate_suffix(path, "priv") {
        ModuleVisibility::Private
    } else {
        ModuleVisibility::Public
    }
}

fn is_entry_module_path(path: &Path) -> bool {
    has_penultimate_suffix(path, "entry")
}

fn format_for(
    formats: &BTreeMap<String, ModuleFormat>,
    id: &ModuleCName,
    physical: &Path,
) -> Result<ModuleFormat, ResolveModuleError> {
    let configured = formats.get(&id.to_string()).copied();
    let extension = ModuleFormat::from_path(physical);
    match (configured, extension) {
        (Some(_), Err(error @ ResolveModuleError::InvalidModuleSuffix(_))) => Err(error),
        (Some(configured), Ok(extension)) if configured != extension => {
            Err(ResolveModuleError::FormatConflict {
                configured,
                extension,
            })
        }
        (Some(configured), _) => Ok(configured),
        (None, extension) => extension,
    }
}

fn logical_root(
    workspace_root: &Path,
    source_root: &Path,
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
        if path.to_string_lossy() != raw || path.extension().is_none() {
            return Err(ResolveModuleError::InvalidImport(value.into()));
        }
        Ok(path)
    };
    if value.starts_with("@src/") {
        let path = parse("@src/")?;
        if path
            .components()
            .next()
            .is_some_and(|part| part.as_os_str() == "bin")
        {
            return Err(ResolveModuleError::InvalidImport(value.into()));
        }
        return Ok((ModuleCName::Source(path.clone()), source_root.join(path)));
    }
    if value.starts_with("@bin/") {
        let path = parse("@bin/")?;
        return Ok((
            ModuleCName::Binary(path.clone()),
            source_root.join("bin").join(path),
        ));
    }
    if value.starts_with("@test/") {
        let path = parse("@test/")?;
        return Ok((
            ModuleCName::Test(path.clone()),
            workspace_root.join("tests").join(path),
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
    crate_mode: bool,
) -> Result<(ModuleCName, bool), ResolveModuleError> {
    if !crate_mode {
        let name = root
            .file_name()
            .ok_or_else(|| ResolveModuleError::InvalidImport(root.display().to_string()))?;
        return Ok((ModuleCName::Standalone(PathBuf::from(name)), true));
    }
    let tests_root = resolve_physical(&workspace_root.join("tests"))?;
    let binary_root = resolve_physical(&source_root.join("bin"))?;
    if let Ok(path) = root.strip_prefix(&binary_root) {
        return Ok((
            ModuleCName::Binary(validate_root_relative(path, root)?),
            true,
        ));
    }
    if let Ok(path) = root.strip_prefix(&tests_root) {
        return Ok((ModuleCName::Test(validate_root_relative(path, root)?), true));
    }
    if let Ok(path) = root.strip_prefix(source_root) {
        let path = validate_root_relative(path, root)?;
        if path
            .components()
            .next()
            .is_some_and(|part| part.as_os_str() == "bin")
        {
            return Err(ResolveModuleError::InvalidImport(
                root.display().to_string(),
            ));
        }
        return Ok((ModuleCName::Source(path), false));
    }
    let name = root
        .file_name()
        .ok_or_else(|| ResolveModuleError::InvalidImport(root.display().to_string()))?;
    Ok((ModuleCName::Standalone(PathBuf::from(name)), true))
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

fn is_private_file_name(path: &Path) -> bool {
    has_penultimate_suffix(path, "priv") || is_package_system_source(path)
}

fn is_private_module_path(path: &Path) -> bool {
    is_private_file_name(path)
}

fn has_penultimate_suffix(path: &Path, suffix: &str) -> bool {
    path.file_stem()
        .and_then(|stem| Path::new(stem).extension())
        .and_then(|extension| extension.to_str())
        == Some(suffix)
}

fn is_package_system_source(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()) == Some("telora")
        && has_penultimate_suffix(path, "native")
}

fn authority_for_path(path: &Path) -> ModuleAuthority {
    if is_package_system_source(path) {
        ModuleAuthority::PackageSystem
    } else {
        ModuleAuthority::Ordinary
    }
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
