//! Opt-in development COM registration. TSF profile registration is separate.
use std::ffi::OsString;
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
const MARKER_PREFIX: &str = "sumibi-win-com-dev-v2|";

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
            access | KEY_WOW64_64KEY,
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
            KEY_READ | KEY_WOW64_64KEY,
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
            KEY_READ | KEY_WRITE | KEY_WOW64_64KEY,
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

fn get_optional(key: HKEY, name: Option<&str>) -> Result<Option<String>, Failure> {
    let name = name.map(wide);
    let name = name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()));
    let mut kind = REG_VALUE_TYPE::default();
    let mut length = 0;
    // SAFETY: first call obtains the size; second fills an owned bounded buffer.
    let status =
        unsafe { RegQueryValueExW(key, name, None, Some(&mut kind), None, Some(&mut length)) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    status.ok()?;
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
    Ok(Some(String::from_utf16(&units[..units.len() - 1])?))
}

fn get(key: HKEY, name: Option<&str>) -> Result<String, Failure> {
    get_optional(key, name)?.ok_or_else(|| "required development COM value is absent".into())
}

fn counts(key: HKEY) -> Result<(u32, u32), Failure> {
    let (mut children, mut values) = (0, 0);
    // SAFETY: live query handle and initialized count outputs.
    unsafe {
        RegQueryInfoKeyW(
            key,
            None,
            None,
            None,
            Some(&mut children),
            None,
            None,
            Some(&mut values),
            None,
            None,
            None,
            None,
        )
        .ok()?;
    }
    Ok((children, values))
}

fn server(class: HKEY) -> Result<Key, Failure> {
    let mut key = HKEY::default();
    let name = wide(SERVER_KEY);
    // SAFETY: class handle is open and output slot is writable.
    unsafe {
        RegOpenKeyExW(
            class,
            PCWSTR(name.as_ptr()),
            None,
            KEY_READ | KEY_WOW64_64KEY,
            &mut key,
        )
        .ok()?
    };
    Ok(Key(key))
}

fn complete_path(class: HKEY) -> Result<PathBuf, Failure> {
    if counts(class)? != (1, 1) {
        return Err("unexpected COM class values/children; preserving registration".into());
    }
    let marker = get(class, Some(MARKER))?;
    let expected_path = marker
        .strip_prefix(MARKER_PREFIX)
        .filter(|path| !path.is_empty())
        .ok_or("CLSID is not owned by this development registration version")?;
    let server = server(class)?;
    if counts(server.0)? != (0, 2) {
        return Err("unexpected InprocServer32 values/children; preserving registration".into());
    }
    if get(server.0, Some("ThreadingModel"))? != "Apartment" {
        return Err("unexpected COM threading model; refusing to modify registration".into());
    }
    let path = get(server.0, None)?;
    if path != expected_path {
        return Err("COM server path does not match its ownership marker; preserving it".into());
    }
    Ok(PathBuf::from(path))
}

pub(super) fn registered_path() -> Result<PathBuf, Failure> {
    let class = open_class(KEY_READ)?.ok_or("Sumibi development COM registration is absent")?;
    complete_path(class.0)
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
    let path_text = path.to_str().ok_or("DLL path must be valid Unicode")?;
    if MARKER_PREFIX.encode_utf16().count() + path_text.encode_utf16().count() + 1 > 32768 {
        return Err("DLL path exceeds the bounded development ownership marker".into());
    }
    let (class, fresh) = create_key(HKEY_CURRENT_USER, CLASS_KEY)?;
    if !fresh {
        return Err("CLSID appeared during registration; refusing to overwrite it".into());
    }
    let outcome = (|| {
        put(
            class.0,
            Some(MARKER),
            &format!("{MARKER_PREFIX}{path_text}"),
        )?;
        let (server, fresh) = create_key(class.0, SERVER_KEY)?;
        if !fresh {
            return Err("InprocServer32 appeared during registration".into());
        }
        put(server.0, None, path_text)?;
        put(server.0, Some("ThreadingModel"), "Apartment")?;
        drop(server);
        drop(class);
        verify()?;
        Ok(())
    })();
    if let Err(cause) = outcome {
        // The exact CLSID key was absent before this command. Best-effort
        // rollback leaves an explicit error if cleanup also fails.
        let rollback = remove_fresh(path_text);
        if let Err(error) = rollback {
            return Err(format!("registration failed: {cause}; rollback failed: {error}").into());
        }
        return Err(cause);
    }
    Ok(())
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

fn remove_owned(path: &Path) -> Result<(), Failure> {
    let class = open_class(KEY_READ | KEY_WRITE)?.ok_or("registration is absent")?;
    if complete_path(class.0)? != path {
        return Err("COM registration changed before removal; preserving it".into());
    }
    let name = wide(SERVER_KEY);
    // SAFETY: remove only the exact child key owned by this registration.
    unsafe { RegDeleteKeyExW(class.0, PCWSTR(name.as_ptr()), KEY_WOW64_64KEY.0, None).ok()? };
    drop(class);
    let name = wide(CLASS_KEY);
    // SAFETY: RegDeleteKeyExW refuses to delete a key with unexpected children.
    unsafe {
        RegDeleteKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(name.as_ptr()),
            KEY_WOW64_64KEY.0,
            None,
        )
        .ok()?
    };
    Ok(())
}

fn remove_fresh(path: &str) -> Result<(), Failure> {
    let class = open_class(KEY_READ | KEY_WRITE)?.ok_or("new registration disappeared")?;
    let (children, values) = counts(class.0)?;
    let marker = get_optional(class.0, Some(MARKER))?;
    if children > 1
        || values != u32::from(marker.is_some())
        || marker
            .as_deref()
            .is_some_and(|value| value != format!("{MARKER_PREFIX}{path}"))
    {
        return Err("unexpected fresh COM class content; preserving it".into());
    }
    if children == 1 {
        let server = server(class.0)?; // refuses an unknown child in place of InprocServer32
        let (children, values) = counts(server.0)?;
        let registered = get_optional(server.0, None)?;
        let threading = get_optional(server.0, Some("ThreadingModel"))?;
        if children != 0
            || values != u32::from(registered.is_some()) + u32::from(threading.is_some())
            || registered.as_deref().is_some_and(|value| value != path)
            || threading
                .as_deref()
                .is_some_and(|value| value != "Apartment")
        {
            return Err("unexpected fresh COM server content; preserving it".into());
        }
    }
    let name = wide(SERVER_KEY);
    // SAFETY: root was created by this command; no pre-existing registration
    // can be removed. A missing child is expected after an early failure.
    let child = unsafe { RegDeleteKeyExW(class.0, PCWSTR(name.as_ptr()), KEY_WOW64_64KEY.0, None) };
    if child != ERROR_FILE_NOT_FOUND {
        child.ok()?;
    }
    drop(class);
    let name = wide(CLASS_KEY);
    unsafe {
        RegDeleteKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(name.as_ptr()),
            KEY_WOW64_64KEY.0,
            None,
        )
        .ok()?
    };
    Ok(())
}

pub(super) fn unregister() -> Result<(), Failure> {
    let path = registered_path()?;
    remove_owned(&path)?;
    println!(
        "Current-user development COM registration removed: {}",
        path.display()
    );
    Ok(())
}
