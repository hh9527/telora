use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use telora_core::{ResolvedWorkspace, WorkspaceSpec};

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
        let path = home.join(plan_name);
        publish_plan(&path, &plan)?;
        roots.insert(name, create(&path)?);
    }
    remove_stale_plans(&home, &live)?;
    Ok(roots)
}

fn publish_plan(path: &Path, plan: &serde_json::Value) -> Result<(), String> {
    let mut bytes =
        serde_json::to_vec(plan).map_err(|error| format!("cannot encode IMOS plan: {error}"))?;
    bytes.push(b'\n');
    if fs::read(path).is_ok_and(|existing| existing == bytes) {
        return Ok(());
    }
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(&temporary, bytes)
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("cannot replace {}: {error}", path.display())
    })
}

fn create(plan: &Path) -> Result<PathBuf, String> {
    let executable = env::var_os("TELORA_IMOS").unwrap_or_else(|| "imos".into());
    let output = Command::new(&executable)
        .arg("create")
        .arg(plan)
        .output()
        .map_err(|error| format!("cannot start {:?}: {error}", executable))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "IMOS could not materialize {}: {}",
            plan.display(),
            stderr.trim()
        ));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| "IMOS returned a non-UTF-8 installation path".to_owned())?;
    let lines = stdout.lines().collect::<Vec<_>>();
    if lines.len() != 1 || lines[0].is_empty() {
        return Err("IMOS returned an invalid installation path".into());
    }
    Ok(PathBuf::from(lines[0]))
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
