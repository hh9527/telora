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

    fn load_manifest(&mut self, manifest: &Path) -> Result<(), ResolveModuleError> {
        let source = std::fs::read_to_string(manifest).map_err(|error| {
            ResolveModuleError::Manifest(format!("cannot read {}: {error}", manifest.display()))
        })?;
        let value =
            crate::json::parse_json(&manifest.display().to_string(), &source).map_err(|error| {
                ResolveModuleError::Manifest(format!("invalid {}: {error}", manifest.display()))
            })?;
        self.apply_manifest(value.value())
    }

    fn apply_manifest(&mut self, value: crate::ValueRef<'_>) -> Result<(), ResolveModuleError> {
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
                let root = resolve_physical(&self.workspace_root.join(path.as_str()))?;
                self.dependencies.insert(name.to_owned(), root);
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
                self.formats
                    .insert(module.to_owned(), ModuleFormat::parse(format.as_str())?);
            }
        }
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
        let configured = self.formats.get(&id.to_string()).copied();
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
mod tests {
    use super::*;

    #[test]
    fn formats_logical_ids_without_physical_paths() {
        let id = ModuleCName::Source(PathBuf::from("a file.yml"));
        assert_eq!(id.to_string(), "@src/a file.yml");
        assert_eq!(
            ModuleFormat::from_path(Path::new("a.yaml")),
            Ok(ModuleFormat::Yaml)
        );
        assert_eq!(
            ModuleFormat::from_path(Path::new("a.yml")),
            Ok(ModuleFormat::Yaml)
        );
        assert!(ModuleFormat::from_path(Path::new("a.JSON")).is_err());
    }

    #[test]
    fn path_dependencies_keep_logical_identity() {
        let temporary =
            std::env::temp_dir().join(format!("telora-module-id-test-{}", std::process::id()));
        std::fs::create_dir_all(temporary.join("app")).unwrap();
        std::fs::create_dir_all(temporary.join("models")).unwrap();
        std::fs::write(temporary.join("app/main.telora"), "0").unwrap();
        std::fs::write(
            temporary.join("telora-deps.json"),
            r#"{"dependencies":{"models":{"path":"models"}}}"#,
        )
        .unwrap();
        std::fs::write(temporary.join("models/user.telora"), "0").unwrap();
        let resolver = ModuleResolver::for_root(&temporary.join("app/main.telora")).unwrap();
        let root = resolver
            .resolve_root(&temporary.join("app/main.telora"))
            .unwrap();
        let dependency = resolver
            .resolve_import(&root.id, "models/user.telora")
            .unwrap();
        assert_eq!(dependency.id.to_string(), "models/user.telora");
        assert_eq!(dependency.format, ModuleFormat::Telora);
        assert!(matches!(
            resolver.resolve_import(&root.id, ""),
            Err(ResolveModuleError::EmptyPath)
        ));
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn builtins_precede_vtops_and_private_modules_stay_crate_local() {
        let temporary = std::env::temp_dir().join(format!(
            "telora-module-authority-test-{}",
            std::process::id()
        ));
        let app = temporary.join("app");
        let dependency = temporary.join("dependency");
        let shadow = temporary.join("shadow");
        std::fs::create_dir_all(app.join("src")).unwrap();
        std::fs::create_dir_all(dependency.join("src")).unwrap();
        std::fs::create_dir_all(shadow.join("src")).unwrap();
        let main = app.join("main.telora");
        std::fs::write(&main, "0").unwrap();
        std::fs::write(app.join("src/local.priv.telora"), "0").unwrap();
        std::fs::write(app.join("src/host.native.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/public.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/internal.priv.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/service.native.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/value.priv.json"), "0").unwrap();
        std::fs::write(dependency.join("src/bad.native.json"), "0").unwrap();
        std::fs::write(shadow.join("src/array"), "0").unwrap();
        std::fs::write(
            app.join("telora-deps.json"),
            r#"{"dependencies":{"dep":{"path":"../dependency"},"std":{"path":"../shadow"}},"formats":{"std/array":"telora","dep/bad.native.json":"json"}}"#,
        )
        .unwrap();

        let entry = ModuleCName::builtin("host/run-entry.telora");
        let resolver = ModuleResolver::for_root(&main)
            .unwrap()
            .with_builtins([
                ("std/array".to_owned(), 5),
                ("dep/service.native.telora".to_owned(), 1_500),
            ])
            .with_entry_context(entry.clone(), std::iter::empty());
        let root = resolver.resolve_root(&main).unwrap();
        let builtin = resolver.resolve_import(&root.id, "std/array").unwrap();
        assert_eq!(builtin.id, ModuleCName::builtin("std/array"));
        assert_eq!(builtin.authority, ModuleAuthority::RuntimeSystem);

        let local = resolver
            .resolve_import(&root.id, "@src/local.priv.telora")
            .unwrap();
        assert_eq!(local.authority, ModuleAuthority::Ordinary);
        let native = resolver
            .resolve_import(&root.id, "@src/host.native.telora")
            .unwrap();
        assert_eq!(native.authority, ModuleAuthority::PackageSystem);

        assert!(matches!(
            resolver.resolve_import(&root.id, "dep/internal.priv.telora"),
            Err(ResolveModuleError::PrivateModuleAccess(_))
        ));
        assert!(matches!(
            resolver.resolve_import(&root.id, "dep/value.priv.json"),
            Err(ResolveModuleError::PrivateModuleAccess(_))
        ));
        let entry_private = resolver
            .resolve_import(&entry, "dep/internal.priv.telora")
            .unwrap();
        assert_eq!(
            entry_private.id,
            ModuleCName::Dependency {
                name: "dep".into(),
                path: "internal.priv.telora".into(),
            }
        );
        assert!(matches!(
            resolver.resolve_import(&root.id, "dep/service.native.telora"),
            Err(ResolveModuleError::PrivateModuleAccess(_))
        ));
        let entry_native = resolver
            .resolve_import(&entry, "dep/service.native.telora")
            .unwrap();
        assert_eq!(entry_native.authority, ModuleAuthority::RuntimeSystem);
        assert_eq!(entry_native.to_string(), "dep/service.native.telora");
        assert!(matches!(
            resolver.resolve_import(&root.id, "dep/bad.native.json"),
            Err(ResolveModuleError::InvalidModuleSuffix(_))
        ));
        let dependency_module = resolver
            .resolve_import(&root.id, "dep/public.telora")
            .unwrap();
        assert!(matches!(
            resolver.resolve_import(&dependency_module.id, "@unknown"),
            Err(ResolveModuleError::InvalidImport(_))
        ));
        let internal = resolver
            .resolve_import(&dependency_module.id, "./internal.priv.telora")
            .unwrap();
        assert_eq!(internal.authority, ModuleAuthority::Ordinary);
        assert!(matches!(
            ModuleResolver::for_root(&app.join("src/local.priv.telora"))
                .and_then(|resolver| resolver.resolve_root(&app.join("src/local.priv.telora"))),
            Err(ResolveModuleError::PrivateModuleRoot)
        ));
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn crate_layout_resolves_binary_source_and_contextual_source_roots() {
        let temporary =
            std::env::temp_dir().join(format!("telora-crate-layout-test-{}", std::process::id()));
        let app = temporary.join("app");
        let dependency = temporary.join("dependency");
        std::fs::create_dir_all(app.join("src/model")).unwrap();
        std::fs::create_dir_all(app.join("src/bin")).unwrap();
        std::fs::create_dir_all(dependency.join("src/model")).unwrap();
        let main = app.join("src/bin/tool.telora");
        std::fs::write(&main, "0").unwrap();
        std::fs::write(app.join("src/model/a.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/model/a.telora"), "0").unwrap();
        std::fs::write(
            app.join("telora-deps.json"),
            r#"{"dependencies":{"parser":{"path":"../dependency"}}}"#,
        )
        .unwrap();

        let entry = ModuleCName::builtin("host/run-entry.telora");
        let resolver = ModuleResolver::for_root(&main)
            .unwrap()
            .with_entry_context(entry.clone(), ["std/rt.priv.telora".to_owned()]);
        let root = resolver.resolve_root(&main).unwrap();
        assert_eq!(root.id, ModuleCName::Binary(PathBuf::from("tool.telora")));
        assert_eq!(root.to_string(), "@bin/tool.telora");

        let local = resolver
            .resolve_import(&root.id, "@src/model/a.telora")
            .unwrap();
        assert_eq!(local.to_string(), "@src/model/a.telora");
        assert!(matches!(
            resolver.resolve_import(&root.id, "./model/a.telora"),
            Err(ResolveModuleError::InvalidImport(message))
                if message.contains("binary and test roots must import crate sources with @src/...")
        ));

        let dependency = resolver
            .resolve_import(&root.id, "parser/model/a.telora")
            .unwrap();
        assert_eq!(dependency.to_string(), "parser/model/a.telora");
        assert_eq!(
            resolver
                .resolve_import(&dependency.id, "@src/model/a.telora")
                .unwrap()
                .id,
            dependency.id
        );
        assert!(matches!(
            resolver.resolve_import(&local.id, "@bin/tool.telora"),
            Err(ResolveModuleError::InvalidImport(_))
        ));
        assert_eq!(entry.to_string(), "host/run-entry.telora");
        assert_eq!(
            resolver
                .resolve_import(&entry, "@bin/tool.telora")
                .unwrap()
                .id,
            ModuleCName::Binary(PathBuf::from("tool.telora"))
        );
        assert_eq!(
            resolver
                .resolve_import(&entry, "std/rt.priv.telora")
                .unwrap()
                .id,
            ModuleCName::builtin("std/rt.priv.telora")
        );
        assert!(matches!(
            resolver.resolve_import(&root.id, "std/rt.priv.telora"),
            Err(ResolveModuleError::PrivateModuleAccess(_))
        ));
        assert!(matches!(
            resolver.resolve_import(&root.id, "@entry"),
            Err(ResolveModuleError::InvalidImport(_))
        ));
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn logical_roots_reject_symlink_escapes() {
        use std::os::unix::fs::symlink;

        let temporary = std::env::temp_dir().join(format!(
            "telora-logical-root-symlink-test-{}",
            std::process::id()
        ));
        let app = temporary.join("app");
        std::fs::create_dir_all(app.join("src/bin")).unwrap();
        std::fs::create_dir_all(app.join("tests")).unwrap();
        std::fs::write(app.join("telora-deps.json"), "{}").unwrap();
        std::fs::write(temporary.join("outside.telora"), "export def output = 1;").unwrap();
        symlink(
            temporary.join("outside.telora"),
            app.join("src/bin/escape.telora"),
        )
        .unwrap();
        symlink(
            temporary.join("outside.telora"),
            app.join("tests/escape.telora"),
        )
        .unwrap();

        assert!(matches!(
            ModuleResolver::from_cwd(&app, "@bin/escape.telora"),
            Err(ResolveModuleError::CrateEscape(_))
        ));
        assert!(matches!(
            ModuleResolver::from_cwd(&app, "@test/escape.telora"),
            Err(ResolveModuleError::CrateEscape(_))
        ));
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn local_aliases_share_one_identity_and_formats_are_exact() {
        let temporary =
            std::env::temp_dir().join(format!("telora-module-alias-test-{}", std::process::id()));
        std::fs::create_dir_all(temporary.join("app/sub")).unwrap();
        let main = temporary.join("app/main.telora");
        let data = temporary.join("app/data.json");
        std::fs::write(&main, "0").unwrap();
        std::fs::write(&data, "{}").unwrap();
        let resolver = ModuleResolver::for_root(&main).unwrap();
        let root = resolver.resolve_root(&main).unwrap();
        let dotted = resolver
            .resolve_import(&root.id, "./sub/../data.json")
            .unwrap();
        let absolute = resolver.resolve_import(&root.id, "@src/data.json").unwrap();
        assert_eq!(dotted, absolute);
        assert_eq!(dotted.format, ModuleFormat::Json);
        assert!(ModuleFormat::from_path(Path::new("data.JSON")).is_err());
        assert!(ModuleFormat::from_path(Path::new("data")).is_err());
        assert!(ModuleFormat::from_path(Path::new("data.txt")).is_err());
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn json_manifest_validates_shape_and_exact_format_overrides() {
        let temporary = std::env::temp_dir().join(format!(
            "telora-module-manifest-test-{}",
            std::process::id()
        ));
        let app = temporary.join("app");
        let dependency = temporary.join("dependency");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&dependency).unwrap();
        let main = app.join("main.telora");
        std::fs::write(&main, "0").unwrap();
        std::fs::write(dependency.join("schema"), "{}").unwrap();
        std::fs::write(
            temporary.join("telora-deps.json"),
            r#"{
                "dependencies": {"dep": {"path": "dependency"}},
                "formats": {"dep/schema": "json"}
            }"#,
        )
        .unwrap();
        let resolver = ModuleResolver::for_root(&main).unwrap();
        let root = resolver.resolve_root(&main).unwrap();
        let schema = resolver.resolve_import(&root.id, "dep/schema").unwrap();
        assert_eq!(schema.format, ModuleFormat::Json);

        std::fs::write(
            temporary.join("telora-deps.json"),
            r#"{"dependencies": []}"#,
        )
        .unwrap();
        assert!(matches!(
            ModuleResolver::for_root(&main),
            Err(ResolveModuleError::Manifest(message))
                if message.contains("dependencies") && message.contains("object")
        ));

        std::fs::write(temporary.join("telora-deps.json"), "{").unwrap();
        assert!(matches!(
            ModuleResolver::for_root(&main),
            Err(ResolveModuleError::Manifest(message))
                if message.contains("invalid") && message.contains("telora-deps.json")
        ));
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn dependency_resolution_rejects_lexical_and_symlink_escapes() {
        use std::os::unix::fs::symlink;

        let temporary =
            std::env::temp_dir().join(format!("telora-module-escape-test-{}", std::process::id()));
        let app = temporary.join("app");
        let dependency = temporary.join("dependency");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&dependency).unwrap();
        std::fs::write(app.join("main.telora"), "0").unwrap();
        std::fs::write(temporary.join("outside.telora"), "0").unwrap();
        std::fs::write(
            temporary.join("telora-deps.json"),
            r#"{"dependencies":{"dep":{"path":"dependency"}}}"#,
        )
        .unwrap();
        symlink(
            temporary.join("outside.telora"),
            dependency.join("escape.telora"),
        )
        .unwrap();
        let resolver = ModuleResolver::for_root(&app.join("main.telora")).unwrap();
        let root = resolver.resolve_root(&app.join("main.telora")).unwrap();
        assert!(matches!(
            resolver.resolve_import(&root.id, "dep/../outside.telora"),
            Err(ResolveModuleError::CrateEscape(_))
        ));
        assert!(matches!(
            resolver.resolve_import(&root.id, "dep/escape.telora"),
            Err(ResolveModuleError::CrateEscape(_))
        ));
        std::fs::remove_dir_all(temporary).unwrap();
    }
}
