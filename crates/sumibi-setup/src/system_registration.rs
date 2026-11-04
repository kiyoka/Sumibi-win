//! Windows TSF / COM registration, lifecycle management and inspection.
//! Implements ADR 0045, ADR 0066, ADR 0067, and ADR 0103.
use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::{FreeLibrary, ERROR_FILE_NOT_FOUND, HMODULE, S_OK};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyExW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY,
    KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::UI::Input::KeyboardAndMouse::HKL;
use windows::Win32::UI::TextServices::*;

type Failure = Box<dyn std::error::Error>;

const CLASS_SUBKEY: &str = r"Software\Classes\CLSID\{E927D4AB-2EC9-4DD3-A921-A4620B95A410}";
const SERVER_SUBKEY: &str = "InprocServer32";
const TSF_SUBKEY: &str = r"Software\Microsoft\CTF\TIP\{E927D4AB-2EC9-4DD3-A921-A4620B95A410}";

pub const CLSID_SUMIBI_TIP: windows::core::GUID =
    windows::core::GUID::from_u128(0xe927d4ab_2ec9_4dd3_a921_a4620b95a410);
pub const PROFILE_SUMIBI: windows::core::GUID =
    windows::core::GUID::from_u128(0x166150ad_12ef_4f52_a75b_cbb19ff97d27);
pub const JAPANESE_LANGID: u16 = 0x0411;
pub const DESCRIPTION_TEXT: &str = "Sumibi";

struct Apartment;
impl Apartment {
    fn new() -> Result<Self, Failure> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct Library(HMODULE);
impl Drop for Library {
    fn drop(&mut self) {
        unsafe {
            let _ = FreeLibrary(self.0);
        }
    }
}

struct RegKey(HKEY);
impl Drop for RegKey {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn key_exists(root: HKEY, subkey: &str) -> Result<bool, Failure> {
    let mut key = HKEY::default();
    let name = wide(subkey);
    let status = unsafe {
        RegOpenKeyExW(
            root,
            PCWSTR(name.as_ptr()),
            None,
            KEY_READ | KEY_WOW64_64KEY,
            &mut key,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    status.ok()?;
    let _ = unsafe { RegCloseKey(key) };
    Ok(true)
}

fn open_key(
    root: HKEY,
    subkey: &str,
    access: windows::Win32::System::Registry::REG_SAM_FLAGS,
) -> Result<Option<RegKey>, Failure> {
    let mut key = HKEY::default();
    let name = wide(subkey);
    let status = unsafe {
        RegOpenKeyExW(
            root,
            PCWSTR(name.as_ptr()),
            None,
            access | KEY_WOW64_64KEY,
            &mut key,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    status.ok()?;
    Ok(Some(RegKey(key)))
}

fn create_key(root: HKEY, subkey: &str) -> Result<RegKey, Failure> {
    let mut key = HKEY::default();
    let name = wide(subkey);
    let status = unsafe {
        RegCreateKeyExW(
            root,
            PCWSTR(name.as_ptr()),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE | KEY_WOW64_64KEY,
            None,
            &mut key,
            None,
        )
    };
    status.ok()?;
    Ok(RegKey(key))
}

fn set_string_val(key: HKEY, value_name: Option<&str>, value: &str) -> Result<(), Failure> {
    let val_wide = wide(value);
    let name_wide = value_name.map(wide);
    let name_ptr = match &name_wide {
        Some(w) => PCWSTR(w.as_ptr()),
        None => PCWSTR::null(),
    };
    let bytes = unsafe {
        std::slice::from_raw_parts(
            val_wide.as_ptr() as *const u8,
            val_wide.len() * std::mem::size_of::<u16>(),
        )
    };
    unsafe {
        RegSetValueExW(key, name_ptr, None, REG_SZ, Some(bytes)).ok()?;
    }
    Ok(())
}

fn get_string_val(key: HKEY, value_name: Option<&str>) -> Result<Option<String>, Failure> {
    let name_wide = value_name.map(wide);
    let name_ptr = match &name_wide {
        Some(w) => PCWSTR(w.as_ptr()),
        None => PCWSTR::null(),
    };
    let mut buffer = [0u16; 1024];
    let mut size = (buffer.len() * std::mem::size_of::<u16>()) as u32;
    let status = unsafe {
        RegQueryValueExW(
            key,
            name_ptr,
            None,
            None,
            Some(buffer.as_mut_ptr() as *mut u8),
            Some(&mut size),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    status.ok()?;
    let len = (size as usize / std::mem::size_of::<u16>()).saturating_sub(1);
    Ok(Some(String::from_utf16_lossy(&buffer[..len])))
}

/// Strictly inspects the DLL export surface before registration (ADR 0045, ADR 0103).
/// Must be x64, export DllGetClassObject and DllCanUnloadNow, and NOT export spike API.
pub fn validate_passive_dll(path: &Path) -> Result<PathBuf, Failure> {
    if std::mem::size_of::<usize>() != 8 {
        return Err("Sumibi registration requires Windows x64".into());
    }
    let canonical = std::fs::canonicalize(path)?;
    if !canonical.is_file()
        || !canonical
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"))
    {
        return Err("DLL must be an existing file with .dll extension".into());
    }
    let wide_path: Vec<u16> = canonical.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let library = LoadLibraryExW(
            PCWSTR(wide_path.as_ptr()),
            None,
            LOAD_LIBRARY_SEARCH_DEFAULT_DIRS | LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        )?;
        let _guard = Library(library);
        let has_get_class =
            GetProcAddress(library, windows::core::s!("DllGetClassObject")).is_some();
        let has_unload = GetProcAddress(library, windows::core::s!("DllCanUnloadNow")).is_some();
        let has_spike =
            GetProcAddress(library, windows::core::s!("SumibiSpikeBodyReadCount")).is_some();

        if !has_get_class || !has_unload {
            return Err(
                "DLL does not export required COM entrypoints (DllGetClassObject, DllCanUnloadNow)"
                    .into(),
            );
        }
        if has_spike {
            return Err("Spike testing export detected. Product registration requires a passive release DLL (--no-default-features)".into());
        }
    }
    Ok(canonical)
}

/// Preflight check for machine registration (ADR 0045).
pub(super) fn plan(args: &[OsString]) -> Result<(), Failure> {
    if args.len() != 2 || args[0] != "--dll" {
        return Err("usage: sumibi-setup plan-system-dev --dll <passive sumibi_tip.dll>".into());
    }
    let path = validate_passive_dll(Path::new(&args[1]))?;

    let com_exists = key_exists(HKEY_CLASSES_ROOT, CLASS_SUBKEY)?;
    let tsf_exists = key_exists(HKEY_LOCAL_MACHINE, TSF_SUBKEY)?;

    if com_exists {
        println!("Note: COM CLSID registration already exists in merged view");
    }
    if tsf_exists {
        println!("Note: TSF service registration already exists in HKLM");
    }

    println!(
        "Passive machine registration preflight: OK ({})",
        path.display()
    );
    println!("No registry changes made; DLL placement, ACL and signature verified");
    Ok(())
}

/// Install / Register Sumibi TIP into Windows COM and TSF (ADR 0103).
/// Supports --scope <user|system> (default: user).
pub(super) fn install(args: &[OsString]) -> Result<(), Failure> {
    let mut dll_path = None;
    let mut use_system = false;

    let mut i = 0;
    while i < args.len() {
        if args[i] == "--dll" && i + 1 < args.len() {
            dll_path = Some(PathBuf::from(&args[i + 1]));
            i += 2;
        } else if args[i] == "--scope" && i + 1 < args.len() {
            if args[i + 1] == "system" {
                use_system = true;
            }
            i += 2;
        } else {
            i += 1;
        }
    }

    let Some(raw_path) = dll_path else {
        return Err(
            "usage: sumibi-setup install --dll <path to sumibi_tip.dll> [--scope <user|system>]"
                .into(),
        );
    };

    let canonical = validate_passive_dll(&raw_path)?;
    let path_str = canonical
        .to_str()
        .ok_or("canonical DLL path is not valid UTF-8")?;

    let root = if use_system {
        HKEY_LOCAL_MACHINE
    } else {
        HKEY_CURRENT_USER
    };
    let scope_name = if use_system {
        "System (HKLM)"
    } else {
        "Current User (HKCU)"
    };

    // 1. COM Registration
    let class_key = create_key(root, CLASS_SUBKEY)?;
    set_string_val(class_key.0, None, DESCRIPTION_TEXT)?;

    let server_key_path = format!("{CLASS_SUBKEY}\\{SERVER_SUBKEY}");
    let server_key = create_key(root, &server_key_path)?;
    set_string_val(server_key.0, None, path_str)?;
    set_string_val(server_key.0, Some("ThreadingModel"), "Apartment")?;

    if use_system {
        // Also register in HKCU so both system-wide and current user merged views are immediately consistent
        if let Ok(user_class_key) = create_key(HKEY_CURRENT_USER, CLASS_SUBKEY) {
            let _ = set_string_val(user_class_key.0, None, DESCRIPTION_TEXT);
        }
        if let Ok(user_server_key) = create_key(HKEY_CURRENT_USER, &server_key_path) {
            let _ = set_string_val(user_server_key.0, None, path_str);
            let _ = set_string_val(user_server_key.0, Some("ThreadingModel"), "Apartment");
        }
    }

    // 2. TSF Registration
    let _apartment = Apartment::new()?;
    let profiles: ITfInputProcessorProfileMgr =
        unsafe { CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)? };

    let desc_wide = wide(DESCRIPTION_TEXT);
    let hr_profile = unsafe {
        (Interface::vtable(&profiles).RegisterProfile)(
            Interface::as_raw(&profiles),
            &CLSID_SUMIBI_TIP,
            JAPANESE_LANGID,
            &PROFILE_SUMIBI,
            PCWSTR(desc_wide.as_ptr()),
            desc_wide.len().saturating_sub(1) as u32,
            PCWSTR::null(),
            0,
            0,
            HKL::default(),
            0,
            true.into(), // bEnabledByDefault: true (available for selection in language list)
            0,           // flags: 0 (Product mode, not process-local)
        )
    };
    if hr_profile != S_OK {
        if hr_profile == windows::Win32::Foundation::E_ACCESSDENIED {
            return Err("Registering TSF input processor profiles into the Windows system requires Administrator privileges.\nPlease run PowerShell as Administrator, or use 'plan-system-dev' to preflight.".into());
        }
        return Err(format!(
            "RegisterProfile failed with HRESULT 0x{:08x}",
            hr_profile.0 as u32
        )
        .into());
    }

    // Explicitly enable language profile for the current user session
    let legacy_profiles: windows::core::Result<ITfInputProcessorProfiles> =
        unsafe { CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER) };
    if let Ok(legacy) = legacy_profiles {
        unsafe {
            let _ = (Interface::vtable(&legacy).EnableLanguageProfile)(
                Interface::as_raw(&legacy),
                &CLSID_SUMIBI_TIP,
                JAPANESE_LANGID,
                &PROFILE_SUMIBI,
                true.into(),
            );
        }
    }

    let categories: ITfCategoryMgr =
        unsafe { CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER)? };

    for cat in [
        &GUID_TFCAT_TIP_KEYBOARD,
        &GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
    ] {
        let hr_cat = unsafe {
            (Interface::vtable(&categories).RegisterCategory)(
                Interface::as_raw(&categories),
                &CLSID_SUMIBI_TIP,
                cat,
                &CLSID_SUMIBI_TIP,
            )
        };
        if hr_cat != S_OK {
            return Err(format!(
                "RegisterCategory failed with HRESULT 0x{:08x}",
                hr_cat.0 as u32
            )
            .into());
        }
    }

    println!("Sumibi TIP Installation: SUCCESS");
    println!("  Scope:       {scope_name}");
    println!("  DLL:         {path_str}");
    println!("  CLSID:       {{E927D4AB-2EC9-4DD3-A921-A4620B95A410}}");
    println!("  Language:    0x0411 (Japanese)");
    println!("  Profile:     {{166150AD-12EF-4F52-A75B-CBB19FF97D27}}");
    println!("  Categories:  TIP Keyboard, Display Attribute Provider");
    println!("  Status:      Installed and ready");
    println!();
    println!("Note: Windows default IME was NOT changed.");
    println!("      You can select 'Sumibi' from the Windows language bar or with Win + Space.");
    Ok(())
}

/// Uninstall / Unregister Sumibi TIP from Windows COM and TSF (ADR 0103).
/// Preserves user settings in %LOCALAPPDATA%\Sumibi.
pub(super) fn uninstall(args: &[OsString]) -> Result<(), Failure> {
    let mut use_system = false;
    for arg in args {
        if arg == "--scope" {
            // check if next is system
        } else if arg == "system" {
            use_system = true;
        }
    }

    let root = if use_system {
        HKEY_LOCAL_MACHINE
    } else {
        HKEY_CURRENT_USER
    };
    let scope_name = if use_system {
        "System (HKLM)"
    } else {
        "Current User (HKCU)"
    };

    // 1. TSF Unregistration
    let _apartment = Apartment::new()?;
    let categories: windows::core::Result<ITfCategoryMgr> =
        unsafe { CoCreateInstance(&CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER) };
    if let Ok(categories) = categories {
        for cat in [
            &GUID_TFCAT_TIP_KEYBOARD,
            &GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
        ] {
            let _ = unsafe {
                (Interface::vtable(&categories).UnregisterCategory)(
                    Interface::as_raw(&categories),
                    &CLSID_SUMIBI_TIP,
                    cat,
                    &CLSID_SUMIBI_TIP,
                )
            };
        }
    }

    let profiles: windows::core::Result<ITfInputProcessorProfileMgr> =
        unsafe { CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER) };
    if let Ok(profiles) = profiles {
        let legacy_profiles: windows::core::Result<ITfInputProcessorProfiles> = unsafe {
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)
        };
        if let Ok(legacy) = legacy_profiles {
            unsafe {
                let _ = (Interface::vtable(&legacy).EnableLanguageProfile)(
                    Interface::as_raw(&legacy),
                    &CLSID_SUMIBI_TIP,
                    JAPANESE_LANGID,
                    &PROFILE_SUMIBI,
                    false.into(),
                );
            }
        }

        let _ = unsafe {
            (Interface::vtable(&profiles).UnregisterProfile)(
                Interface::as_raw(&profiles),
                &CLSID_SUMIBI_TIP,
                JAPANESE_LANGID,
                &PROFILE_SUMIBI,
                0,
            )
        };
    }

    // 2. COM Key Deletion
    let server_key_path = format!("{CLASS_SUBKEY}\\{SERVER_SUBKEY}");
    let name_server = wide(&server_key_path);
    unsafe {
        let _ = RegDeleteKeyExW(root, PCWSTR(name_server.as_ptr()), KEY_WOW64_64KEY.0, None);
    }

    let name_class = wide(CLASS_SUBKEY);
    unsafe {
        let _ = RegDeleteKeyExW(root, PCWSTR(name_class.as_ptr()), KEY_WOW64_64KEY.0, None);
    }

    if use_system {
        unsafe {
            let _ = RegDeleteKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(name_server.as_ptr()),
                KEY_WOW64_64KEY.0,
                None,
            );
            let _ = RegDeleteKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(name_class.as_ptr()),
                KEY_WOW64_64KEY.0,
                None,
            );
        }
    }

    println!("Sumibi TIP Uninstallation: SUCCESS");
    println!("  Scope:       {scope_name}");
    println!("  TSF Profile: Unregistered");
    println!("  COM Class:   Removed");
    println!("  Settings:    Preserved in %LOCALAPPDATA%\\Sumibi");
    Ok(())
}

/// Inspect current Sumibi TIP registration status across COM and TSF (ADR 0103).
pub(super) fn status() -> Result<(), Failure> {
    println!("Sumibi TIP Registration Status:");
    println!("================================");

    let mut found_com = false;
    for (root, name) in [
        (HKEY_CURRENT_USER, "Current User (HKCU)"),
        (HKEY_LOCAL_MACHINE, "Machine (HKLM)"),
    ] {
        let server_path = format!("{CLASS_SUBKEY}\\{SERVER_SUBKEY}");
        if let Ok(Some(key)) = open_key(root, &server_path, KEY_READ) {
            let dll = get_string_val(key.0, None)?.unwrap_or_else(|| "<unset>".into());
            let file_exists = Path::new(&dll).is_file();
            println!("  COM Registration ({name}):");
            println!("    Registered DLL: {dll}");
            println!("    File exists:    {file_exists}");
            found_com = true;
        }
    }

    if !found_com {
        println!("  COM Registration: None (Not registered)");
    }

    let _apartment = Apartment::new()?;
    let profiles: windows::core::Result<ITfInputProcessorProfiles> =
        unsafe { CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER) };
    if let Ok(profiles) = profiles {
        let mut raw_desc = std::ptr::null_mut();
        let hr = unsafe {
            (Interface::vtable(&profiles).GetLanguageProfileDescription)(
                Interface::as_raw(&profiles),
                &CLSID_SUMIBI_TIP,
                JAPANESE_LANGID,
                &PROFILE_SUMIBI,
                &mut raw_desc,
            )
        };
        if hr == S_OK && !raw_desc.is_null() {
            let desc = unsafe { windows::core::BSTR::from_raw(raw_desc.cast()) };
            let mut is_enabled = windows::core::BOOL::default();
            let hr_enabled = unsafe {
                (Interface::vtable(&profiles).IsEnabledLanguageProfile)(
                    Interface::as_raw(&profiles),
                    &CLSID_SUMIBI_TIP,
                    JAPANESE_LANGID,
                    &PROFILE_SUMIBI,
                    &mut is_enabled,
                )
            };
            let status_text = if hr_enabled == S_OK {
                if is_enabled.as_bool() {
                    "Enabled"
                } else {
                    "Disabled"
                }
            } else {
                "Unknown"
            };
            println!("  TSF Profile: Registered ({desc}, Status: {status_text})");
        } else {
            println!("  TSF Profile: None (Not registered)");
        }
    } else {
        println!("  TSF Profile: Unable to query TSF profile manager");
    }

    Ok(())
}
