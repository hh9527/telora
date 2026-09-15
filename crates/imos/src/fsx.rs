//! Filesystem contracts used by the store, not a virtual filesystem.
//!
//! Identity keys are local to a store's filesystem. They are not permanent IDs:
//! after the last link disappears the OS may reuse an identity.
use std::fs::File;
use std::io;
use std::path::Path;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as platform;

#[cfg(not(unix))]
compile_error!(
    "imos filesystem backend currently requires Unix; other platforms need an fsx backend"
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct VolumeId(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileId(u128);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    volume: VolumeId,
    file: FileId,
}

impl FileIdentity {
    pub(crate) fn same_volume(self, other: Self) -> bool {
        self.volume == other.volume
    }

    /// Preserve the existing Unix on-disk request names. The store enforces
    /// same-volume registration before using this volume-local key.
    pub(crate) fn request_key(self) -> String {
        self.file.0.to_string()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ModificationStamp(i128);

#[derive(Clone, Debug)]
pub(crate) struct FileSnapshot {
    pub(crate) identity: FileIdentity,
    pub(crate) links: u64,
    length: u64,
    modified: ModificationStamp,
    pub(crate) is_file: bool,
    pub(crate) is_dir: bool,
}

impl FileSnapshot {
    /// Modification detection, not a proof of content equality.
    pub(crate) fn same_content_stamp(&self, other: &Self) -> bool {
        self.length == other.length && self.modified == other.modified
    }
}

#[derive(Clone, Copy)]
pub(crate) enum AccessPolicy {
    PrivateDirectory,
    Directory,
    RegularFile,
    ExecutableFile,
    /// Accidental-write protection; not an immutability/security guarantee.
    WriteProtected,
}

pub(crate) use platform::{
    protect_file, request_lock_key, set_access, snapshot, snapshot_file, sync_directory,
};

pub(crate) fn hard_link(source: &Path, target: &Path) -> io::Result<()> {
    // A copy would break the reference-counting protocol: never fall back.
    std::fs::hard_link(source, target)
}

pub(crate) fn replace_request(file: tempfile::NamedTempFile, target: &Path) -> io::Result<File> {
    file.persist(target).map_err(|error| error.error)
}

/// Publish a complete staged directory. Existing-object handling belongs to
/// the store, under its object lock; this does not promise durable publication.
pub(crate) fn publish_directory(staged: &Path, target: &Path) -> io::Result<()> {
    std::fs::rename(staged, target)
}

pub(crate) fn try_lock(file: &File, exclusive: bool) -> io::Result<bool> {
    let result = if exclusive {
        fs2::FileExt::try_lock_exclusive(file)
    } else {
        fs2::FileExt::try_lock_shared(file)
    };
    match result {
        Ok(()) => Ok(true),
        Err(error)
            if error.kind() == io::ErrorKind::WouldBlock
                || error.raw_os_error().is_some_and(|code| {
                    Some(code) == fs2::lock_contended_error().raw_os_error()
                }) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn unlock(file: &File) -> io::Result<()> {
    fs2::FileExt::unlock(file)
}

#[cfg(test)]
mod tests;
