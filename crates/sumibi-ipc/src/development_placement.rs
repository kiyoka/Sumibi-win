//! Read-only development placement inspection. Not a product launch policy.
use crate::authenticode::{
    retain_expected_signer, InspectionError, SignerFingerprint, VerifiedFile,
};
use crate::protected_acl::{
    check_file, open_development_ancestor, open_development_directory, AclError, AncestorKind,
    DevelopmentAncestor, DevelopmentDirectory,
};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use windows::Win32::Security::Cryptography::*;
use windows::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Artifact {
    Tip,
    Host,
}
impl Artifact {
    fn filename(self) -> &'static str {
        match self {
            Self::Tip => "sumibi_tip.dll",
            Self::Host => "sumibi-host.exe",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacementError {
    InvalidVersion,
    Acl(AclError),
    File(InspectionError),
    FileMismatch,
    UnsupportedArtifact,
    InvalidReceipt,
    HashFailed,
}

/// Keeps each checked component alive. It does not establish product trust,
/// the integrity of dependencies, or the identity of a running process.
#[must_use = "retain the checked placement while using it"]
pub struct DevelopmentPlacement {
    _root: DevelopmentAncestor,
    _parent: DevelopmentAncestor,
    _install: DevelopmentDirectory,
    _version: DevelopmentDirectory,
    file: File,
    path: PathBuf,
    version: String,
    artifact: Artifact,
}

impl DevelopmentPlacement {
    /// Inspect one fixed-layout development artifact. A checked child blocks
    /// inherited ACEs, as required when the parent allows inherit-only rules.
    /// Caller must select the drive and version from a trusted configuration.
    pub fn open(root: &Path, version: &str, artifact: Artifact) -> Result<Self, PlacementError> {
        if !valid_version(version) {
            return Err(PlacementError::InvalidVersion);
        }
        let root_guard = open_development_ancestor(root, AncestorKind::DriveRoot)
            .map_err(PlacementError::Acl)?;
        let parent_path = root.join("Program Files");
        let parent_guard = open_development_ancestor(&parent_path, AncestorKind::InstallParent)
            .map_err(PlacementError::Acl)?;
        let install_path = parent_path.join("Sumibi");
        let install_guard =
            open_development_directory(&install_path).map_err(PlacementError::Acl)?;
        let version_path = install_path.join(version);
        let version_guard =
            open_development_directory(&version_path).map_err(PlacementError::Acl)?;
        let path = version_path.join(artifact.filename());
        let file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(&path)
            .map_err(|_| PlacementError::File(InspectionError::OpenFailed))?;
        check_file(&file).map_err(PlacementError::Acl)?;
        Ok(Self {
            _root: root_guard,
            _parent: parent_guard,
            _install: install_guard,
            _version: version_guard,
            file,
            path,
            version: version.to_owned(),
            artifact,
        })
    }

    /// Validate and retain the existing development staging receipt and the
    /// SHA-256 of the held DLL. This is consistency checking, not signature
    /// trust, signer identity validation, or authorization to register it.
    pub fn inspect_receipt(self) -> Result<ReceiptDevelopmentPlacement, PlacementError> {
        if self.artifact != Artifact::Tip {
            return Err(PlacementError::UnsupportedArtifact);
        }
        let path = self
            .path
            .parent()
            .ok_or(PlacementError::InvalidReceipt)?
            .join("staging.json");
        let mut receipt = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ.0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(path)
            .map_err(|_| PlacementError::InvalidReceipt)?;
        check_file(&receipt).map_err(PlacementError::Acl)?;
        let size = receipt
            .metadata()
            .map_err(|_| PlacementError::InvalidReceipt)?
            .len();
        if size == 0 || size > 16384 {
            return Err(PlacementError::InvalidReceipt);
        }
        let mut bytes = Vec::new();
        (&mut receipt)
            .take(16385)
            .read_to_end(&mut bytes)
            .map_err(|_| PlacementError::InvalidReceipt)?;
        if bytes.len() as u64 != size {
            return Err(PlacementError::InvalidReceipt);
        }
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
        let record: Receipt =
            serde_json::from_slice(bytes).map_err(|_| PlacementError::InvalidReceipt)?;
        let digest = hash_file(&self.file)?;
        validate_receipt(&record, &self.version, &self.path, &digest)?;
        Ok(ReceiptDevelopmentPlacement {
            _placement: self,
            _receipt: receipt,
        })
    }

    /// Additionally check the configured signer and retain both handles.
    /// The caller must supply a protected, trusted fingerprint policy. This
    /// still does not prove running-process identity or permit execution.
    pub fn inspect_signer(
        self,
        expected: SignerFingerprint,
    ) -> Result<SignedDevelopmentPlacement, PlacementError> {
        let signed = retain_expected_signer(&self.path, expected).map_err(PlacementError::File)?;
        if !signed
            .matches_open_file(&self.file)
            .map_err(PlacementError::File)?
        {
            return Err(PlacementError::FileMismatch);
        }
        Ok(SignedDevelopmentPlacement {
            _placement: self,
            _signed: signed,
        })
    }
}

#[must_use = "retain the checked placement and signed file together"]
pub struct SignedDevelopmentPlacement {
    _placement: DevelopmentPlacement,
    _signed: VerifiedFile,
}

#[must_use = "retain the checked placement and receipt together"]
pub struct ReceiptDevelopmentPlacement {
    _placement: DevelopmentPlacement,
    _receipt: File,
}
impl ReceiptDevelopmentPlacement {
    /// Retain the receipt while additionally checking the configured signer.
    /// This does not attest process identity or provide a product trust policy.
    pub fn inspect_signer(
        self,
        expected: SignerFingerprint,
    ) -> Result<SignedReceiptDevelopmentPlacement, PlacementError> {
        let signed = self._placement.inspect_signer(expected)?;
        Ok(SignedReceiptDevelopmentPlacement {
            _signed: signed,
            _receipt: self._receipt,
        })
    }
}
#[must_use = "retain the checked placement, receipt and signed file together"]
pub struct SignedReceiptDevelopmentPlacement {
    _signed: SignedDevelopmentPlacement,
    _receipt: File,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    purpose: String,
    sha256: String,
    signer_thumbprint: String,
    installed_dll: String,
    machine_registration_performed: bool,
}

fn validate_receipt(
    record: &Receipt,
    version: &str,
    expected: &Path,
    digest: &[u8; 32],
) -> Result<(), PlacementError> {
    let expected_path = expected.to_str().ok_or(PlacementError::InvalidReceipt)?;
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    if record.purpose != "Development staging only; not registered or trusted"
        || record.machine_registration_performed
        || record.signer_thumbprint.len() != 40
        || !record
            .signer_thumbprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || !record.sha256.eq_ignore_ascii_case(&hex)
        || version != format!("dev-{}", &hex[..16])
        || !record.installed_dll.eq_ignore_ascii_case(expected_path)
    {
        return Err(PlacementError::InvalidReceipt);
    }
    Ok(())
}

struct Algorithm(BCRYPT_ALG_HANDLE);
impl Drop for Algorithm {
    fn drop(&mut self) {
        unsafe {
            let _ = BCryptCloseAlgorithmProvider(self.0, 0);
        }
    }
}
struct Hash(BCRYPT_HASH_HANDLE);
impl Drop for Hash {
    fn drop(&mut self) {
        unsafe {
            let _ = BCryptDestroyHash(self.0);
        }
    }
}

fn hash_file(file: &File) -> Result<[u8; 32], PlacementError> {
    let size = file
        .metadata()
        .map_err(|_| PlacementError::HashFailed)?
        .len();
    if size == 0 || size > 64 * 1024 * 1024 {
        return Err(PlacementError::HashFailed);
    }
    let mut reader = file.try_clone().map_err(|_| PlacementError::HashFailed)?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|_| PlacementError::HashFailed)?;
    let (digest, count) = hash_reader(&mut reader)?;
    if count != size {
        return Err(PlacementError::HashFailed);
    }
    Ok(digest)
}
fn hash_reader(reader: &mut impl Read) -> Result<([u8; 32], u64), PlacementError> {
    let mut provider = BCRYPT_ALG_HANDLE::default();
    // SAFETY: handles are created by CNG, owned by the wrappers and released
    // in hash-before-provider order; all buffers outlive each CNG call.
    unsafe {
        BCryptOpenAlgorithmProvider(
            &mut provider,
            BCRYPT_SHA256_ALGORITHM,
            None,
            BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0),
        )
        .ok()
    }
    .map_err(|_| PlacementError::HashFailed)?;
    let provider = Algorithm(provider);
    let mut handle = BCRYPT_HASH_HANDLE::default();
    unsafe { BCryptCreateHash(provider.0, &mut handle, None, None, 0).ok() }
        .map_err(|_| PlacementError::HashFailed)?;
    let hash = Hash(handle);
    let mut total = 0u64;
    let mut buffer = [0u8; 32768];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| PlacementError::HashFailed)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or(PlacementError::HashFailed)?;
        if total > 64 * 1024 * 1024 {
            return Err(PlacementError::HashFailed);
        }
        unsafe { BCryptHashData(hash.0, &buffer[..read], 0).ok() }
            .map_err(|_| PlacementError::HashFailed)?;
    }
    let mut digest = [0u8; 32];
    unsafe { BCryptFinishHash(hash.0, &mut digest, 0).ok() }
        .map_err(|_| PlacementError::HashFailed)?;
    Ok((digest, total))
}

fn valid_version(version: &str) -> bool {
    version.len() == 20
        && version.starts_with("dev-")
        && version.as_bytes()[4..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sha256_matches_standard_vector_and_receipt_mismatches_are_rejected() {
        let (digest, count) = hash_reader(&mut std::io::Cursor::new(b"abc")).unwrap();
        assert_eq!(count, 3);
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let path = Path::new(r"D:\Program Files\Sumibi\dev-ba7816bf8f01cfea\sumibi_tip.dll");
        let mut receipt = Receipt {
            purpose: "Development staging only; not registered or trusted".into(),
            sha256: hex.to_ascii_uppercase(),
            signer_thumbprint: "A".repeat(40),
            installed_dll: path.to_str().unwrap().into(),
            machine_registration_performed: false,
        };
        assert_eq!(
            validate_receipt(&receipt, "dev-ba7816bf8f01cfea", path, &digest),
            Ok(())
        );
        receipt.machine_registration_performed = true;
        assert_eq!(
            validate_receipt(&receipt, "dev-ba7816bf8f01cfea", path, &digest),
            Err(PlacementError::InvalidReceipt)
        );
        receipt.machine_registration_performed = false;
        receipt.sha256.replace_range(0..1, "0");
        assert_eq!(
            validate_receipt(&receipt, "dev-ba7816bf8f01cfea", path, &digest),
            Err(PlacementError::InvalidReceipt)
        );
        receipt.sha256 = hex.to_ascii_uppercase();
        receipt.installed_dll.push('x');
        assert_eq!(
            validate_receipt(&receipt, "dev-ba7816bf8f01cfea", path, &digest),
            Err(PlacementError::InvalidReceipt)
        );
        receipt.installed_dll = path.to_str().unwrap().into();
        receipt.signer_thumbprint = "Z".repeat(40);
        assert_eq!(
            validate_receipt(&receipt, "dev-ba7816bf8f01cfea", path, &digest),
            Err(PlacementError::InvalidReceipt)
        );
        assert!(serde_json::from_str::<Receipt>(r#"{"purpose":"x","purpose":"y"}"#).is_err());
    }
    #[test]
    fn rejects_invalid_version_before_touching_disk() {
        for version in [
            "",
            "dev-123",
            "dev-1234567890abcdef/..",
            "dev-1234567890abcdeF",
            "dev-000000000000000g",
        ] {
            assert!(matches!(
                DevelopmentPlacement::open(Path::new(r"D:\"), version, Artifact::Tip),
                Err(PlacementError::InvalidVersion)
            ));
        }
        assert!(valid_version("dev-d74b8348334397d6"));
        assert!(matches!(
            DevelopmentPlacement::open(
                Path::new(r"D:\not-root"),
                "dev-d74b8348334397d6",
                Artifact::Tip
            ),
            Err(PlacementError::Acl(AclError::InvalidDescriptor))
        ));
        assert!(matches!(
            DevelopmentPlacement::open(
                Path::new("relative"),
                "dev-d74b8348334397d6",
                Artifact::Tip
            ),
            Err(PlacementError::Acl(AclError::InvalidDescriptor))
        ));
    }
    #[test]
    fn configured_development_placement_is_read_only_and_self_signed_is_denied() {
        let Some(root) = std::env::var_os("SUMIBI_TEST_DRIVE_ROOT") else {
            eprintln!("development placement test skipped: no fixture configured");
            return;
        };
        let version = std::env::var("SUMIBI_TEST_VERSION").expect("missing test version");
        let checked = DevelopmentPlacement::open(Path::new(&root), &version, Artifact::Tip)
            .expect("development placement")
            .inspect_receipt()
            .expect("development receipt and hash");
        assert!(matches!(
            checked.inspect_signer(SignerFingerprint([0; 32])),
            Err(PlacementError::File(InspectionError::NotTrusted(_)))
        ));
        println!("development placement checked; untrusted signature rejected");
    }
}
