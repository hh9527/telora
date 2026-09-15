use imos::Store;

#[tokio::test]
async fn install_replace_and_collect_requests_and_artifacts() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let source = temporary.path().join("source");
    std::fs::write(&source, b"artifact").unwrap();
    let store = Store::open(temporary.path().join("store")).await.unwrap();
    let plan = |key: &str| {
        serde_json::json!({
            "version": 1, "name": "request", "key": key,
            "items": [{"name": "file", "key": "download-v1", "kind": {
                "type": "InstallFile", "url": url::Url::from_file_path(&source).unwrap().as_str(),
                "to": "file.txt"
            }}]
        })
    };
    let first = store.install(&home, plan("plan-v1")).await.unwrap();
    assert_eq!(std::fs::read(first.join("file.txt")).unwrap(), b"artifact");
    let second = store.install(&home, plan("plan-v2")).await.unwrap();
    assert_ne!(first, second);
    let gc = store.gc().await.unwrap();
    assert_eq!(gc.requests, 1);
    assert_eq!(gc.installs, 1);
    assert_eq!(gc.downloads, 0);
    assert!(!first.exists());
    assert_eq!(std::fs::read(second.join("file.txt")).unwrap(), b"artifact");
    std::fs::remove_file(home.join("request")).unwrap();
    let gc = store.gc().await.unwrap();
    assert_eq!(gc.requests, 1);
    assert_eq!(gc.installs, 1);
    assert_eq!(gc.downloads, 1);
    assert!(!second.exists());
}
