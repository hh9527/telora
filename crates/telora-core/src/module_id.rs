use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};

use crate::ast::{Expr, ExprKind};
use crate::{Value, Vm};

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

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ModuleId {
    Main,
    Source(PathBuf),
    Builtin(String),
    Dependency { name: String, path: PathBuf },
}

impl ModuleId {
    pub fn builtin(name: impl Into<String>) -> Self {
        Self::Builtin(name.into())
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Main => formatter.write_str("@main"),
            Self::Source(path) => write!(formatter, "@src/{}", path.display()),
            Self::Builtin(name) => formatter.write_str(name),
            Self::Dependency { name, path } => write!(formatter, "{name}/{}", path.display()),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ResolvedModule {
    pub id: ModuleId,
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
    main_path: PathBuf,
    main_is_bin_entry: bool,
    dependencies: BTreeMap<String, PathBuf>,
    formats: BTreeMap<String, ModuleFormat>,
    builtins: BTreeMap<String, u32>,
    selected_entry: Option<ModuleId>,
    injected_modules: BTreeSet<String>,
    virtual_modules: BTreeSet<String>,
}

impl ModuleResolver {
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
        let main_is_bin_entry = start.file_name().and_then(|name| name.to_str()) == Some("bin-src");
        let inferred_root = if main_is_bin_entry {
            start.parent().unwrap_or(start)
        } else {
            start
        };
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
        let mut resolver = Self {
            workspace_root,
            source_root,
            main_path: resolve_physical(&root)?,
            main_is_bin_entry,
            dependencies: BTreeMap::new(),
            formats: BTreeMap::new(),
            builtins: BTreeMap::new(),
            selected_entry: None,
            injected_modules: BTreeSet::new(),
            virtual_modules: BTreeSet::new(),
        };
        let has_external_manifest = manifest.is_some();
        if let Some(manifest) = manifest {
            resolver.load_manifest(&manifest)?;
        }
        let source = match root_source {
            Some(source) => Some(source),
            None => match std::fs::read_to_string(&root) {
                Ok(source) => Some(source),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => {
                    return Err(ResolveModuleError::Io(format!(
                        "cannot read {}: {error}",
                        root.display()
                    )));
                }
            },
        };
        if let Some(source) = source {
            let mut sources = crate::SourceDatabase::default();
            let source_id = sources.add(root.display().to_string(), source);
            let parsed = crate::parser::parse_registered(&sources, source_id);
            let crate_options = parsed
                .options
                .iter()
                .filter(|option| option.key.value.starts_with("crate."))
                .collect::<Vec<_>>();
            if !crate_options.is_empty() {
                if has_external_manifest {
                    return Err(ResolveModuleError::Manifest(
                        "@main cannot use both telora-deps.json and embedded crate options".into(),
                    ));
                }
                let mut values = Vm::new();
                for option in crate_options {
                    let value = immediate_value(&option.value, &mut values)?;
                    resolver.apply_option(option.key.value.as_str(), &value)?;
                }
            }
        }
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
        entry: ModuleId,
        modules: impl IntoIterator<Item = String>,
    ) -> Self {
        self.selected_entry = Some(entry);
        self.injected_modules = modules.into_iter().collect();
        self
    }

    pub(crate) fn with_virtual_modules(
        mut self,
        modules: impl IntoIterator<Item = String>,
    ) -> Self {
        self.virtual_modules = modules.into_iter().collect();
        self
    }

    pub fn resolve_root(&self, path: &Path) -> Result<ResolvedModule, ResolveModuleError> {
        let path = resolve_physical(path)?;
        if is_private_file_name(&path) {
            return Err(ResolveModuleError::PrivateModuleRoot);
        }
        let format = self.format_for(&ModuleId::Main, &path)?;
        Ok(ResolvedModule {
            id: ModuleId::Main,
            format,
            authority: ModuleAuthority::Ordinary,
            physical_path: Some(path),
        })
    }

    pub fn resolve_import(
        &self,
        importer: &ModuleId,
        target: &str,
    ) -> Result<ResolvedModule, ResolveModuleError> {
        if target.is_empty() {
            return Err(ResolveModuleError::EmptyPath);
        }
        let privileged = self.selected_entry.as_ref() == Some(importer);
        if self.virtual_modules.contains(target) {
            return Ok(ResolvedModule {
                id: ModuleId::builtin(target),
                format: ModuleFormat::Telora,
                authority: ModuleAuthority::RuntimeSystem,
                physical_path: None,
            });
        }
        if self.injected_modules.contains(target) {
            if !privileged {
                return Err(ResolveModuleError::PrivateModuleAccess(target.into()));
            }
            return Ok(ResolvedModule {
                id: ModuleId::builtin(target),
                format: ModuleFormat::Telora,
                authority: ModuleAuthority::RuntimeSystem,
                physical_path: None,
            });
        }
        if !target.starts_with(['.', '@'])
            && let Some(_registration_id) = self.builtins.get(target)
        {
            if target.starts_with("entry/") && !privileged {
                return Err(ResolveModuleError::InvalidImport(target.into()));
            }
            return Ok(ResolvedModule {
                id: ModuleId::builtin(target),
                format: ModuleFormat::Telora,
                authority: ModuleAuthority::RuntimeSystem,
                physical_path: None,
            });
        }
        if target.starts_with("entry/") {
            return Err(ResolveModuleError::InvalidImport(target.into()));
        }
        if target == "@main" {
            if !privileged {
                return Err(ResolveModuleError::InvalidImport(target.into()));
            }
            let format = self.format_for(&ModuleId::Main, &self.main_path)?;
            return Ok(ResolvedModule {
                id: ModuleId::Main,
                format,
                authority: ModuleAuthority::Ordinary,
                physical_path: Some(self.main_path.clone()),
            });
        }
        if target.starts_with("@main/") {
            return Err(ResolveModuleError::InvalidImport(target.into()));
        }
        if let Some(path) = target.strip_prefix("@src/") {
            return self.resolve_in_owner(importer, Path::new(path), target);
        }
        if target.starts_with("./") || target.starts_with("../") {
            return match importer {
                ModuleId::Main => {
                    if self.main_is_bin_entry {
                        return Err(ResolveModuleError::InvalidImport(format!(
                            "{target}; bin-src entries must import crate sources with @src/..."
                        )));
                    }
                    let logical = lexical_normalize_relative(Path::new(target))
                        .ok_or_else(|| ResolveModuleError::CrateEscape(target.into()))?;
                    self.resolve_source(logical, target)
                }
                ModuleId::Source(path) => {
                    let logical = lexical_normalize_relative(
                        &path.parent().unwrap_or_else(|| Path::new("")).join(target),
                    )
                    .ok_or_else(|| ResolveModuleError::CrateEscape(target.into()))?;
                    self.resolve_source(logical, target)
                }
                ModuleId::Dependency { name, path } => {
                    let logical = lexical_normalize_relative(
                        &path.parent().unwrap_or_else(|| Path::new("")).join(target),
                    )
                    .ok_or_else(|| ResolveModuleError::CrateEscape(target.into()))?;
                    self.resolve_dependency_parts(name, logical, target, true)
                }
                ModuleId::Builtin(_) => Err(ResolveModuleError::InvalidImport(
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
        self.apply_manifest(&value)
    }

    fn apply_manifest(&mut self, value: &Value) -> Result<(), ResolveModuleError> {
        let Value::Dict(manifest_value) = value else {
            return Err(ResolveModuleError::Manifest(
                "dependency manifest must be a JSON object".into(),
            ));
        };
        if let Some(dependencies) = manifest_value.get("dependencies") {
            let Value::Dict(dependencies) = dependencies else {
                return Err(ResolveModuleError::Manifest(
                    "manifest field \"dependencies\" must be an object".into(),
                ));
            };
            for (name, specification) in dependencies
                .shape()
                .fields()
                .iter()
                .zip(dependencies.values())
            {
                let path = match specification {
                    Value::Dict(specification) => match specification.get("path") {
                        Some(Value::String(path)) => path.as_ref(),
                        _ => {
                            return Err(ResolveModuleError::Manifest(format!(
                                "dependency {name:?} must have a String path"
                            )));
                        }
                    },
                    _ => {
                        return Err(ResolveModuleError::Manifest(format!(
                            "dependency {name:?} must be an object"
                        )));
                    }
                };
                let root = resolve_physical(&self.workspace_root.join(path))?;
                self.dependencies.insert(name.to_owned(), root);
            }
        }
        if let Some(formats) = manifest_value.get("formats") {
            let Value::Dict(formats) = formats else {
                return Err(ResolveModuleError::Manifest(
                    "manifest field \"formats\" must be an object".into(),
                ));
            };
            for (module, format) in formats.shape().fields().iter().zip(formats.values()) {
                let Value::String(format) = format else {
                    return Err(ResolveModuleError::Manifest(format!(
                        "format override {module:?} must be a String"
                    )));
                };
                self.formats
                    .insert(module.to_owned(), ModuleFormat::parse(format.as_ref())?);
            }
        }
        Ok(())
    }

    fn apply_option(&mut self, key: &str, value: &Value) -> Result<(), ResolveModuleError> {
        let Value::Dict(fields) = value else {
            return Err(ResolveModuleError::Manifest(format!(
                "option {key:?} must contain a Dict"
            )));
        };
        match key {
            "crate.dependency" => {
                let name = match fields.get("name") {
                    Some(Value::String(name)) => name.as_ref(),
                    _ => {
                        return Err(ResolveModuleError::Manifest(
                            "crate.dependency field \"name\" must be a String".into(),
                        ));
                    }
                };
                let path = match fields.get("source") {
                    Some(Value::Tagged { tag, payload }) if tag.name() == "Path" => {
                        let Value::Dict(source) = payload.as_ref() else {
                            return Err(ResolveModuleError::Manifest(
                                "crate.dependency Path payload must be a Dict".into(),
                            ));
                        };
                        match source.get("path") {
                            Some(Value::String(path)) => path.as_ref(),
                            _ => {
                                return Err(ResolveModuleError::Manifest(
                                    "crate.dependency Path field \"path\" must be a String".into(),
                                ));
                            }
                        }
                    }
                    _ => {
                        return Err(ResolveModuleError::Manifest(
                            "crate.dependency field \"source\" must be 'Path({path: String})"
                                .into(),
                        ));
                    }
                };
                if self.dependencies.contains_key(name) {
                    return Err(ResolveModuleError::Manifest(format!(
                        "duplicate crate dependency {name:?}"
                    )));
                }
                let root = resolve_physical(&self.workspace_root.join(path))?;
                self.dependencies.insert(name.into(), root);
            }
            "crate.format" => {
                let module = match fields.get("module") {
                    Some(Value::String(module)) => module.as_ref(),
                    _ => {
                        return Err(ResolveModuleError::Manifest(
                            "crate.format field \"module\" must be a String".into(),
                        ));
                    }
                };
                let format = match fields.get("format") {
                    Some(Value::String(format)) => ModuleFormat::parse(format.as_ref())?,
                    _ => {
                        return Err(ResolveModuleError::Manifest(
                            "crate.format field \"format\" must be a String".into(),
                        ));
                    }
                };
                if self.formats.insert(module.into(), format).is_some() {
                    return Err(ResolveModuleError::Manifest(format!(
                        "duplicate crate format module {module:?}"
                    )));
                }
            }
            _ => {
                return Err(ResolveModuleError::Manifest(format!(
                    "unknown pre-resolution option {key:?}"
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
        importer: &ModuleId,
        path: &Path,
        original: &str,
    ) -> Result<ResolvedModule, ResolveModuleError> {
        let path = lexical_normalize_relative(path)
            .ok_or_else(|| ResolveModuleError::CrateEscape(original.into()))?;
        match importer {
            ModuleId::Main | ModuleId::Source(_) => self.resolve_source(path, original),
            ModuleId::Dependency { name, .. } => {
                self.resolve_dependency_parts(name, path, original, true)
            }
            ModuleId::Builtin(_) => Err(ResolveModuleError::InvalidImport(
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
        if !physical.starts_with(&self.source_root) || physical == self.main_path {
            return Err(ResolveModuleError::CrateEscape(original.into()));
        }
        let id = ModuleId::Source(path);
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
        if !physical.starts_with(&source_root) {
            return Err(ResolveModuleError::CrateEscape(original.into()));
        }
        let private = is_private_module_path(&path);
        let id = ModuleId::Dependency {
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
        id: &ModuleId,
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

pub fn resolve_root_module(path: &Path) -> Result<ResolvedModule, ResolveModuleError> {
    ModuleResolver::for_root(path)?.resolve_root(path)
}

pub(crate) fn immediate_value(expression: &Expr, vm: &mut Vm) -> Result<Value, ResolveModuleError> {
    match &expression.value {
        ExprKind::Int(value) => Ok(Value::Int(*value)),
        ExprKind::Float(value) if value.is_finite() => Ok(Value::Float(*value)),
        ExprKind::String(value) => Ok(Value::string(value.as_str())),
        ExprKind::Atom(name) => Ok(Value::atom(name.clone())),
        ExprKind::Array(values) => values
            .iter()
            .map(|value| immediate_value(value, vm))
            .collect::<Result<Vec<_>, _>>()
            .map(|values| Value::Array(values.into())),
        ExprKind::Dict(fields) => {
            let entries = fields
                .iter()
                .map(|field| {
                    immediate_value(&field.value.value, vm).map(|value| {
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
            vm.make_dict(entries).map_err(ResolveModuleError::Manifest)
        }
        ExprKind::Call { callee, arguments }
            if matches!(callee.value, ExprKind::Atom(_)) && arguments.len() == 1 =>
        {
            let ExprKind::Atom(tag) = &callee.value else {
                unreachable!("guarded above")
            };
            Ok(Value::tagged(
                crate::value::Atom::named(tag.clone()),
                immediate_value(&arguments[0], vm)?,
            ))
        }
        _ => Err(ResolveModuleError::Manifest(
            "option accepts only immediate values".into(),
        )),
    }
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
        let id = ModuleId::Source(PathBuf::from("a file.yml"));
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
        std::fs::write(dependency.join("src/value.priv.json"), "0").unwrap();
        std::fs::write(dependency.join("src/bad.native.json"), "0").unwrap();
        std::fs::write(shadow.join("src/array"), "0").unwrap();
        std::fs::write(
            app.join("telora-deps.json"),
            r#"{"dependencies":{"dep":{"path":"../dependency"},"std":{"path":"../shadow"}},"formats":{"std/array":"telora","dep/bad.native.json":"json"}}"#,
        )
        .unwrap();

        let entry = ModuleId::builtin("entry/exec.telora");
        let resolver = ModuleResolver::for_root(&main)
            .unwrap()
            .with_builtins([("std/array".to_owned(), 5)])
            .with_entry_context(entry.clone(), std::iter::empty());
        let root = resolver.resolve_root(&main).unwrap();
        let builtin = resolver.resolve_import(&root.id, "std/array").unwrap();
        assert_eq!(builtin.id, ModuleId::builtin("std/array"));
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
            ModuleId::Dependency {
                name: "dep".into(),
                path: "internal.priv.telora".into(),
            }
        );
        assert!(matches!(
            resolver.resolve_import(&root.id, "dep/bad.native.json"),
            Err(ResolveModuleError::InvalidModuleSuffix(_))
        ));
        let dependency_module = resolver
            .resolve_import(&root.id, "dep/public.telora")
            .unwrap();
        assert!(matches!(
            resolver.resolve_import(&dependency_module.id, "@main"),
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
    fn crate_layout_resolves_main_source_and_contextual_source_roots() {
        let temporary =
            std::env::temp_dir().join(format!("telora-crate-layout-test-{}", std::process::id()));
        let app = temporary.join("app");
        let dependency = temporary.join("dependency");
        std::fs::create_dir_all(app.join("src/model")).unwrap();
        std::fs::create_dir_all(app.join("bin-src")).unwrap();
        std::fs::create_dir_all(dependency.join("src/model")).unwrap();
        let main = app.join("bin-src/tool.telora");
        std::fs::write(&main, "0").unwrap();
        std::fs::write(app.join("src/model/a.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/model/a.telora"), "0").unwrap();
        std::fs::write(
            app.join("telora-deps.json"),
            r#"{"dependencies":{"parser":{"path":"../dependency"}}}"#,
        )
        .unwrap();

        let entry = ModuleId::builtin("entry/exec.telora");
        let resolver = ModuleResolver::for_root(&main)
            .unwrap()
            .with_entry_context(entry.clone(), ["entry/opts.priv.telora".to_owned()]);
        let root = resolver.resolve_root(&main).unwrap();
        assert_eq!(root.id, ModuleId::Main);
        assert_eq!(root.to_string(), "@main");

        let local = resolver
            .resolve_import(&root.id, "@src/model/a.telora")
            .unwrap();
        assert_eq!(local.to_string(), "@src/model/a.telora");
        assert!(matches!(
            resolver.resolve_import(&root.id, "./model/a.telora"),
            Err(ResolveModuleError::InvalidImport(message))
                if message.contains("bin-src entries must import crate sources with @src/...")
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
            resolver.resolve_import(&local.id, "@main"),
            Err(ResolveModuleError::InvalidImport(_))
        ));
        assert_eq!(entry.to_string(), "entry/exec.telora");
        assert_eq!(
            resolver.resolve_import(&entry, "@main").unwrap().id,
            ModuleId::Main
        );
        assert_eq!(
            resolver
                .resolve_import(&entry, "entry/opts.priv.telora")
                .unwrap()
                .id,
            ModuleId::builtin("entry/opts.priv.telora")
        );
        assert!(matches!(
            resolver.resolve_import(&ModuleId::Main, "entry/opts.priv.telora"),
            Err(ResolveModuleError::PrivateModuleAccess(_))
        ));
        assert!(matches!(
            resolver.resolve_import(&ModuleId::Main, "@entry"),
            Err(ResolveModuleError::InvalidImport(_))
        ));
        assert!(matches!(
            resolver.resolve_import(&ModuleId::Main, "entry/exec.telora"),
            Err(ResolveModuleError::InvalidImport(_))
        ));
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn embedded_crate_options_mark_the_root_and_supply_dependencies() {
        let temporary = std::env::temp_dir().join(format!(
            "telora-embedded-manifest-test-{}",
            std::process::id()
        ));
        let app = temporary.join("app");
        let dependency = temporary.join("dependency");
        std::fs::create_dir_all(app.join("src")).unwrap();
        std::fs::create_dir_all(app.join("bin-src")).unwrap();
        std::fs::create_dir_all(dependency.join("src")).unwrap();
        let main = app.join("bin-src/tool.telora");
        std::fs::write(
            &main,
            r#"option "crate.dependency" {name: "dep", source: 'Path({path: "../dependency"})};
import "dep/value.telora" as value;
export { value };
option "crate.format" {module: "dep/raw.data", format: "json"};"#,
        )
        .unwrap();
        std::fs::write(
            dependency.join("src/value.telora"),
            "export let value = 42;",
        )
        .unwrap();
        std::fs::write(dependency.join("src/raw.data"), "{\"value\":42}").unwrap();

        let resolver = ModuleResolver::for_root(&main).unwrap();
        assert_eq!(resolver.workspace_root(), app);
        let root = resolver.resolve_root(&main).unwrap();
        assert_eq!(
            resolver
                .resolve_import(&root.id, "dep/value.telora")
                .unwrap()
                .id
                .to_string(),
            "dep/value.telora"
        );
        assert_eq!(
            resolver
                .resolve_import(&root.id, "dep/raw.data")
                .unwrap()
                .format,
            ModuleFormat::Json
        );

        std::fs::write(app.join("telora-deps.json"), r#"{"dependencies":{}}"#).unwrap();
        assert!(matches!(
            ModuleResolver::for_root(&main),
            Err(ResolveModuleError::Manifest(message))
                if message.contains("both telora-deps.json and embedded crate options")
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
