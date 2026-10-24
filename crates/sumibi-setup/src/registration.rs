//! Opt-in development COM registration. TSF profile registration is separate.
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use windows::core::{Result as WinResult, PCWSTR};
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Registry::*;
use windows::Win32::UI::TextServices::ITfTextInputProcessorEx;

type Failure = Box<dyn std::error::Error>;
const CLASS_KEY: &str = r"Software\Classes\CLSID\{E927D4AB-2EC9-4DD3-A921-A4620B95A410}";
const SERVER_KEY: &str = "InprocServer32";
const MARKER: &str = "SumibiDevelopmentRegistration";
const MARKER_VALUE: &str = "sumibi-win-com-dev-v1";

struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: only successful RegOpenKeyExW/RegCreateKeyExW results are stored.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn bytes(value: &str) -> Vec<u8> {
    wide(value).into_iter().flat_map(u16::to_le_bytes).collect()
}

fn open_class(access: REG_SAM_FLAGS) -> Result<Option<Key>, Failure> {
    let mut key = HKEY::default();
    let name = wide(CLASS_KEY);
    // SAFETY: the output handle is initialized and the terminated name lives through the call.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(name.as_ptr()),
            None,
            access,
            &mut key,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    status.ok()?;
    Ok(Some(Key(key)))
}

fn class_exists_in_merged_view() -> Result<bool, Failure> {
    let mut key = HKEY::default();
    let name = wide(r"CLSID\{E927D4AB-2EC9-4DD3-A921-A4620B95A410}");
    // SAFETY: HKEY_CLASSES_ROOT is read only here and the output is initialized.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CLASSES_ROOT,
            PCWSTR(name.as_ptr()),
            None,
            KEY_READ,
            &mut key,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    status.ok()?;
    drop(Key(key));
    Ok(true)
}

fn create_key(parent: HKEY, name: &str) -> Result<(Key, bool), Failure> {
    let mut key = HKEY::default();
    let mut disposition = REG_CREATE_KEY_DISPOSITION::default();
    let name = wide(name);
    // SAFETY: caller supplies a valid parent, output slots and terminated name.
    unsafe {
        RegCreateKeyExW(
            parent,
            PCWSTR(name.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            None,
            &mut key,
            Some(&mut disposition),
        )
        .ok()?;
    }
    Ok((Key(key), disposition == REG_CREATED_NEW_KEY))
}

fn put(key: HKEY, name: Option<&str>, value: &str) -> WinResult<()> {
    let name = name.map(wide);
    let name = name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()));
    // SAFETY: the key is open; the terminated name and UTF-16 value remain live.
    unsafe { RegSetValueExW(key, name, None, REG_SZ, Some(&bytes(value))).ok() }
}

fn get(key: HKEY, name: Option<&str>) -> Result<String, Failure> {
    let name = name.map(wide);
    let name = name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()));
    let mut kind = REG_VALUE_TYPE::default();
    let mut length = 0;
    // SAFETY: first call obtains the size; second fills an owned bounded buffer.
    unsafe {
        RegQueryValueExW(key, name, None, Some(&mut kind), None, Some(&mut length)).ok()?;
    }
    if kind != REG_SZ || !(2..=65536).contains(&length) || length % 2 != 0 {
        return Err("invalid development COM registry value".into());
    }
    let mut data = vec![0u8; length as usize];
    unsafe {
        RegQueryValueExW(
            key,
            name,
            None,
            Some(&mut kind),
            Some(data.as_mut_ptr()),
            Some(&mut length),
        )
        .ok()?;
    }
    if kind != REG_SZ || length as usize != data.len() {
        return Err("development COM registry value changed during read".into());
    }
    let units: Vec<u16> = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    if units.last() != Some(&0) || units[..units.len() - 1].contains(&0) {
        return Err("invalid development COM registry string".into());
    }
    Ok(OsString::from_wide(&units[..units.len() - 1])
        .to_string_lossy()
        .into_owned())
}

fn server(class: HKEY) -> Result<Key, Failure> {
    let mut key = HKEY::default();
    let name = wide(SERVER_KEY);
    // SAFETY: class handle is open and output slot is writable.
    unsafe { RegOpenKeyExW(class, PCWSTR(name.as_ptr()), None, KEY_READ, &mut key).ok()? };
    Ok(Key(key))
}

fn registered_path() -> Result<PathBuf, Failure> {
    let class = open_class(KEY_READ)?.ok_or("Sumibi development COM registration is absent")?;
    if get(class.0, Some(MARKER))? != MARKER_VALUE {
        return Err("CLSID is not owned by this development registration".into());
    }
    let server = server(class.0)?;
    if get(server.0, Some("ThreadingModel"))? != "Apartment" {
        return Err("unexpected COM threading model; refusing to modify registration".into());
    }
    Ok(PathBuf::from(get(server.0, None)?))
}

fn activate() -> Result<(), Failure> {
    // SAFETY: this command is a single-threaded console process.
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe { CoUninitialize() };
            }
        }
        let _apartment = Apartment;
        let tip: ITfTextInputProcessorEx =
            CoCreateInstance(&sumibi_tip::CLSID_SUMIBI_TIP, None, CLSCTX_INPROC_SERVER)?;
        drop(tip);
    }
    Ok(())
}

pub(super) fn register(args: &[OsString]) -> Result<(), Failure> {
    if args.len() != 2 || args[0] != "--dll" {
        return Err("usage: sumibi-setup register-com-dev --dll <sumibi_tip.dll>".into());
    }
    if std::mem::size_of::<usize>() != 8 {
        return Err("development COM registration requires Windows x64".into());
    }
    let path = std::fs::canonicalize(Path::new(&args[1]))?;
    if !path.is_file()
        || !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"))
    {
        return Err("DLL must be a file with .dll extension".into());
    }
    if class_exists_in_merged_view()? {
        return Err("CLSID already registered; refusing to shadow or overwrite it".into());
    }
    let (class, fresh) = create_key(HKEY_CURRENT_USER, CLASS_KEY)?;
    if !fresh {
        return Err("CLSID appeared during registration; refusing to overwrite it".into());
    }
    let outcome = (|| {
        put(class.0, Some(MARKER), MARKER_VALUE)?;
        let (server, fresh) = create_key(class.0, SERVER_KEY)?;
        if !fresh {
            return Err("InprocServer32 appeared during registration".into());
        }
        put(server.0, None, &path.to_string_lossy())?;
        put(server.0, Some("ThreadingModel"), "Apartment")?;
        drop(server);
        drop(class);
        verify()?;
        Ok(())
    })();
    if outcome.is_err() {
        // The exact CLSID key was absent before this command. Best-effort
        // rollback leaves an explicit error if cleanup also fails.
        let rollback = remove_fresh();
        if let Err(error) = rollback {
            return Err(format!("registration failed; rollback failed: {error}").into());
        }
    }
    outcome
}

pub(super) fn verify() -> Result<(), Failure> {
    let path = registered_path()?;
    if !path.is_file() {
        return Err(format!("registered DLL no longer exists: {}", path.display()).into());
    }
    activate()?;
    println!("Current-user COM activation: OK ({})", path.display());
    Ok(())
}

fn remove_owned() -> Result<(), Failure> {
    let class = open_class(KEY_READ | KEY_WRITE)?.ok_or("registration is absent")?;
    if get(class.0, Some(MARKER))? != MARKER_VALUE {
        return Err("CLSID is not owned by this development registration".into());
    }
    drop(server(class.0)?);
    let name = wide(SERVER_KEY);
    // SAFETY: remove only the exact child key owned by this registration.
    unsafe { RegDeleteKeyW(class.0, PCWSTR(name.as_ptr())).ok()? };
    drop(class);
    let name = wide(CLASS_KEY);
    // SAFETY: RegDeleteKeyW refuses to delete a key with unexpected children.
    unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(name.as_ptr())).ok()? };
    Ok(())
}

fn remove_fresh() -> Result<(), Failure> {
    let class = open_class(KEY_READ | KEY_WRITE)?.ok_or("new registration disappeared")?;
    let name = wide(SERVER_KEY);
    // SAFETY: root was created by this command; no pre-existing registration
    // can be removed. A missing child is expected after an early failure.
    let child = unsafe { RegDeleteKeyW(class.0, PCWSTR(name.as_ptr())) };
    if child != ERROR_FILE_NOT_FOUND {
        child.ok()?;
    }
    drop(class);
    let name = wide(CLASS_KEY);
    unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(name.as_ptr())).ok()? };
    Ok(())
}

pub(super) fn unregister() -> Result<(), Failure> {
    let path = registered_path()?;
    remove_owned()?;
    println!(
        "Current-user development COM registration removed: {}",
        path.display()
    );
    Ok(())
}
