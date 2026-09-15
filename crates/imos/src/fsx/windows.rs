//! Windows filesystem backend for the store contracts.
//!
//! Identity comes from `GetFileInformationByHandle`: the volume serial
//! number mirrors `st_dev`, the file index mirrors `st_ino`, and
//! `nNumberOfLinks` mirrors `st_nlink`. The stable standard library does not
//! expose by-handle metadata (`windows_by_handle` is unstable), so this
//! talks to Win32 directly. `FILE_FLAG_BACKUP_SEMANTICS` allows opening
//! directories, which the store identity check needs.
use super::*;
use sha2::{Digest, Sha256};
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;

const FILE_READ_ATTRIBUTES: u32 = 0x0080;
const FILE_SHARE_READ: u32 = 0x1;
const FILE_SHARE_WRITE: u32 = 0x2;
const FILE_SHARE_DELETE: u32 = 0x4;
const OPEN_EXISTING: u32 = 3;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
const INVALID_HANDLE_VALUE: isize = -1;

// FILETIME is 100ns ticks since 1601-01-01; 11_644_473_600 seconds lie
// between that epoch and 1970-01-01.
const FILETIME_EPOCH_OFFSET_TICKS: u64 = 11_644_473_600_000_000;

// Mirrors BY_HANDLE_FILE_INFORMATION: every field is a DWORD (or a FILETIME
// pair of DWORDs), so no padding can appear.
#[repr(C)]
#[derive(Default)]
struct ByHandleFileInformation {
    attributes: u32,
    creation_time: [u32; 2],
    access_time: [u32; 2],
    write_time: [u32; 2],
    volume_serial: u32,
    size_high: u32,
    size_low: u32,
    links: u32,
    index_high: u32,
    index_low: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileW(
        filename: *const u16,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *const c_void,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: isize,
    ) -> isize;
    fn GetFileInformationByHandle(file: isize, info: *mut ByHandleFileInformation) -> i32;
    fn CloseHandle(handle: isize) -> i32;
}

fn from_handle_information(info: &ByHandleFileInformation) -> io::Result<FileSnapshot> {
    // FAT/exFAT report a zero file index; on NTFS the index is never zero.
    if info.index_high == 0 && info.index_low == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "file identity is unavailable; the imos store requires a local NTFS filesystem",
        ));
    }
    let ticks = ((info.write_time[1] as u64) << 32) | info.write_time[0] as u64;
    Ok(FileSnapshot {
        identity: FileIdentity {
            volume: VolumeId(info.volume_serial as u64),
            file: FileId(((info.index_high as u128) << 32) | info.index_low as u128),
        },
        links: info.links as u64,
        length: ((info.size_high as u64) << 32) | info.size_low as u64,
        modified: ModificationStamp(
            i128::from(ticks.saturating_sub(FILETIME_EPOCH_OFFSET_TICKS)).saturating_mul(100),
        ),
        is_file: info.attributes & FILE_ATTRIBUTE_DIRECTORY == 0,
        is_dir: info.attributes & FILE_ATTRIBUTE_DIRECTORY != 0,
    })
}

fn from_handle(handle: isize) -> io::Result<FileSnapshot> {
    let mut info = ByHandleFileInformation::default();
    // SAFETY: `handle` comes from `CreateFileW` or a live `std::fs::File`,
    // and `info` is a correctly shaped BY_HANDLE_FILE_INFORMATION.
    if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    from_handle_information(&info)
}

pub(crate) fn snapshot(path: &Path) -> io::Result<FileSnapshot> {
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: `wide` is NUL-terminated, and the handle is closed on every
    // return path.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let result = from_handle(handle);
    // SAFETY: `handle` is a live `CreateFileW` handle, closed exactly once.
    unsafe { CloseHandle(handle) };
    result
}

pub(crate) fn snapshot_file(file: &File) -> io::Result<FileSnapshot> {
    // SAFETY: `file` owns the handle for the duration of this call.
    from_handle(file.as_raw_handle() as isize)
}

pub(crate) fn set_access(_path: &Path, _policy: AccessPolicy) -> io::Result<()> {
    // The store lives under the user's profile, whose default ACLs already
    // restrict access to the owner. The read-only attribute is deliberately
    // not used for `WriteProtected`: it would block `replace_request` from
    // replacing published request files and block removal of stale objects
    // during collection. Immutability is enforced by store locks and
    // identity checks instead.
    Ok(())
}

pub(crate) fn protect_file(_file: &File) -> io::Result<()> {
    Ok(())
}

pub(crate) fn sync_directory(_path: &Path) -> io::Result<()> {
    // Directory fsync needs a handle opened with FILE_FLAG_BACKUP_SEMANTICS
    // and FlushFileBuffers; the store tolerates the crash-durability gap on
    // Windows, matching the fork backend this port supersedes.
    Ok(())
}

pub(crate) fn request_lock_key(target: &Path) -> String {
    // Callers pass `std::fs::canonicalize` output, which keeps the Windows
    // verbatim (`\\?\`) prefix, and Win32 path lookup is case-insensitive:
    // drop the prefix and fold case so aliases of one target share a lock
    // key. Over-merging cannot happen for distinct files, because names that
    // differ only by case denote the same file under default NTFS semantics.
    let text = target.as_os_str().to_string_lossy();
    let normalized = if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else if let Some(local) = text.strip_prefix(r"\\?\") {
        local.to_string()
    } else {
        text.into_owned()
    };
    hex::encode(Sha256::digest(normalized.to_lowercase().as_bytes()))
}
