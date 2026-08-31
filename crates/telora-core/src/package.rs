use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::module_id::{ModuleFormat, validate_crate_name};

pub const CONFIG_FILE: &str = "telora-config.json";
pub const CRATE_FILE: &str = "telora-crate.json";
pub const LOCK_FILE: &str = "telora-lock.json";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfig {
    pub version: u32,
    pub members: Vec<PathBuf>,
    #[serde(default)]
    pub sources: BTreeMap<String, RemoteSource>,
    #[serde(default)]
    pub overrides: BTreeMap<String, PathOverride>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteSource {
    pub tarball: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PathOverride {
    pub path: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CrateManifest {
    pub name: String,
    pub modules: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModuleDeclaration {
    pub selector: String,
    pub logical_path: PathBuf,
    pub physical_path: PathBuf,
    pub format: ModuleFormat,
    pub kind: ModuleDeclarationKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UndeclaredModule {
    pub crate_name: String,
    pub selector: String,
    pub relative_path: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModuleDeclarationKind {
    Source,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceLock {
    pub version: u32,
    pub packages: BTreeMap<String, LockedPackage>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LockedPackage {
    pub source: LockedSource,
    pub modules: Vec<String>,
    pub dependencies: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum LockedSource {
    Workspace { workspace: PathBuf },
    Tarball { tarball: String },
}

#[derive(Clone, Debug)]
pub struct WorkspaceSpec {
    root: PathBuf,
    config: WorkspaceConfig,
    members: BTreeMap<String, ResolvedCrate>,
}

#[derive(Clone, Debug)]
pub struct ResolvedWorkspace {
    root: PathBuf,
    crates: BTreeMap<String, ResolvedCrate>,
    lock: WorkspaceLock,
}

#[derive(Clone, Debug)]
struct ResolvedCrate {
    root: PathBuf,
    manifest: CrateManifest,
    modules: BTreeMap<String, ModuleDeclaration>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageError(String);

impl PackageError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for PackageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PackageError {}

impl WorkspaceSpec {
    pub fn discover(start: &Path) -> Result<Self, PackageError> {
        let start = absolute(start)?;
        let search = if start.is_file() {
            start.parent().unwrap_or(&start)
        } else {
            &start
        };
        let ancestors = search.ancestors().collect::<Vec<_>>();
        let config_path = ancestors
            .iter()
            .map(|directory| directory.join(CONFIG_FILE))
            .find(|candidate| candidate.is_file())
            .ok_or_else(|| {
                if let Some(legacy) = ancestors
                    .iter()
                    .map(|directory| directory.join("telora-deps.json"))
                    .find(|candidate| candidate.is_file())
                {
                    return PackageError::new(format!(
                        "found legacy manifest {}; migrate package configuration to workspace {}, crate {}, and generated {}",
                        legacy.display(), CONFIG_FILE, CRATE_FILE, LOCK_FILE
                    ));
                }
                PackageError::new(format!(
                    "cannot find {CONFIG_FILE} from {} or its ancestors",
                    start.display()
                ))
            })?;
        let root = config_path
            .parent()
            .expect("workspace config has a parent")
            .to_owned();
        let config: WorkspaceConfig = read_json(&config_path)?;
        if config.version != 1 {
            return Err(PackageError::new(format!(
                "unsupported {} version {}; expected 1",
                config_path.display(),
                config.version
            )));
        }
        validate_source_catalog(&config)?;

        let mut members = BTreeMap::new();
        for member in &config.members {
            let member_root = contained_directory(&root, member, "workspace member")?;
            let resolved = read_crate(&member_root)?;
            if members
                .insert(resolved.manifest.name.clone(), resolved)
                .is_some()
            {
                return Err(PackageError::new(format!(
                    "duplicate workspace crate name in {}",
                    config_path.display()
                )));
            }
        }
        for name in config.sources.keys() {
            if members.contains_key(name) {
                return Err(PackageError::new(format!(
                    "crate {name:?} is both a workspace member and a remote source"
                )));
            }
        }

        Ok(Self {
            root,
            config,
            members,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config(&self) -> &WorkspaceConfig {
        &self.config
    }

    pub fn lock_path(&self) -> PathBuf {
        self.root.join(LOCK_FILE)
    }

    pub fn read_lock(&self) -> Result<WorkspaceLock, PackageError> {
        let path = self.lock_path();
        let lock: WorkspaceLock = read_json(&path)?;
        if lock.version != 1 {
            return Err(PackageError::new(format!(
                "unsupported {} version {}; expected 1",
                path.display(),
                lock.version
            )));
        }
        Ok(lock)
    }

    pub fn validate_existing_lock(&self) -> Result<WorkspaceLock, PackageError> {
        let lock = self.read_lock().map_err(|error| {
            PackageError::new(format!(
                "{error}; run `telora lock` to refresh the workspace lock"
            ))
        })?;
        validate_lock(&lock, self).map_err(|error| {
            PackageError::new(format!(
                "{error}; run `telora lock` to refresh the workspace lock"
            ))
        })?;
        Ok(lock)
    }

    pub fn remote_sources(&self) -> impl Iterator<Item = (&str, &RemoteSource)> {
        self.config
            .sources
            .iter()
            .map(|(name, source)| (name.as_str(), source))
    }

    pub fn imos_plan(&self, name: &str) -> Result<serde_json::Value, PackageError> {
        let source = self.config.sources.get(name).ok_or_else(|| {
            PackageError::new(format!("workspace has no remote crate named {name:?}"))
        })?;
        let plan_digest = package_digest(b"plan", &[name.as_bytes(), source.tarball.as_bytes()]);
        let download_digest = package_digest(b"download", &[source.tarball.as_bytes()]);
        Ok(serde_json::json!({
            "version": 1,
            "name": format!("telora-{}.json", &plan_digest[..40]),
            "key": format!("tp-{}", &plan_digest[..60]),
            "items": [{
                "name": "Telora crate archive",
                "key": format!("td-{}", &download_digest[..60]),
                "kind": {
                    "type": "UnpackDir",
                    "url": source.tarball,
                    "archive": "TarGzip",
                    "strip": 0,
                    "to": "."
                }
            }],
            "telora": {"crate": name, "source": source.tarball}
        }))
    }

    pub fn generate_lock(
        &self,
        remote_roots: &BTreeMap<String, PathBuf>,
    ) -> Result<WorkspaceLock, PackageError> {
        let crates = self.baseline_crates(remote_roots)?;
        validate_dependency_graph(&crates)?;
        let packages = crates
            .iter()
            .map(|(name, package)| {
                let source = if self.members.contains_key(name) {
                    LockedSource::Workspace {
                        workspace: package
                            .root
                            .strip_prefix(&self.root)
                            .expect("workspace members are contained")
                            .to_owned(),
                    }
                } else {
                    LockedSource::Tarball {
                        tarball: self.config.sources[name].tarball.clone(),
                    }
                };
                (
                    name.clone(),
                    LockedPackage {
                        source,
                        modules: package.modules.keys().cloned().collect(),
                        dependencies: package.manifest.dependencies.clone(),
                    },
                )
            })
            .collect();
        Ok(WorkspaceLock {
            version: 1,
            packages,
        })
    }

    pub fn write_lock(&self, lock: &WorkspaceLock) -> Result<(), PackageError> {
        let path = self.lock_path();
        let mut bytes = serde_json::to_vec_pretty(lock)
            .map_err(|error| PackageError::new(format!("cannot encode {LOCK_FILE}: {error}")))?;
        bytes.push(b'\n');
        let temporary = self
            .root
            .join(format!(".{LOCK_FILE}.{}.tmp", std::process::id()));
        fs::write(&temporary, bytes).map_err(|error| {
            PackageError::new(format!("cannot write {}: {error}", temporary.display()))
        })?;
        fs::rename(&temporary, &path).map_err(|error| {
            let _ = fs::remove_file(&temporary);
            PackageError::new(format!("cannot replace {}: {error}", path.display()))
        })
    }

    pub fn resolve(
        &self,
        remote_roots: &BTreeMap<String, PathBuf>,
        use_overrides: bool,
    ) -> Result<ResolvedWorkspace, PackageError> {
        let lock = self.validate_existing_lock()?;
        let mut crates = self.baseline_crates(remote_roots)?;
        for name in self.config.sources.keys() {
            let resolved = &crates[name];
            let locked = &lock.packages[name];
            if resolved.manifest.dependencies != locked.dependencies
                || resolved.modules.keys().cloned().collect::<Vec<_>>() != locked.modules
            {
                return Err(PackageError::new(format!(
                    "materialized crate {name:?} does not match {LOCK_FILE}"
                )));
            }
        }
        if use_overrides {
            for (name, source) in &self.config.overrides {
                let root = contained_directory(&self.root, &source.path, "override")?;
                let resolved = read_crate(&root)?;
                if resolved.manifest.name != *name {
                    return Err(PackageError::new(format!(
                        "crate {name:?} resolved to override manifest name {:?}",
                        resolved.manifest.name
                    )));
                }
                let locked = lock
                    .packages
                    .get(name)
                    .expect("validated lock contains every override source");
                if resolved.manifest.dependencies != locked.dependencies {
                    return Err(PackageError::new(format!(
                        "override crate {name:?} dependencies do not match {LOCK_FILE}"
                    )));
                }
                crates.insert(name.clone(), resolved);
            }
        }

        validate_dependency_graph(&crates)?;
        Ok(ResolvedWorkspace {
            root: self.root.clone(),
            crates,
            lock,
        })
    }

    fn baseline_crates(
        &self,
        remote_roots: &BTreeMap<String, PathBuf>,
    ) -> Result<BTreeMap<String, ResolvedCrate>, PackageError> {
        let mut crates = self.members.clone();
        for (name, source) in &self.config.sources {
            let root = remote_roots.get(name).ok_or_else(|| {
                PackageError::new(format!(
                    "crate {name:?} is not materialized from {}",
                    source.tarball
                ))
            })?;
            let root = installed_crate_root(root)?;
            let resolved = read_crate(&root)?;
            if resolved.manifest.name != *name {
                return Err(PackageError::new(format!(
                    "crate {name:?} resolved to manifest name {:?}",
                    resolved.manifest.name
                )));
            }
            crates.insert(name.clone(), resolved);
        }
        Ok(crates)
    }

    pub fn resolve_workspace_only(&self) -> Result<ResolvedWorkspace, PackageError> {
        self.resolve(&BTreeMap::new(), true)
    }
}

impl ResolvedWorkspace {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn lock(&self) -> &WorkspaceLock {
        &self.lock
    }

    pub fn crate_root(&self, name: &str) -> Option<&Path> {
        self.crates.get(name).map(|package| package.root.as_path())
    }

    pub fn crate_manifest(&self, name: &str) -> Option<&CrateManifest> {
        self.crates.get(name).map(|package| &package.manifest)
    }

    pub fn crates(&self) -> impl Iterator<Item = (&str, &Path)> {
        self.crates
            .iter()
            .map(|(name, package)| (name.as_str(), package.root.as_path()))
    }

    pub fn modules(&self, name: &str) -> Option<impl Iterator<Item = &ModuleDeclaration>> {
        Some(self.crates.get(name)?.modules.values())
    }

    pub fn declares_dependency(&self, owner: &str, dependency: &str) -> bool {
        owner == dependency
            || self.crates.get(owner).is_some_and(|package| {
                package
                    .manifest
                    .dependencies
                    .binary_search_by(|item| item.as_str().cmp(dependency))
                    .is_ok()
            })
    }

    pub fn module(&self, name: &str, selector: &str) -> Option<&ModuleDeclaration> {
        self.crates
            .get(name)
            .and_then(|package| package.modules.get(selector))
    }

    pub fn crate_for_path(&self, path: &Path) -> Result<&str, PackageError> {
        let path = absolute(path)?;
        let mut matches = self
            .crates
            .iter()
            .filter(|(_, package)| path.starts_with(&package.root))
            .map(|(name, package)| (name.as_str(), package.root.components().count()))
            .collect::<Vec<_>>();
        matches.sort_by_key(|(_, depth)| std::cmp::Reverse(*depth));
        matches.first().map(|(name, _)| *name).ok_or_else(|| {
            PackageError::new(format!(
                "{} is not inside a configured workspace crate",
                path.display()
            ))
        })
    }

    pub fn dependencies(&self, name: &str) -> Option<impl Iterator<Item = (&str, &Path)>> {
        let package = self.crates.get(name)?;
        Some(
            package
                .manifest
                .dependencies
                .iter()
                .filter_map(|dependency| {
                    self.crates
                        .get(dependency)
                        .map(|resolved| (dependency.as_str(), resolved.root.as_path()))
                }),
        )
    }

    pub fn undeclared_modules(&self, name: &str) -> Result<Vec<UndeclaredModule>, PackageError> {
        let package = self
            .crates
            .get(name)
            .ok_or_else(|| PackageError::new(format!("workspace has no crate named {name:?}")))?;
        let declared = package
            .modules
            .values()
            .map(|module| module.physical_path.clone())
            .collect::<BTreeSet<_>>();
        let mut found = Vec::new();
        collect_source_modules(
            &package.root.join("src"),
            &package.root.join("src"),
            "@src",
            true,
            &declared,
            name,
            &mut found,
        )?;
        found.sort_by(|left, right| left.selector.cmp(&right.selector));
        Ok(found)
    }
}

impl CrateManifest {
    pub fn declarations(&self, root: &Path) -> Result<Vec<ModuleDeclaration>, PackageError> {
        validate_crate_name(&self.name).map_err(|error| PackageError::new(error.to_string()))?;
        ensure_unique_sorted_set("module", &self.modules)?;
        ensure_unique_sorted_set("dependency", &self.dependencies)?;
        for dependency in &self.dependencies {
            validate_crate_name(dependency)
                .map_err(|error| PackageError::new(error.to_string()))?;
            if dependency == &self.name {
                return Err(PackageError::new(format!(
                    "crate {:?} cannot depend on itself",
                    self.name
                )));
            }
        }
        self.modules
            .iter()
            .map(|selector| parse_module_declaration(root, selector))
            .collect()
    }
}

fn validate_source_catalog(config: &WorkspaceConfig) -> Result<(), PackageError> {
    if config.members.is_empty() {
        return Err(PackageError::new(
            "workspace config must contain at least one member",
        ));
    }
    let mut member_paths = BTreeSet::new();
    for path in &config.members {
        validate_relative_path(path, "workspace member")?;
        if !member_paths.insert(path) {
            return Err(PackageError::new(format!(
                "duplicate workspace member path {}",
                path.display()
            )));
        }
    }
    for (name, source) in &config.sources {
        validate_crate_name(name).map_err(|error| PackageError::new(error.to_string()))?;
        if !(source.tarball.starts_with("http://") || source.tarball.starts_with("https://"))
            || !source.tarball.ends_with(".tar.gz")
        {
            return Err(PackageError::new(format!(
                "source {name:?} must be an http(s) .tar.gz URL"
            )));
        }
    }
    for (name, source) in &config.overrides {
        validate_crate_name(name).map_err(|error| PackageError::new(error.to_string()))?;
        if !config.sources.contains_key(name) {
            return Err(PackageError::new(format!(
                "override {name:?} has no remote source"
            )));
        }
        validate_relative_path(&source.path, "override")?;
    }
    Ok(())
}

fn validate_lock(lock: &WorkspaceLock, spec: &WorkspaceSpec) -> Result<(), PackageError> {
    let expected_names = spec
        .members
        .keys()
        .chain(spec.config.sources.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let locked_names = lock.packages.keys().cloned().collect::<BTreeSet<_>>();
    if locked_names != expected_names {
        return Err(PackageError::new(format!(
            "{LOCK_FILE} package names do not match {CONFIG_FILE}"
        )));
    }
    for (name, package) in &lock.packages {
        validate_crate_name(name).map_err(|error| PackageError::new(error.to_string()))?;
        ensure_sorted_set("locked module", &package.modules)?;
        ensure_sorted_set("locked dependency", &package.dependencies)?;
        for dependency in &package.dependencies {
            if !lock.packages.contains_key(dependency) {
                return Err(PackageError::new(format!(
                    "locked crate {name:?} requires missing crate {dependency:?}"
                )));
            }
        }
        match &package.source {
            LockedSource::Workspace { workspace } => {
                let member = spec.members.get(name).ok_or_else(|| {
                    PackageError::new(format!(
                        "locked workspace crate {name:?} is not a configured member"
                    ))
                })?;
                let expected = member
                    .root
                    .strip_prefix(&spec.root)
                    .expect("member roots are contained")
                    .to_owned();
                if workspace != &expected {
                    return Err(PackageError::new(format!(
                        "locked workspace path for {name:?} does not match config"
                    )));
                }
                let modules = member.modules.keys().cloned().collect::<Vec<_>>();
                if package.modules != modules
                    || package.dependencies != member.manifest.dependencies
                {
                    return Err(PackageError::new(format!(
                        "locked workspace crate {name:?} does not match {CRATE_FILE}"
                    )));
                }
            }
            LockedSource::Tarball { tarball } => {
                if spec.config.sources.get(name).map(|source| &source.tarball) != Some(tarball) {
                    return Err(PackageError::new(format!(
                        "locked tarball for {name:?} does not match config"
                    )));
                }
            }
        }
    }
    validate_locked_dependency_graph(&lock.packages)?;
    Ok(())
}

fn validate_locked_dependency_graph(
    packages: &BTreeMap<String, LockedPackage>,
) -> Result<(), PackageError> {
    fn visit(
        name: &str,
        packages: &BTreeMap<String, LockedPackage>,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
    ) -> Result<(), PackageError> {
        if visited.contains(name) {
            return Ok(());
        }
        if !visiting.insert(name.to_owned()) {
            return Err(PackageError::new(format!(
                "locked crate dependency cycle contains {name:?}"
            )));
        }
        for dependency in &packages[name].dependencies {
            visit(dependency, packages, visiting, visited)?;
        }
        visiting.remove(name);
        visited.insert(name.to_owned());
        Ok(())
    }
    let mut visited = BTreeSet::new();
    for name in packages.keys() {
        visit(name, packages, &mut BTreeSet::new(), &mut visited)?;
    }
    Ok(())
}

fn installed_crate_root(root: &Path) -> Result<PathBuf, PackageError> {
    let root = fs::canonicalize(root).map_err(|error| {
        PackageError::new(format!(
            "cannot resolve installed root {}: {error}",
            root.display()
        ))
    })?;
    if root.join(CRATE_FILE).is_file() {
        return Ok(root);
    }
    let mut candidates = fs::read_dir(&root)
        .map_err(|error| PackageError::new(format!("cannot inspect {}: {error}", root.display())))?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join(CRATE_FILE).is_file())
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    candidates.sort();
    if candidates.len() == 1 {
        return fs::canonicalize(&candidates[0]).map_err(|error| {
            PackageError::new(format!("cannot resolve installed crate root: {error}"))
        });
    }
    Err(PackageError::new(format!(
        "installed package {} must contain exactly one crate root",
        root.display()
    )))
}

fn validate_dependency_graph(crates: &BTreeMap<String, ResolvedCrate>) -> Result<(), PackageError> {
    for (name, package) in crates {
        for dependency in &package.manifest.dependencies {
            if !crates.contains_key(dependency) {
                return Err(PackageError::new(format!(
                    "crate {name:?} requires unavailable crate {dependency:?}"
                )));
            }
        }
    }

    fn visit(
        name: &str,
        crates: &BTreeMap<String, ResolvedCrate>,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
    ) -> Result<(), PackageError> {
        if visited.contains(name) {
            return Ok(());
        }
        if !visiting.insert(name.to_owned()) {
            return Err(PackageError::new(format!(
                "crate dependency cycle contains {name:?}"
            )));
        }
        for dependency in &crates[name].manifest.dependencies {
            visit(dependency, crates, visiting, visited)?;
        }
        visiting.remove(name);
        visited.insert(name.to_owned());
        Ok(())
    }

    let mut visited = BTreeSet::new();
    for name in crates.keys() {
        visit(name, crates, &mut BTreeSet::new(), &mut visited)?;
    }
    Ok(())
}

fn read_crate(root: &Path) -> Result<ResolvedCrate, PackageError> {
    let manifest_path = root.join(CRATE_FILE);
    let mut manifest: CrateManifest = read_json(&manifest_path)?;
    manifest.modules.sort();
    manifest.dependencies.sort();
    let declarations = manifest.declarations(root)?;
    let modules = declarations
        .into_iter()
        .map(|declaration| (declaration.selector.clone(), declaration))
        .collect();
    Ok(ResolvedCrate {
        root: root.to_owned(),
        manifest,
        modules,
    })
}

fn parse_module_declaration(
    root: &Path,
    selector: &str,
) -> Result<ModuleDeclaration, PackageError> {
    let (kind, rest, directory) = if let Some(rest) = selector.strip_prefix("@src/") {
        (ModuleDeclarationKind::Source, rest, root.join("src"))
    } else {
        return Err(PackageError::new(format!(
            "invalid module selector {selector:?}"
        )));
    };
    let logical_path = PathBuf::from(rest);
    validate_relative_path(&logical_path, "module selector")?;
    let extension = logical_path.extension().and_then(|value| value.to_str());
    let (format, physical_relative) = match extension {
        Some("json") => (ModuleFormat::Json, logical_path.clone()),
        Some("toml") => (ModuleFormat::Toml, logical_path.clone()),
        Some("yaml" | "yml") => (ModuleFormat::Yaml, logical_path.clone()),
        Some(_) => {
            return Err(PackageError::new(format!(
                "invalid module selector suffix in {selector:?}"
            )));
        }
        None => {
            let mut physical = logical_path.clone();
            physical.set_extension("telora");
            (ModuleFormat::Telora, physical)
        }
    };
    let physical_path = directory.join(physical_relative);
    let canonical = fs::canonicalize(&physical_path).map_err(|error| {
        PackageError::new(format!(
            "cannot resolve declared module {}: {error}",
            physical_path.display()
        ))
    })?;
    let canonical_root = fs::canonicalize(root).map_err(|error| {
        PackageError::new(format!(
            "cannot resolve crate root {}: {error}",
            root.display()
        ))
    })?;
    if !canonical.starts_with(&canonical_root) || !canonical.is_file() {
        return Err(PackageError::new(format!(
            "declared module {selector:?} escapes its crate root"
        )));
    }
    Ok(ModuleDeclaration {
        selector: selector.to_owned(),
        logical_path,
        physical_path: canonical,
        format,
        kind,
    })
}

#[allow(clippy::too_many_arguments)]
fn collect_source_modules(
    root: &Path,
    directory: &Path,
    prefix: &str,
    recursive: bool,
    declared: &BTreeSet<PathBuf>,
    crate_name: &str,
    found: &mut Vec<UndeclaredModule>,
) -> Result<(), PackageError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(PackageError::new(format!(
                "cannot scan module directory {}: {error}",
                directory.display()
            )));
        }
    };
    for entry in entries {
        let entry = entry.map_err(|error| {
            PackageError::new(format!("cannot scan {}: {error}", directory.display()))
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            PackageError::new(format!("cannot inspect {}: {error}", path.display()))
        })?;
        if file_type.is_dir() {
            if recursive {
                collect_source_modules(root, &path, prefix, true, declared, crate_name, found)?;
            }
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let Ok(format) = ModuleFormat::from_path(&path) else {
            continue;
        };
        if !recursive && format != ModuleFormat::Telora {
            continue;
        }
        let canonical = fs::canonicalize(&path).map_err(|error| {
            PackageError::new(format!(
                "cannot resolve module file {}: {error}",
                path.display()
            ))
        })?;
        if declared.contains(&canonical) {
            continue;
        }
        let relative = path.strip_prefix(root).expect("scanned path is below root");
        if !recursive && relative.components().count() != 1 {
            continue;
        }
        let mut logical = relative.to_owned();
        if format == ModuleFormat::Telora {
            logical.set_extension("");
        }
        let logical = logical.to_string_lossy().replace('\\', "/");
        found.push(UndeclaredModule {
            crate_name: crate_name.to_owned(),
            selector: format!("{prefix}/{logical}"),
            relative_path: path
                .strip_prefix(root.parent().unwrap_or(root))
                .unwrap_or(&path)
                .to_owned(),
        });
    }
    Ok(())
}

fn ensure_unique_sorted_set(label: &str, values: &[String]) -> Result<(), PackageError> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(PackageError::new(format!("duplicate {label} {value:?}")));
        }
    }
    Ok(())
}

fn ensure_sorted_set(label: &str, values: &[String]) -> Result<(), PackageError> {
    ensure_unique_sorted_set(label, values)?;
    if values.windows(2).any(|pair| pair[0] > pair[1]) {
        return Err(PackageError::new(format!("{label} entries must be sorted")));
    }
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, PackageError> {
    let source = fs::read(path)
        .map_err(|error| PackageError::new(format!("cannot read {}: {error}", path.display())))?;
    serde_json::from_slice(&source)
        .map_err(|error| PackageError::new(format!("invalid {}: {error}", path.display())))
}

fn contained_directory(root: &Path, relative: &Path, label: &str) -> Result<PathBuf, PackageError> {
    validate_relative_path(relative, label)?;
    let path = fs::canonicalize(root.join(relative)).map_err(|error| {
        PackageError::new(format!(
            "cannot resolve {label} {}: {error}",
            relative.display()
        ))
    })?;
    if !path.starts_with(root) || !path.is_dir() {
        return Err(PackageError::new(format!(
            "{label} {} must be a directory inside the workspace",
            relative.display()
        )));
    }
    Ok(path)
}

fn validate_relative_path(path: &Path, label: &str) -> Result<(), PackageError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(PackageError::new(format!(
            "{label} {} must be a contained relative path",
            path.display()
        )));
    }
    Ok(())
}

fn absolute(path: &Path) -> Result<PathBuf, PackageError> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|error| PackageError::new(format!("cannot read current directory: {error}")))
    }
}

fn package_digest(domain: &[u8], parts: &[&[u8]]) -> String {
    let mut hash = crate::sha256::Context::default();
    hash.update(b"telora.package.tarball\0\x01");
    hash.update(domain);
    for part in parts {
        hash.update(&(part.len() as u64).to_be_bytes());
        hash.update(part);
    }
    hash.finish()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
#[path = "package/tests.rs"]
mod tests;
