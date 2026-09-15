use super::*;
use sha2::{Digest, Sha256};
use std::os::unix::fs::{MetadataExt, PermissionsExt};

fn from_metadata(metadata: std::fs::Metadata) -> FileSnapshot {
    FileSnapshot {
        identity: FileIdentity {
            volume: VolumeId(metadata.dev()),
            file: FileId(metadata.ino().into()),
        },
        links: metadata.nlink(),
        length: metadata.len(),
        modified: ModificationStamp(
            i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec()),
        ),
        is_file: metadata.is_file(),
        is_dir: metadata.is_dir(),
    }
}

pub(crate) fn snapshot(path: &Path) -> io::Result<FileSnapshot> {
    std::fs::metadata(path).map(from_metadata)
}

pub(crate) fn snapshot_file(file: &File) -> io::Result<FileSnapshot> {
    file.metadata().map(from_metadata)
}

fn permissions(policy: AccessPolicy) -> std::fs::Permissions {
    std::fs::Permissions::from_mode(match policy {
        AccessPolicy::PrivateDirectory => 0o700,
        AccessPolicy::Directory | AccessPolicy::ExecutableFile => 0o755,
        AccessPolicy::RegularFile => 0o644,
        AccessPolicy::WriteProtected => 0o444,
    })
}

pub(crate) fn set_access(path: &Path, policy: AccessPolicy) -> io::Result<()> {
    std::fs::set_permissions(path, permissions(policy))
}

pub(crate) fn protect_file(file: &File) -> io::Result<()> {
    file.set_permissions(permissions(AccessPolicy::WriteProtected))
}

/// Unlike file-content sync, this synchronizes the directory entry changes.
pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

/// Input is the canonical parent plus target name. Preserve existing Unix
/// lock names. A Windows backend must account for case/alias equivalence.
pub(crate) fn request_lock_key(target: &Path) -> String {
    hex::encode(Sha256::digest(target.as_os_str().as_encoded_bytes()))
}
