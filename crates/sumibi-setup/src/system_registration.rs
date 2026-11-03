//! Read-only planning for an eventual machine-wide passive TIP installation.
//! The installer must first solve protected DLL placement and update/rollback.
use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{FreeLibrary, ERROR_FILE_NOT_FOUND};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, HKEY, HKEY_CLASSES_ROOT, HKEY_LOCAL_MACHINE, KEY_READ,
};

type Failure = Box<dyn std::error::Error>;
const CLASS_KEY: &str = r"CLSID\{E927D4AB-2EC9-4DD3-A921-A4620B95A410}";
const TSF_KEY: &str = r"Software\Microsoft\CTF\TIP\{E927D4AB-2EC9-4DD3-A921-A4620B95A410}";

fn exists(root: HKEY, name: &str) -> Result<bool, Failure> {
    let mut key = HKEY::default();
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: read-only open of a fixed registry path and initialized output.
    let status = unsafe { RegOpenKeyExW(root, PCWSTR(name.as_ptr()), None, KEY_READ, &mut key) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    status.ok()?;
    let _ = unsafe { RegCloseKey(key) };
    Ok(true)
}

pub(super) fn plan(args: &[OsString]) -> Result<(), Failure> {
    if args.len() != 2 || args[0] != "--dll" {
        return Err("usage: sumibi-setup plan-system-dev --dll <passive sumibi_tip.dll>".into());
    }
    if std::mem::size_of::<usize>() != 8 {
        return Err("machine registration plan requires Windows x64".into());
    }
    let path = std::fs::canonicalize(Path::new(&args[1]))?;
    if !path.is_file()
        || !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"))
    {
        return Err("DLL must be a file with .dll extension".into());
    }
    if path.to_str().is_none() {
        return Err("DLL path must be valid Unicode for COM registration".into());
    }
    let path_wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: only inspect exports. LoadLibraryExW can execute DllMain, so
    // callers must provide their own trusted development DLL.
    unsafe {
        let library = LoadLibraryExW(
            PCWSTR(path_wide.as_ptr()),
            None,
            LOAD_LIBRARY_SEARCH_DEFAULT_DIRS | LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        )?;
        let valid = GetProcAddress(library, windows::core::s!("DllGetClassObject")).is_some()
            && GetProcAddress(library, windows::core::s!("DllCanUnloadNow")).is_some()
            && GetProcAddress(library, windows::core::s!("SumibiSpikeBodyReadCount")).is_none();
        FreeLibrary(library)?;
        if !valid {
            return Err("machine registration requires a passive non-spike TIP DLL".into());
        }
    }
    if exists(HKEY_CLASSES_ROOT, CLASS_KEY)? {
        return Err("CLSID already registered; refusing to shadow or overwrite it".into());
    }
    if exists(HKEY_LOCAL_MACHINE, TSF_KEY)? {
        return Err("TSF service already registered; refusing to overwrite it".into());
    }
    println!(
        "Passive machine registration preflight: OK ({})",
        path.display()
    );
    println!("No registry changes made; DLL placement, ACL and signature still need review");
    Ok(())
}
