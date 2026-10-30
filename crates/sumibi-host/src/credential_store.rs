//! Host-only bridge to Windows Credential Manager. Consent remains separate.
use crate::manual::Credential;
use sumibi_credentials::StoreError;

/// Load a key as a Host credential. The caller must replace the policy key
/// through PolicyAuthority so existing consent is invalidated and new consent
/// is explicitly requested. Storage errors must not preserve old authority.
pub fn load_credential() -> Result<Option<Credential>, StoreError> {
    match sumibi_credentials::load()? {
        Some(secret) => Credential::new(secret)
            .map(Some)
            .ok_or(StoreError::InvalidRecord),
        None => Ok(None),
    }
}
