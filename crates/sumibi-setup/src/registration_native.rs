//! Native registration API adapter; no default activation or broad unregister.
//! The only caller currently uses low-integrity redirected registry fixtures.
use super::registration_registry::Failure;
use windows::core::{Interface, GUID, HRESULT};
use windows::Win32::Foundation::S_OK;
use windows::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, IClassFactory, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::{
    GetModuleHandleW, GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
};
use windows::Win32::UI::Input::KeyboardAndMouse::HKL;
use windows::Win32::UI::TextServices::*;

pub(super) const JAPANESE: u16 = 0x0411;
pub(super) const DESCRIPTION: &str = "Sumibi isolated registration fixture";
pub(super) struct Apartment;
impl Apartment {
    pub fn new() -> Result<Self, Failure> {
        // SAFETY: this is a single-threaded console worker; balanced on same thread.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

pub(super) struct Factories {
    profiles: IClassFactory,
    categories: IClassFactory,
    // Deliberately retain the OS module until this dedicated process exits.
    // MSCTF may retain process-global objects after our own references close.
    _library: windows::Win32::Foundation::HMODULE,
}
impl Factories {
    pub fn new() -> Result<Self, Failure> {
        // SAFETY: call only after registry redirection. Reject preloaded MSCTF:
        // its process-global registry cache could predate the test mapping.
        unsafe {
            if GetModuleHandleW(windows::core::w!("msctf.dll")).is_ok() {
                return Err(
                    "native fixture needs the dedicated worker without preloaded MSCTF".into(),
                );
            }
            let library = LoadLibraryExW(
                windows::core::w!("msctf.dll"),
                None,
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )?;
            let address = GetProcAddress(library, windows::core::s!("DllGetClassObject"))
                .ok_or("system MSCTF class factory export missing")?;
            type GetClass = unsafe extern "system" fn(
                *const GUID,
                *const GUID,
                *mut *mut std::ffi::c_void,
            ) -> HRESULT;
            let get_class: GetClass = std::mem::transmute(address);
            let get = |class: &GUID| -> Result<IClassFactory, Failure> {
                let mut raw = std::ptr::null_mut();
                exact(
                    "MSCTF DllGetClassObject",
                    get_class(class, &IClassFactory::IID, &mut raw),
                )?;
                if raw.is_null() {
                    return Err("MSCTF returned a null factory".into());
                }
                Ok(IClassFactory::from_raw(raw))
            };
            Ok(Self {
                profiles: get(&CLSID_TF_InputProcessorProfiles)?,
                categories: get(&CLSID_TF_CategoryMgr)?,
                _library: library,
            })
        }
    }
    pub fn instantiate(&self) -> Result<NativeTsf, Failure> {
        // SAFETY: no aggregation; the returned interfaces stay on the owner STA.
        unsafe {
            Ok(NativeTsf {
                profiles: self.profiles.CreateInstance(None)?,
                categories: self.categories.CreateInstance(None)?,
            })
        }
    }
}
pub(super) struct NativeTsf {
    profiles: ITfInputProcessorProfileMgr,
    categories: ITfCategoryMgr,
}
fn exact(operation: &str, hr: HRESULT) -> Result<(), Failure> {
    if hr != S_OK {
        return Err(format!("{operation} expected S_OK, got 0x{:08x}", hr.0 as u32).into());
    }
    Ok(())
}
impl NativeTsf {
    pub fn register_profile(&self, class: &GUID, profile: &GUID) -> Result<(), Failure> {
        let description: Vec<u16> = DESCRIPTION.encode_utf16().collect();
        // SAFETY: valid refs/slices; no default enable, no substitute layout,
        // no local-process flags. The fixture is hidden and never activated.
        let hr = unsafe {
            (Interface::vtable(&self.profiles).RegisterProfile)(
                Interface::as_raw(&self.profiles),
                class,
                JAPANESE,
                profile,
                windows::core::PCWSTR(description.as_ptr()),
                description.len() as u32,
                windows::core::PCWSTR::null(),
                0,
                0,
                HKL::default(),
                0,
                false.into(),
                TF_RP_HIDDENINSETTINGUI,
            )
        };
        exact("RegisterProfile", hr)
    }
    pub fn unregister_profile(&self, class: &GUID, profile: &GUID) -> Result<(), Failure> {
        // SAFETY: exact tuple only, flags zero: never TF_URP_ALLPROFILES.
        let hr = unsafe {
            (Interface::vtable(&self.profiles).UnregisterProfile)(
                Interface::as_raw(&self.profiles),
                class,
                JAPANESE,
                profile,
                0,
            )
        };
        exact("UnregisterProfile", hr)
    }
    pub fn register_category(&self, class: &GUID, category: &GUID) -> Result<(), Failure> {
        allowed_category(category)?;
        // SAFETY: exact owner/category/item tuple; no unsupported capability category.
        let hr = unsafe {
            (Interface::vtable(&self.categories).RegisterCategory)(
                Interface::as_raw(&self.categories),
                class,
                category,
                class,
            )
        };
        exact("RegisterCategory", hr)
    }
    pub fn unregister_category(&self, class: &GUID, category: &GUID) -> Result<(), Failure> {
        allowed_category(category)?;
        // SAFETY: exact same tuple; no class-wide cleanup.
        let hr = unsafe {
            (Interface::vtable(&self.categories).UnregisterCategory)(
                Interface::as_raw(&self.categories),
                class,
                category,
                class,
            )
        };
        exact("UnregisterCategory", hr)
    }
    pub fn verify_disabled(&self, class: &GUID, profile: &GUID) -> Result<(), Failure> {
        let profiles: ITfInputProcessorProfiles = self.profiles.cast()?;
        let mut raw_description = std::ptr::null_mut();
        // SAFETY: exact tuple and initialized BSTR output, released on all paths.
        let hr = unsafe {
            (Interface::vtable(&profiles).GetLanguageProfileDescription)(
                Interface::as_raw(&profiles),
                class,
                JAPANESE,
                profile,
                &mut raw_description,
            )
        };
        let description = unsafe { windows::core::BSTR::from_raw(raw_description.cast()) };
        exact("GetLanguageProfileDescription", hr)?;
        if !description.iter().copied().eq(DESCRIPTION.encode_utf16()) {
            return Err("native profile description mismatch".into());
        }
        let mut enabled = windows::core::BOOL(1);
        // SAFETY: initialized BOOL; a failed read can never look like disabled.
        let hr = unsafe {
            (Interface::vtable(&profiles).IsEnabledLanguageProfile)(
                Interface::as_raw(&profiles),
                class,
                JAPANESE,
                profile,
                &mut enabled,
            )
        };
        exact("IsEnabledLanguageProfile", hr)?;
        if enabled.as_bool() {
            return Err("native profile unexpectedly enabled".into());
        }
        Ok(())
    }
}
fn allowed_category(category: &GUID) -> Result<(), Failure> {
    if *category != GUID_TFCAT_TIP_KEYBOARD && *category != GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER {
        return Err("unsupported text service capability category".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn s_false_and_failed_hresult_never_count_as_completed_mutation() {
        assert!(exact("test", S_OK).is_ok());
        assert!(exact("test", windows::Win32::Foundation::S_FALSE).is_err());
        assert!(exact("test", windows::core::HRESULT(0x80004005u32 as i32)).is_err());
    }
    #[test]
    fn unsupported_capability_categories_are_rejected() {
        assert!(allowed_category(&GUID_TFCAT_TIP_KEYBOARD).is_ok());
        assert!(allowed_category(&GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER).is_ok());
        assert!(allowed_category(&GUID_TFCAT_TIPCAP_SECUREMODE).is_err());
        assert!(allowed_category(&GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT).is_err());
    }
}
