use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use directories::ProjectDirs;
use imos::Store;
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
    let mut live = BTreeSet::new();
    for (_, plan) in &plans {
        let plan_name = plan
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "generated IMOS plan has no name".to_owned())?;
        live.insert(plan_name.to_owned());
    }
    let store_path = configured_store_path()?;
    let roots = install_plans(home.clone(), store_path, plans)?;
    remove_stale_plans(&home, &live)?;
    Ok(roots)
}

fn install_plans(
    home: PathBuf,
    store_path: PathBuf,
    plans: Vec<(String, serde_json::Value)>,
) -> Result<BTreeMap<String, PathBuf>, String> {
    let roots = std::thread::spawn(move || -> Result<_, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("cannot start package runtime: {error}"))?;
        runtime.block_on(async {
            let store = Store::open(store_path)
                .await
                .map_err(|error| format!("cannot open IMOS store: {error:#}"))?;
            let mut roots = BTreeMap::new();
            for (name, plan) in plans {
                let root = store
                    .install(&home, plan)
                    .await
                    .map_err(|error| format!("cannot install crate {name:?}: {error:#}"))?;
                roots.insert(name, root);
            }
            Ok(roots)
        })
    })
    .join()
    .map_err(|_| "package runtime panicked".to_owned())??;
    Ok(roots)
}

fn configured_store_path() -> Result<PathBuf, String> {
    match env::var_os("TELORA_IMOS_STORE") {
        Some(path) if !path.is_empty() => Ok(PathBuf::from(path)),
        Some(_) => Err("TELORA_IMOS_STORE must not be empty".to_owned()),
        None => ProjectDirs::from("dev", "imos", "imos")
            .map(|dirs| dirs.cache_dir().to_path_buf())
            .ok_or_else(|| "cannot determine IMOS store directory".to_owned()),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn installs_imos_plans_from_an_existing_runtime() {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("refs");
        fs::create_dir(&home).unwrap();
        let source = temporary.path().join("crate.telora");
        fs::write(&source, "pub def value: Int = 42;").unwrap();
        let url = url::Url::from_file_path(&source).unwrap();
        let plan = serde_json::json!({
            "version": 1,
            "name": "test-crate.json",
            "key": "test-crate-v1",
            "items": [{"name": "source", "key": "test-source-v1", "kind": {
                "type": "InstallFile", "url": url, "to": "lib.telora"
            }}]
        });
        let roots = install_plans(
            home,
            temporary.path().join("store"),
            vec![("test-crate".into(), plan)],
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(roots["test-crate"].join("lib.telora")).unwrap(),
            "pub def value: Int = 42;"
        );
    }
}
