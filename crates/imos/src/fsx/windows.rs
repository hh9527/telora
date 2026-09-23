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
// between that epoch and 1970-01-01, i.e. 11_644_473_600 * 10_000_000 ticks.
const FILETIME_EPOCH_OFFSET_TICKS: u64 = 116_444_736_000_000_000;

/// Converts FILETIME ticks to nanoseconds since the Unix epoch, the stamp
/// contract shared with the Unix backend. The subtraction is signed so
/// timestamps from before 1970 stay negative instead of clamping to zero.
fn filetime_ticks_to_unix_ns(ticks: u64) -> i128 {
    (i128::from(ticks) - i128::from(FILETIME_EPOCH_OFFSET_TICKS)) * 100
}

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
        modified: ModificationStamp(filetime_ticks_to_unix_ns(ticks)),
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

pub(crate) fn set_access(path: &Path, policy: AccessPolicy) -> io::Result<()> {
    match policy {
        // Plain directories, regular files and executables keep the ACLs
        // they inherit from their private store root.
        AccessPolicy::Directory | AccessPolicy::RegularFile | AccessPolicy::ExecutableFile => {
            Ok(())
        }
        // Deliberately not the read-only attribute: it would block
        // `replace_request` from replacing published request files and block
        // removal of stale objects during collection. Accidental-write
        // protection is an explicit platform tradeoff, guarded by store
        // locks and identity checks instead.
        AccessPolicy::WriteProtected => Ok(()),
        // `Store::open` accepts arbitrary roots and `TELORA_IMOS_STORE` can
        // relocate the store outside the user profile, so inherited ACLs
        // cannot be assumed to restrict access: write an explicit
        // owner-only DACL, the counterpart of the Unix 0700 policy.
        AccessPolicy::PrivateDirectory => set_owner_only_dacl(path),
    }
}

/// Grants full control to the directory's owner, SYSTEM and Administrators
/// through a protected, inheritable DACL, mirroring the Unix 0700 policy for
/// store roots. Children created inside the directory afterwards inherit
/// the same grants.
fn set_owner_only_dacl(path: &Path) -> io::Result<()> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW, SE_FILE_OBJECT, SetEntriesInAclW,
        SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        AllocateAndInitializeSid, DACL_SECURITY_INFORMATION, FreeSid, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSID, SECURITY_NT_AUTHORITY,
        SUB_CONTAINERS_AND_OBJECTS_INHERIT,
    };

    const FILE_ALL_ACCESS: u32 = 0x001F_01FF;

    // SAFETY: the returned SID must be freed with `FreeSid`.
    unsafe fn well_known_sid(subauthorities: &[u32]) -> io::Result<PSID> {
        let mut sid: PSID = std::ptr::null_mut();
        let mut tail = [0u32; 7];
        tail[..subauthorities.len() - 1].copy_from_slice(&subauthorities[1..]);
        // SAFETY: `SECURITY_NT_AUTHORITY` is a static authority value, the
        // sub-authority count matches `subauthorities`, and `sid` receives a
        // PSID the caller frees with `FreeSid`.
        if unsafe {
            AllocateAndInitializeSid(
                &SECURITY_NT_AUTHORITY,
                subauthorities.len() as u8,
                subauthorities[0],
                tail[0],
                tail[1],
                tail[2],
                tail[3],
                tail[4],
                tail[5],
                tail[6],
                &mut sid,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(sid)
    }

    let system = unsafe { well_known_sid(&[18]) }?;
    let administrators = match unsafe { well_known_sid(&[32, 544]) } {
        Ok(sid) => sid,
        Err(error) => {
            // SAFETY: `system` was allocated above and is freed exactly once.
            unsafe { FreeSid(system) };
            return Err(error);
        }
    };

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();

    let mut owner: PSID = std::ptr::null_mut();
    let mut descriptor = std::ptr::null_mut();
    // SAFETY: `wide` is NUL-terminated and names an existing directory the
    // caller controls; `owner`/`descriptor` belong to the returned security
    // descriptor and are freed below.
    let query = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };

    let mut dacl = std::ptr::null_mut();
    let result = if query == 0 {
        let grant = |sid: PSID| EXPLICIT_ACCESS_W {
            grfAccessPermissions: FILE_ALL_ACCESS,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: std::ptr::null_mut(),
                MultipleTrusteeOperation: 0,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: 0,
                ptstrName: sid.cast(),
            },
        };
        let entries = [grant(owner), grant(system), grant(administrators)];
        // SAFETY: `entries` is an array of correctly shaped EXPLICIT_ACCESS_W;
        // `dacl` receives an ACL owned by us and freed below.
        let build = unsafe {
            SetEntriesInAclW(
                entries.len() as u32,
                entries.as_ptr(),
                std::ptr::null(),
                &mut dacl,
            )
        };
        if build != 0 {
            build
        } else {
            // SAFETY: `wide` is NUL-terminated. The protected DACL replaces
            // inherited access on the directory itself, while the inheritable
            // entries cover children created afterwards.
            unsafe {
                SetNamedSecurityInfoW(
                    wide.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    dacl,
                    std::ptr::null(),
                )
            }
        }
    } else {
        query
    };

    // SAFETY: `descriptor`/`dacl` are either null or were produced by the
    // calls above, and each non-null one is freed exactly once.
    unsafe {
        if !descriptor.is_null() {
            LocalFree(descriptor.cast());
        }
        if !dacl.is_null() {
            LocalFree(dacl.cast());
        }
        FreeSid(system);
        FreeSid(administrators);
    }

    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    Ok(())
}

pub(crate) fn protect_file(_file: &File) -> io::Result<()> {
    // Same tradeoff as `WriteProtected`: the read-only attribute would block
    // request replacement (`tempfile` persist) and stale-object removal.
    // Accidental-write protection relies on store locks and identity checks.
    Ok(())
}

pub(crate) fn sync_directory(_path: &Path) -> io::Result<()> {
    // Directory fsync needs a handle opened with FILE_FLAG_BACKUP_SEMANTICS
    // and FlushFileBuffers; the store tolerates the crash-durability gap on
    // Windows, matching the fork backend this port supersedes.
    Ok(())
}

pub(crate) fn request_lock_key(target: &Path) -> String {
    // Input premise: callers resolve the request home with
    // `std::fs::canonicalize` before joining the target name, and the name is
    // restricted by `validate_plan_name` to lowercase ASCII letters, digits
    // and single separators. Under those premises the residual alias classes
    // are the verbatim (`\\?\` and `\\?\UNC\`) prefixes that canonicalize
    // keeps and differences in drive/directory casing: strip the prefixes and
    // fold case so those spellings share one lock key. This is not a general
    // Windows path normalizer; alias forms outside the premise (e.g. 8.3
    // short names or symlinks under the canonical parent) are expected to be
    // resolved by canonicalize before the key is computed, and any that are
    // not would surface as separate keys for the same target.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_epoch_filetime_converts_to_zero() {
        assert_eq!(filetime_ticks_to_unix_ns(FILETIME_EPOCH_OFFSET_TICKS), 0);
    }

    #[test]
    fn filetime_before_unix_epoch_stays_negative() {
        // 1601-01-01, the FILETIME zero point, is a whole number of seconds
        // before 1970-01-01 and must not clamp to zero.
        assert_eq!(
            filetime_ticks_to_unix_ns(0),
            -11_644_473_600i128 * 1_000_000_000
        );
    }

    #[test]
    fn filetime_one_second_after_unix_epoch() {
        assert_eq!(
            filetime_ticks_to_unix_ns(FILETIME_EPOCH_OFFSET_TICKS + 10_000_000),
            1_000_000_000
        );
    }

    #[test]
    fn private_directory_dacl_grants_owner_only() {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
        use windows_sys::Win32::Security::{
            ACCESS_ALLOWED_ACE, ACL_SIZE_INFORMATION, AllocateAndInitializeSid, EqualSid, FreeSid,
            GetAce, GetAclInformation, GetSecurityDescriptorControl, PSID, SE_DACL_PROTECTED,
            SECURITY_WORLD_SID_AUTHORITY,
        };
        use windows_sys::Win32::Security::{AclSizeInformation, DACL_SECURITY_INFORMATION};

        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        set_access(root, AccessPolicy::PrivateDirectory).unwrap();

        let wide = root
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let mut dacl = std::ptr::null_mut();
        let mut descriptor = std::ptr::null_mut();
        // SAFETY: `wide` is NUL-terminated and names the directory we just
        // protected; the returned descriptor is freed at the end of the test.
        unsafe {
            assert_eq!(
                GetNamedSecurityInfoW(
                    wide.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut dacl,
                    std::ptr::null_mut(),
                    &mut descriptor,
                ),
                0
            );
            let mut control = 0u16;
            let mut revision = 0u32;
            assert_ne!(
                GetSecurityDescriptorControl(descriptor, &mut control, &mut revision),
                0
            );
            assert_ne!(control & SE_DACL_PROTECTED, 0, "DACL must be protected");

            let mut size: ACL_SIZE_INFORMATION = std::mem::zeroed();
            assert_ne!(
                GetAclInformation(
                    dacl,
                    &mut size as *mut _ as *mut core::ffi::c_void,
                    std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                    AclSizeInformation,
                ),
                0
            );
            assert_eq!(size.AceCount, 3, "owner, SYSTEM and Administrators only");

            let mut everyone: PSID = std::ptr::null_mut();
            assert_ne!(
                AllocateAndInitializeSid(
                    &SECURITY_WORLD_SID_AUTHORITY,
                    1,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    &mut everyone,
                ),
                0
            );
            let mut ace = std::ptr::null_mut();
            for index in 0..size.AceCount {
                assert_ne!(GetAce(dacl, index, &mut ace), 0);
                let allowed = &*(ace as *const ACCESS_ALLOWED_ACE);
                assert_eq!(allowed.Header.AceType, 0, "every ACE grants access");
                let sid = &allowed.SidStart as *const u32 as *mut core::ffi::c_void;
                assert_eq!(EqualSid(sid, everyone), 0, "no ACE may grant Everyone");
            }
            FreeSid(everyone);
            LocalFree(descriptor.cast());
        }
    }

    #[test]
    fn lock_key_folds_verbatim_prefixes_and_casing() {
        let verbatim = request_lock_key(Path::new(r"\\?\C:\Users\Dev\Data\plan.json"));
        let plain = request_lock_key(Path::new(r"C:\Users\Dev\Data\plan.json"));
        assert_eq!(verbatim, plain);

        let folded = request_lock_key(Path::new(r"C:\users\dev\data\plan.json"));
        assert_eq!(plain, folded);
    }

    #[test]
    fn lock_key_normalizes_unc_verbatim_prefix() {
        let verbatim = request_lock_key(Path::new(r"\\?\UNC\server\share\plan.json"));
        let plain = request_lock_key(Path::new(r"\\server\share\plan.json"));
        assert_eq!(verbatim, plain);
    }

    #[test]
    fn lock_key_distinguishes_names_and_parents() {
        let home = Path::new(r"C:\store\home");
        assert_ne!(
            request_lock_key(&home.join("plan-a.json")),
            request_lock_key(&home.join("plan-b.json"))
        );
        assert_ne!(
            request_lock_key(&home.join("plan.json")),
            request_lock_key(Path::new(r"C:\store\other\plan.json"))
        );
    }
}
