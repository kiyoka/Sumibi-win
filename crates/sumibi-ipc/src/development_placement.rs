//! Read-only development placement inspection. Not a product launch policy.
use crate::authenticode::{
    retain_expected_signer, InspectionError, SignerFingerprint, VerifiedFile,
};
use crate::protected_acl::{
    check_file, open_development_ancestor, open_development_directory, AclError, AncestorKind,
    DevelopmentAncestor, DevelopmentDirectory,
};
use std::fs::{File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
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
        let placement = DevelopmentPlacement::open(Path::new(&root), &version, Artifact::Tip)
            .expect("development placement");
        assert!(matches!(
            placement.inspect_signer(SignerFingerprint([0; 32])),
            Err(PlacementError::File(InspectionError::NotTrusted(_)))
        ));
        println!("development placement checked; untrusted signature rejected");
    }
}
