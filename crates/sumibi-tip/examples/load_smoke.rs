//! Load the actual DLL in a disposable process, without COM/TSF registration.
#![cfg(windows)]

#[path = "../src/mock.rs"]
mod mock;

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::ptr;
use windows::core::{Interface, GUID, HRESULT, PCWSTR};
use windows::Win32::Foundation::{FreeLibrary, S_FALSE, S_OK};
use windows::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, IClassFactory, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
};
use windows::Win32::UI::TextServices::*;

fn display_attributes(tip: &ITfTextInputProcessorEx) -> windows::core::Result<()> {
    let provider: ITfDisplayAttributeProvider = tip.cast()?;
    // SAFETY: valid same-thread COM interfaces and initialized output slots.
    // Raw enumeration calls check S_FALSE, which the convenience API erases.
    unsafe {
        assert!(provider.GetDisplayAttributeInfo(ptr::null()).is_err());
        assert!(provider.GetDisplayAttributeInfo(&GUID::zeroed()).is_err());
        let info = provider.GetDisplayAttributeInfo(&sumibi_tip::display::GUID_INPUT_ATTRIBUTE)?;
        assert_eq!(info.GetGUID()?, sumibi_tip::display::GUID_INPUT_ATTRIBUTE);
        assert!(!info.GetDescription()?.is_empty());
        let mut attribute = TF_DISPLAYATTRIBUTE::default();
        info.GetAttributeInfo(&mut attribute)?;
        assert_eq!(attribute.lsStyle, TF_LS_DOT);
        assert_eq!(attribute.crText.r#type, TF_CT_NONE);
        assert_eq!(attribute.crBk.r#type, TF_CT_NONE);
        assert_eq!(attribute.bAttr, TF_ATTR_INPUT);
        assert!(!attribute.fBoldLine.as_bool());
        assert!(info.GetAttributeInfo(ptr::null_mut()).is_err());
        assert_eq!(
            info.SetAttributeInfo(&attribute).unwrap_err().code(),
            windows::Win32::Foundation::E_NOTIMPL
        );
        info.Reset()?;
        let items = provider.EnumDisplayAttributeInfo()?;
        let mut slot: Option<ITfDisplayAttributeInfo> = None;
        let mut count = 0;
        assert_eq!(
            (items.vtable().Next)(
                items.as_raw(),
                1,
                (&mut slot as *mut Option<ITfDisplayAttributeInfo>).cast(),
                &mut count
            ),
            S_OK
        );
        assert_eq!(count, 1);
        assert_eq!(slot.take().unwrap().GetGUID()?, info.GetGUID()?);
        let copy = items.Clone()?;
        for enumerator in [&items, &copy] {
            count = 99;
            assert_eq!(
                (enumerator.vtable().Next)(
                    enumerator.as_raw(),
                    1,
                    (&mut slot as *mut Option<ITfDisplayAttributeInfo>).cast(),
                    &mut count
                ),
                S_FALSE
            );
            assert_eq!(count, 0);
            assert!(slot.is_none());
        }
        items.Reset()?;
        let mut slots: [Option<ITfDisplayAttributeInfo>; 2] = [None, None];
        assert_eq!(
            (items.vtable().Next)(items.as_raw(), 2, slots.as_mut_ptr().cast(), &mut count),
            S_FALSE
        );
        assert_eq!(count, 1);
        assert!(slots[0].take().is_some());
        items.Reset()?;
        assert_eq!((items.vtable().Skip)(items.as_raw(), 1), S_OK);
        assert_eq!((items.vtable().Skip)(items.as_raw(), 1), S_FALSE);
        assert_eq!(
            (items.vtable().Next)(
                items.as_raw(),
                2,
                slots.as_mut_ptr().cast(),
                ptr::null_mut()
            ),
            windows::Win32::Foundation::E_POINTER
        );
    }
    println!("DLL display attribute provider / enumeration HRESULTs: OK");
    Ok(())
}

type GetClassObject =
    unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut c_void) -> HRESULT;
type CanUnload = unsafe extern "system" fn() -> HRESULT;

struct Apartment;

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: matches the successful initialization on this main thread.
        unsafe { CoUninitialize() };
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os().nth(1).ok_or("provide the DLL path")?;
    if std::env::args_os().nth(2).is_some() {
        return Err("unexpected argument; use sumibi-setup probe for real TSF activation".into());
    }
    let path = std::fs::canonicalize(path)?;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: dedicated main thread; apartment outlives every COM reference.
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
    let _apartment = Apartment;
    // SAFETY: path is an absolute, NUL-terminated path to the DLL under test.
    // Search is restricted to its directory and the standard safe directories.
    let module = unsafe {
        LoadLibraryExW(
            PCWSTR(wide.as_ptr()),
            None,
            LOAD_LIBRARY_SEARCH_DEFAULT_DIRS | LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        )?
    };
    let checked = (|| -> Result<(), Box<dyn std::error::Error>> {
        // SAFETY: the loaded DLL is our build artifact and these exports have the
        // exact signatures declared above. No function pointer outlives module.
        unsafe {
            let get_class: GetClassObject = std::mem::transmute(
                GetProcAddress(module, windows::core::s!("DllGetClassObject"))
                    .ok_or("missing DllGetClassObject")?,
            );
            let can_unload: CanUnload = std::mem::transmute(
                GetProcAddress(module, windows::core::s!("DllCanUnloadNow"))
                    .ok_or("missing DllCanUnloadNow")?,
            );
            assert_eq!(can_unload(), S_OK);
            let mut output = ptr::null_mut();
            get_class(
                &sumibi_tip::CLSID_SUMIBI_TIP,
                &IClassFactory::IID,
                &mut output,
            )
            .ok()?;
            let factory = IClassFactory::from_raw(output);
            let tip: ITfTextInputProcessorEx = factory.CreateInstance(None)?;
            assert_eq!(can_unload(), S_FALSE);
            mock::exercise(&tip, &factory);
            display_attributes(&tip)?;
            let retained = tip
                .cast::<ITfDisplayAttributeProvider>()?
                .GetDisplayAttributeInfo(&sumibi_tip::display::GUID_INPUT_ATTRIBUTE)?;
            drop(factory);
            assert_eq!(can_unload(), S_FALSE);
            drop(tip);
            assert_eq!(can_unload(), S_FALSE);
            assert_eq!(
                retained.GetGUID()?,
                sumibi_tip::display::GUID_INPUT_ATTRIBUTE
            );
            drop(retained);
            assert_eq!(can_unload(), S_OK);
        }
        Ok(())
    })();
    // SAFETY: the closure has released all COM interfaces and function pointers.
    unsafe { FreeLibrary(module)? };
    checked?;
    println!("DLL load / mock TSF activation / key sink subscription / cleanup / unload: OK");
    Ok(())
}
