use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use telora_core::{ResolvedWorkspace, WorkspaceSpec};
use telora_ees::{InstallSharedRequest, Request};

pub fn prepare(context: &Path) -> Result<Arc<ResolvedWorkspace>, String> {
    let spec = WorkspaceSpec::discover(context).map_err(|error| error.to_string())?;
    spec.validate_existing_lock()
        .map_err(|error| error.to_string())?;
    let roots = materialize(&spec)?;
    spec.resolve(&roots, true)
        .map(Arc::new)
        .map_err(|error| error.to_string())
}

pub fn lock(context: &Path) -> Result<PathBuf, String> {
    let spec = WorkspaceSpec::discover(context).map_err(|error| error.to_string())?;
    let roots = materialize(&spec)?;
    let lock = spec
        .generate_lock(&roots)
        .map_err(|error| error.to_string())?;
    spec.write_lock(&lock).map_err(|error| error.to_string())?;
    Ok(spec.lock_path())
}

fn materialize(spec: &WorkspaceSpec) -> Result<BTreeMap<String, PathBuf>, String> {
    let plans = spec
        .remote_sources()
        .map(|(name, _)| {
            spec.imos_plan(name)
                .map(|plan| (name.to_owned(), plan))
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let home = spec.root().join(".telora/crates-refs");
    if plans.is_empty() {
        if home.is_dir() {
            remove_stale_plans(&home, &BTreeSet::new())?;
        }
        return Ok(BTreeMap::new());
    }
    fs::create_dir_all(&home)
        .map_err(|error| format!("cannot create {}: {error}", home.display()))?;
    let mut roots = BTreeMap::new();
    let mut live = BTreeSet::new();
    for (name, plan) in plans {
        let plan_name = plan
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "generated IMOS plan has no name".to_owned())?;
        live.insert(plan_name.to_owned());
        roots.insert(name.clone(), install_shared(&name, &home, plan)?);
    }
    remove_stale_plans(&home, &live)?;
    Ok(roots)
}

fn install_shared(id: &str, home: &Path, plan: serde_json::Value) -> Result<PathBuf, String> {
    telora_ees::dispatch_blocking(Request::InstallShared(InstallSharedRequest {
        id: id.to_owned(),
        home: home.to_path_buf(),
        plan,
    }))
    .map_err(|error| format!("EES could not start InstallShared: {error:#}"))?
    .into_root()
    .map_err(|message| format!("EES InstallShared failed: {message}"))
}

fn remove_stale_plans(home: &Path, live: &BTreeSet<String>) -> Result<(), String> {
    for entry in
        fs::read_dir(home).map_err(|error| format!("cannot inspect {}: {error}", home.display()))?
    {
        let entry = entry.map_err(|error| format!("cannot inspect {}: {error}", home.display()))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?
            .is_file()
            && name.starts_with("telora-")
            && name.ends_with(".json")
            && !live.contains(name)
        {
            fs::remove_file(entry.path())
                .map_err(|error| format!("cannot remove stale IMOS plan {name:?}: {error}"))?;
        }
    }
    Ok(())
}
