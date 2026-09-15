use super::*;
use std::io::Write;

#[test]
fn links_preserve_identity_and_track_upstream_removal() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let registered = root.path().join("registered");
    std::fs::write(&source, b"plan").unwrap();
    let initial = snapshot(&source).unwrap();
    assert_eq!(initial.links, 1);
    hard_link(&source, &registered).unwrap();
    let linked = snapshot(&registered).unwrap();
    assert_eq!(linked.identity, initial.identity);
    assert_eq!(linked.links, 2);
    assert!(linked.same_content_stamp(&initial));
    std::fs::remove_file(&source).unwrap();
    assert_eq!(snapshot(&registered).unwrap().links, 1);
    std::fs::write(&registered, b"changed plan").unwrap();
    assert!(!snapshot(&registered).unwrap().same_content_stamp(&initial));
}

#[test]
fn request_replacement_preserves_new_identity_and_old_registration() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("request");
    let registered = root.path().join("registered");
    std::fs::write(&target, b"old").unwrap();
    set_access(&target, AccessPolicy::WriteProtected).unwrap();
    hard_link(&target, &registered).unwrap();
    let old = snapshot(&target).unwrap().identity;
    let mut staged = tempfile::NamedTempFile::new_in(root.path()).unwrap();
    staged.write_all(b"new").unwrap();
    staged.as_file().sync_all().unwrap();
    protect_file(staged.as_file()).unwrap();
    let new = snapshot_file(staged.as_file()).unwrap().identity;
    let published = replace_request(staged, &target).unwrap();
    published.sync_all().unwrap();
    sync_directory(root.path()).unwrap();
    assert_ne!(old, new);
    assert_eq!(snapshot(&target).unwrap().identity, new);
    assert_eq!(snapshot(&registered).unwrap().identity, old);
    assert_eq!(snapshot(&registered).unwrap().links, 1);
    assert_eq!(std::fs::read(&target).unwrap(), b"new");
}

#[test]
fn lock_contention_is_distinct_from_io_failure() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let first = File::options()
        .read(true)
        .write(true)
        .open(file.path())
        .unwrap();
    let second = File::options()
        .read(true)
        .write(true)
        .open(file.path())
        .unwrap();
    assert!(try_lock(&first, false).unwrap());
    assert!(try_lock(&second, false).unwrap());
    unlock(&second).unwrap();
    assert!(!try_lock(&second, true).unwrap());
    unlock(&first).unwrap();
    assert!(try_lock(&second, true).unwrap());
    assert!(!try_lock(&first, false).unwrap());
    unlock(&second).unwrap();
}

#[test]
fn publishing_exposes_the_complete_directory() {
    let root = tempfile::tempdir().unwrap();
    let staged = root.path().join("staged");
    let target = root.path().join("published");
    std::fs::create_dir(&staged).unwrap();
    std::fs::write(staged.join("data"), b"complete").unwrap();
    publish_directory(&staged, &target).unwrap();
    assert!(!staged.exists());
    assert_eq!(std::fs::read(target.join("data")).unwrap(), b"complete");
}
