//! Strict development file/directory ACL inspection, not full path trust.
use crate::support::Local;
use std::fs::{File, OpenOptions};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Component, Path, Prefix};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
use windows::Win32::Security::*;
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_SHARE_READ,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AclError {
    QueryFailed,
    InvalidDescriptor,
    UnexpectedOwner,
    UnexpectedAcl,
}

/// Retains a directory handle inspected against the development policy.
/// It does not validate ancestors or confer permission to execute files.
#[must_use = "retain this handle while using the inspected directory"]
pub struct DevelopmentDirectory {
    _directory: File,
}

/// Open the final directory itself without following a reparse point, and
/// hold it without write/delete sharing after checking its development ACL.
/// The caller still has to validate and retain all relevant ancestors.
pub fn open_development_directory(path: &Path) -> Result<DevelopmentDirectory, AclError> {
    if !matches!(path.components().next(), Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
        || !path.is_absolute()
        || path.as_os_str().encode_wide().any(|unit| unit == 0)
    {
        return Err(AclError::InvalidDescriptor);
    }
    let directory = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
        .map_err(|_| AclError::QueryFailed)?;
    check_object(&directory, true)?;
    Ok(DevelopmentDirectory {
        _directory: directory,
    })
}

/// Check the ACL of an already open file. This strict development policy
/// accepts exactly the explicit Admin/System full and Users read/execute
/// rules used by the staging script. The caller must open without following
/// reparse points and separately inspect every ancestor.
pub fn check_file(file: &File) -> Result<(), AclError> {
    check_object(file, false)
}

fn check_object(file: &File, directory: bool) -> Result<(), AclError> {
    let metadata = file.metadata().map_err(|_| AclError::QueryFailed)?;
    if (if directory {
        !metadata.is_dir()
    } else {
        !metadata.is_file()
    }) || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
    {
        return Err(AclError::InvalidDescriptor);
    }
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: the file handle remains live; GetSecurityInfo allocates the
    // returned descriptor, which Local frees after validation.
    let status = unsafe {
        GetSecurityInfo(
            HANDLE(file.as_raw_handle()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            None,
            None,
            None,
            None,
            Some(&mut descriptor),
        )
    };
    if status.0 != 0 || descriptor.0.is_null() {
        return Err(AclError::QueryFailed);
    }
    let _allocation = Local(descriptor.0);
    // SAFETY: descriptor was returned by Windows and remains live.
    unsafe { check_descriptor(descriptor, directory) }
}

unsafe fn check_descriptor(
    descriptor: PSECURITY_DESCRIPTOR,
    directory: bool,
) -> Result<(), AclError> {
    if !unsafe { IsValidSecurityDescriptor(descriptor) }.as_bool() {
        return Err(AclError::InvalidDescriptor);
    }
    let mut control = 0u16;
    let mut revision = 0;
    unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) }
        .map_err(|_| AclError::InvalidDescriptor)?;
    if control & SE_DACL_PROTECTED.0 == 0 {
        return Err(AclError::UnexpectedAcl);
    }
    let mut owner = PSID::default();
    let mut defaulted = windows::core::BOOL::default();
    unsafe { GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted) }
        .map_err(|_| AclError::InvalidDescriptor)?;
    if owner.0.is_null()
        || !unsafe { IsValidSid(owner) }.as_bool()
        || !unsafe { IsWellKnownSid(owner, WinBuiltinAdministratorsSid) }.as_bool()
    {
        return Err(AclError::UnexpectedOwner);
    }
    let mut present = windows::core::BOOL::default();
    let mut acl = std::ptr::null_mut();
    unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) }
        .map_err(|_| AclError::InvalidDescriptor)?;
    if !present.as_bool() || acl.is_null() || !unsafe { IsValidAcl(acl) }.as_bool() {
        return Err(AclError::UnexpectedAcl);
    }
    let mut info = ACL_SIZE_INFORMATION::default();
    unsafe {
        GetAclInformation(
            acl,
            (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    }
    .map_err(|_| AclError::InvalidDescriptor)?;
    if info.AceCount != 3 {
        return Err(AclError::UnexpectedAcl);
    }
    let acl_start = acl as usize;
    let acl_end = acl_start
        .checked_add(info.AclBytesInUse as usize)
        .ok_or(AclError::InvalidDescriptor)?;
    let mut seen = 0u8;
    for index in 0..info.AceCount {
        let mut entry = std::ptr::null_mut();
        unsafe { GetAce(acl, index, &mut entry) }.map_err(|_| AclError::InvalidDescriptor)?;
        let entry_start = entry as usize;
        if entry.is_null()
            || entry_start < acl_start
            || entry_start
                .checked_add(std::mem::size_of::<ACE_HEADER>())
                .is_none_or(|end| end > acl_end)
        {
            return Err(AclError::InvalidDescriptor);
        }
        let header = unsafe { &*entry.cast::<ACE_HEADER>() };
        if header.AceType != 0
            || header.AceFlags != if directory { 3 } else { 0 }
            || usize::from(header.AceSize) < 16
            || entry_start
                .checked_add(usize::from(header.AceSize))
                .is_none_or(|end| end > acl_end)
        {
            return Err(AclError::UnexpectedAcl);
        }
        let ace = unsafe { &*entry.cast::<ACCESS_ALLOWED_ACE>() };
        let sid_bytes = std::ptr::addr_of!(ace.SidStart).cast::<u8>();
        // SID revision and subauthority count fit in the validated ACE size.
        let count = unsafe { *sid_bytes.add(1) } as usize;
        if count > 15 || 8 + 8 + count * 4 != usize::from(header.AceSize) {
            return Err(AclError::UnexpectedAcl);
        }
        let sid = PSID(sid_bytes.cast_mut().cast());
        if !unsafe { IsValidSid(sid) }.as_bool() {
            return Err(AclError::InvalidDescriptor);
        }
        let (bit, mask) = if unsafe { IsWellKnownSid(sid, WinBuiltinAdministratorsSid) }.as_bool() {
            (1, 0x001f01ff)
        } else if unsafe { IsWellKnownSid(sid, WinLocalSystemSid) }.as_bool() {
            (2, 0x001f01ff)
        } else if unsafe { IsWellKnownSid(sid, WinBuiltinUsersSid) }.as_bool() {
            (4, 0x001200a9)
        } else {
            return Err(AclError::UnexpectedAcl);
        };
        if ace.Mask != mask || seen & bit != 0 {
            return Err(AclError::UnexpectedAcl);
        }
        seen |= bit;
    }
    if seen != 7 {
        return Err(AclError::UnexpectedAcl);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::PCWSTR;
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };

    fn check_sddl(sddl: &str, directory: bool) -> Result<(), AclError> {
        let wide: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(wide.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        }
        .unwrap();
        let _allocation = Local(descriptor.0);
        unsafe { check_descriptor(descriptor, directory) }
    }

    #[test]
    fn strict_development_file_acl_rejects_unexpected_authority() {
        let good = "O:BAD:P(A;;FA;;;BA)(A;;FA;;;SY)(A;;0x001200a9;;;BU)";
        assert_eq!(check_sddl(good, false), Ok(()));
        for bad in [
            "O:SYD:P(A;;FA;;;BA)(A;;FA;;;SY)(A;;0x001200a9;;;BU)",
            "O:BAD:(A;;FA;;;BA)(A;;FA;;;SY)(A;;0x001200a9;;;BU)",
            "O:BAD:P(A;ID;FA;;;BA)(A;;FA;;;SY)(A;;0x001200a9;;;BU)",
            "O:BAD:P(D;;FA;;;BA)(A;;FA;;;SY)(A;;0x001200a9;;;BU)",
            "O:BAD:P(A;;FA;;;BA)(A;;FA;;;SY)(A;;FA;;;BU)",
            "O:BAD:P(A;;FA;;;BA)(A;;FA;;;SY)(A;;0x001200a9;;;WD)",
            "O:BAD:P(A;;FA;;;BA)(A;;FA;;;SY)(A;;0x001200a9;;;BU)(A;;GR;;;WD)",
            "O:BAD:P(A;;FA;;;BA)(A;;FA;;;BA)(A;;0x001200a9;;;BU)",
        ] {
            assert!(
                check_sddl(bad, false).is_err(),
                "untrusted descriptor accepted"
            );
        }
    }

    #[test]
    fn strict_development_directory_requires_exact_inheritance_and_permissions() {
        let good = "O:BAD:P(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;0x001200a9;;;BU)";
        assert_eq!(check_sddl(good, true), Ok(()));
        assert!(check_sddl(good, false).is_err());
        for bad in [
            "O:BAD:P(A;CI;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;0x001200a9;;;BU)",
            "O:BAD:P(A;OICIIO;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;0x001200a9;;;BU)",
            "O:BAD:P(A;OICIID;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;0x001200a9;;;BU)",
            "O:BAD:P(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;FA;;;BU)",
            "O:BAD:P(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)(A;OICI;0x001200a9;;;BU)(A;;GR;;;WD)",
        ] {
            assert!(
                check_sddl(bad, true).is_err(),
                "untrusted directory ACL accepted"
            );
        }
        assert!(open_development_directory(Path::new("relative")).is_err());
        assert!(open_development_directory(Path::new(r"\\server\share")).is_err());
    }

    #[test]
    fn ordinary_workspace_executable_does_not_satisfy_protected_file_policy() {
        let file = File::open(std::env::current_exe().unwrap()).unwrap();
        assert!(check_file(&file).is_err());
    }
    #[test]
    fn configured_protected_development_file() {
        let Some(path) = std::env::var_os("SUMIBI_TEST_PROTECTED_FILE") else {
            eprintln!("protected file ACL test skipped: no fixture configured");
            return;
        };
        let file = File::open(path).unwrap();
        assert_eq!(check_file(&file), Ok(()));
        println!("configured protected file ACL verified");
    }
    #[test]
    fn configured_protected_development_directory() {
        let Some(path) = std::env::var_os("SUMIBI_TEST_PROTECTED_DIRECTORY") else {
            eprintln!("protected directory ACL test skipped: no fixture configured");
            return;
        };
        let directory =
            open_development_directory(Path::new(&path)).expect("protected directory ACL");
        drop(directory);
        println!("configured protected directory ACL verified");
    }
}
