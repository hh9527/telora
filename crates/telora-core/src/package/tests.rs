use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use super::*;

fn fixture() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "telora-package-test-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(root.join("app/src/bin")).unwrap();
    fs::create_dir_all(root.join("model/src")).unwrap();
    fs::write(root.join("app/src/model.telora"), "export let value = 1;").unwrap();
    fs::write(root.join("app/src/bin/main.telora"), "0").unwrap();
    fs::write(root.join("model/src/lib.telora"), "export let value = 1;").unwrap();
    fs::write(
        root.join(CONFIG_FILE),
        r#"{"version":1,"members":["app","model"]}"#,
    )
    .unwrap();
    fs::write(
        root.join("app").join(CRATE_FILE),
        r#"{"name":"app","modules":["@src/bin/main","@src/model"],"dependencies":["model"]}"#,
    )
    .unwrap();
    fs::write(
        root.join("model").join(CRATE_FILE),
        r#"{"name":"model","modules":["@src/lib"],"dependencies":[]}"#,
    )
    .unwrap();
    fs::write(
        root.join(LOCK_FILE),
        r#"{"version":1,"packages":{"app":{"source":{"workspace":"app"},"modules":["@src/bin/main","@src/model"],"dependencies":["model"]},"model":{"source":{"workspace":"model"},"modules":["@src/lib"],"dependencies":[]}}}"#,
    )
    .unwrap();
    root
}

#[test]
fn discovers_workspace_and_authoritative_modules() {
    let root = fixture();
    let spec = WorkspaceSpec::discover(&root.join("app/src")).unwrap();
    let workspace = spec.resolve_workspace_only().unwrap();
    assert_eq!(
        workspace.crate_for_path(&root.join("app/src")).unwrap(),
        "app"
    );
    let module = workspace.module("app", "@src/model").unwrap();
    assert_eq!(module.logical_path, Path::new("model"));
    assert_eq!(module.format, ModuleFormat::Telora);
    assert!(workspace.module("app", "@src/missing").is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_manifest_reports_the_three_workspace_documents() {
    let root = std::env::temp_dir().join(format!(
        "telora-legacy-package-test-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("telora-deps.json"), "{}").unwrap();
    let error = WorkspaceSpec::discover(&root.join("src"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("legacy manifest"), "{error}");
    assert!(error.contains(CONFIG_FILE), "{error}");
    assert!(error.contains(CRATE_FILE), "{error}");
    assert!(error.contains(LOCK_FILE), "{error}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_undeclared_or_missing_modules() {
    let root = fixture();
    fs::write(
        root.join("app").join(CRATE_FILE),
        r#"{"name":"app","modules":["@src/missing"],"dependencies":["model"]}"#,
    )
    .unwrap();
    let error = WorkspaceSpec::discover(&root).unwrap_err().to_string();
    assert!(error.contains("declared module"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_source_and_member_name_collisions() {
    let root = fixture();
    fs::write(
        root.join(CONFIG_FILE),
        r#"{"version":1,"members":["app","model"],"sources":{"model":{"tarball":"https://example.test/model.tar.gz"}}}"#,
    )
    .unwrap();
    let error = WorkspaceSpec::discover(&root).unwrap_err().to_string();
    assert!(error.contains("both a workspace member and a remote source"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn reports_files_absent_from_the_module_catalog() {
    let root = fixture();
    fs::write(root.join("app/src/extra.telora"), "export let extra = 1;").unwrap();
    let workspace = WorkspaceSpec::discover(&root)
        .unwrap()
        .resolve_workspace_only()
        .unwrap();
    let undeclared = workspace.undeclared_modules("app").unwrap();
    assert_eq!(undeclared.len(), 1);
    assert_eq!(undeclared[0].selector, "@src/extra");
    fs::write(root.join("app/src/bin/extra.telora"), "0").unwrap();
    assert_eq!(workspace.undeclared_modules("app").unwrap().len(), 2);
    fs::create_dir_all(root.join("app/tests")).unwrap();
    fs::write(root.join("app/tests/extra.telora"), "0").unwrap();
    assert_eq!(workspace.undeclared_modules("app").unwrap().len(), 2);
    fs::create_dir_all(root.join("app/src/entry")).unwrap();
    fs::write(root.join("app/src/entry/extra.telora"), "0").unwrap();
    assert_eq!(workspace.undeclared_modules("app").unwrap().len(), 3);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn generates_and_atomically_writes_the_complete_workspace_lock() {
    let root = fixture();
    fs::remove_file(root.join(LOCK_FILE)).unwrap();
    let spec = WorkspaceSpec::discover(&root).unwrap();
    let lock = spec.generate_lock(&BTreeMap::new()).unwrap();
    assert_eq!(lock.packages.keys().collect::<Vec<_>>(), ["app", "model"]);
    spec.write_lock(&lock).unwrap();
    assert_eq!(spec.validate_existing_lock().unwrap(), lock);
    assert!(
        fs::read_to_string(root.join(LOCK_FILE))
            .unwrap()
            .ends_with('\n')
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_stale_extra_and_noncanonical_lock_state() {
    let root = fixture();
    let mut lock: WorkspaceLock = read_json(&root.join(LOCK_FILE)).unwrap();
    lock.packages.get_mut("app").unwrap().dependencies.clear();
    fs::write(root.join(LOCK_FILE), serde_json::to_vec(&lock).unwrap()).unwrap();
    let spec = WorkspaceSpec::discover(&root).unwrap();
    let error = spec.validate_existing_lock().unwrap_err().to_string();
    assert!(error.contains("does not match"), "{error}");
    assert!(error.contains("telora lock"), "{error}");

    let mut lock = spec.generate_lock(&BTreeMap::new()).unwrap();
    lock.packages.get_mut("app").unwrap().modules = vec!["@src/z".into(), "@src/a".into()];
    fs::write(root.join(LOCK_FILE), serde_json::to_vec(&lock).unwrap()).unwrap();
    let error = spec.validate_existing_lock().unwrap_err().to_string();
    assert!(error.contains("must be sorted"), "{error}");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn accepts_one_common_directory_in_an_imos_install_root() {
    let root = fixture();
    let install = root.join("install");
    fs::create_dir_all(install.join("package/src")).unwrap();
    fs::write(
        install.join("package/telora-crate.json"),
        r#"{"name":"remote","modules":[],"dependencies":[]}"#,
    )
    .unwrap();
    assert_eq!(
        installed_crate_root(&install).unwrap(),
        fs::canonicalize(install.join("package")).unwrap()
    );
    fs::create_dir_all(install.join("other")).unwrap();
    fs::write(
        install.join("other/telora-crate.json"),
        r#"{"name":"other","modules":[],"dependencies":[]}"#,
    )
    .unwrap();
    assert!(installed_crate_root(&install).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn imos_plan_is_deterministic_and_domain_scoped() {
    let root = fixture();
    fs::write(
        root.join(CONFIG_FILE),
        r#"{"version":1,"members":["app","model"],"sources":{"remote":{"tarball":"https://example.test/remote.tar.gz"}}}"#,
    )
    .unwrap();
    let spec = WorkspaceSpec::discover(&root).unwrap();
    let first = spec.imos_plan("remote").unwrap();
    let second = spec.imos_plan("remote").unwrap();
    assert_eq!(first, second);
    assert_eq!(first["version"], 1);
    assert_eq!(first["items"][0]["kind"]["archive"], "TarGzip");
    assert_eq!(first["items"][0]["kind"]["strip"], 0);
    assert!(first["key"].as_str().unwrap().len() <= 64);
    assert!(first["name"].as_str().unwrap().len() <= 64);
    fs::remove_dir_all(root).unwrap();
}
