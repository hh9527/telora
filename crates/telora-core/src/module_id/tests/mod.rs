use super::*;

mod cases;
mod test_catalog;

fn write_test_workspace(root: &Path, members: &[(&str, &str, &[&str])]) {
    fn collect(root: &Path, directory: &Path, modules: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        let mut entries = entries.collect::<Result<Vec<_>, _>>().unwrap();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let file_type = entry.file_type().unwrap();
            if file_type.is_dir() {
                collect(root, &path, modules);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let Ok(format) = ModuleFormat::from_path(&path) else {
                continue;
            };
            if canonical_path_for_physical(path.strip_prefix(root).unwrap()).is_err() {
                continue;
            }
            let mut logical = path.strip_prefix(root).unwrap().to_owned();
            if format == ModuleFormat::Telora {
                logical.set_extension("");
            }
            modules.push(format!(
                "@src/{}",
                logical.to_string_lossy().replace('\\', "/")
            ));
        }
    }

    let mut member_paths = Vec::new();
    for (relative, name, dependencies) in members {
        let crate_root = root.join(relative);
        let mut modules = Vec::new();
        collect(&crate_root.join("src"), &crate_root.join("src"), &mut modules);
        modules.sort();
        let mut dependencies = dependencies.to_vec();
        dependencies.sort();
        std::fs::write(
            crate_root.join(crate::package::CRATE_FILE),
            serde_json::to_vec(&serde_json::json!({
                "name": name,
                "modules": modules,
                "dependencies": dependencies,
            }))
            .unwrap(),
        )
        .unwrap();
        member_paths.push(*relative);
    }
    std::fs::write(
        root.join(crate::package::CONFIG_FILE),
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "members": member_paths,
        }))
        .unwrap(),
    )
    .unwrap();
    let spec = crate::package::WorkspaceSpec::discover(root).unwrap();
    let lock = spec
        .generate_lock(&std::collections::BTreeMap::new())
        .unwrap();
    spec.write_lock(&lock).unwrap();
}
