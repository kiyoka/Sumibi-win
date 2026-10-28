//! Disposable HKCU fixture, NOT a COM/TSF backend. No production CLSID/profile,
//! DLL loading, trust store, HKLM or default input settings are touched here.
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use sumibi_setup::install_journal::FileJournal;
use sumibi_setup::install_transaction::{
    self as tx, Backend, Error, InstallSpec, Journal, Observation, Resource, RESOURCES,
};
use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Com::CoCreateGuid;
use windows::Win32::System::Registry::*;

type Failure = Box<dyn std::error::Error>;
const FIXTURE_PARENT: &str = r"Software\Sumibi\RegistrationTransactionFixture";
const INTERRUPTED: i32 = 77;
const TARGET: &str = "sumibi-registration-fixture-x64-v1";

fn err(message: impl std::fmt::Display) -> Error {
    Error(message.to_string())
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: holds only handles returned by a successful open/create.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn open(parent: HKEY, name: &str, access: REG_SAM_FLAGS) -> tx::Result<Option<Key>> {
    let name = wide(name);
    let mut key = HKEY::default();
    // SAFETY: terminated name and initialized output; every access is x64 view.
    let status = unsafe {
        RegOpenKeyExW(
            parent,
            PCWSTR(name.as_ptr()),
            None,
            access | KEY_WOW64_64KEY,
            &mut key,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    status.ok().map_err(err)?;
    Ok(Some(Key(key)))
}

fn create_exclusive(parent: HKEY, name: &str) -> tx::Result<Key> {
    let name = wide(name);
    let mut key = HKEY::default();
    let mut disposition = REG_CREATE_KEY_DISPOSITION::default();
    // SAFETY: valid parent and terminated name, initialized outputs. An existing
    // key is immediately closed and rejected before any value is changed.
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
        .ok()
        .map_err(err)?;
    }
    let key = Key(key);
    if disposition != REG_CREATED_NEW_KEY {
        return Err(err("fixture key already exists"));
    }
    Ok(key)
}

fn put(key: HKEY, name: &str, value: &str) -> tx::Result<()> {
    let name = wide(name);
    let bytes: Vec<u8> = wide(value).into_iter().flat_map(u16::to_le_bytes).collect();
    // SAFETY: live key, terminated name, and byte slice through the call.
    unsafe {
        RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_SZ, Some(&bytes))
            .ok()
            .map_err(err)
    }
}

fn get(key: HKEY, name: &str) -> tx::Result<Option<String>> {
    let name = wide(name);
    let mut kind = REG_VALUE_TYPE::default();
    let mut length = 0;
    // SAFETY: query size into initialized output before allocating bounded data.
    let status = unsafe {
        RegQueryValueExW(
            key,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut kind),
            None,
            Some(&mut length),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    status.ok().map_err(err)?;
    if kind != REG_SZ || !(2..=65536).contains(&length) || length % 2 != 0 {
        return Err(err("unexpected fixture registry value type/size"));
    }
    let mut data = vec![0; length as usize];
    // SAFETY: owned buffer is exactly the size returned above.
    unsafe {
        RegQueryValueExW(
            key,
            PCWSTR(name.as_ptr()),
            None,
            Some(&mut kind),
            Some(data.as_mut_ptr()),
            Some(&mut length),
        )
        .ok()
        .map_err(err)?;
    }
    if kind != REG_SZ || length as usize != data.len() {
        return Err(err("fixture registry value changed during read"));
    }
    let units: Vec<u16> = data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    if units.last() != Some(&0) || units[..units.len() - 1].contains(&0) {
        return Err(err("invalid fixture registry string"));
    }
    Ok(Some(
        String::from_utf16(&units[..units.len() - 1]).map_err(err)?,
    ))
}

fn counts(key: HKEY) -> tx::Result<(u32, u32)> {
    let (mut children, mut values) = (0, 0);
    // SAFETY: live read handle and initialized count outputs, no string buffers.
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
        .ok()
        .map_err(err)?;
    }
    Ok((children, values))
}
fn flush(key: HKEY) -> tx::Result<()> {
    // SAFETY: key remains live. Used only by this disposable crash-test fixture.
    unsafe { RegFlushKey(key).ok().map_err(err) }
}
fn delete(parent: HKEY, name: &str) -> tx::Result<()> {
    let name = wide(name);
    // SAFETY: delete one exact fixture child in the explicit x64 view.
    // This API refuses unexpected subkeys; recursive deletion is never used.
    unsafe {
        RegDeleteKeyExW(parent, PCWSTR(name.as_ptr()), KEY_WOW64_64KEY.0, None)
            .ok()
            .map_err(err)
    }
}
fn resource_name(resource: Resource) -> &'static str {
    match resource {
        Resource::ComServer => "ComServer",
        Resource::LanguageProfile => "LanguageProfile",
        Resource::KeyboardCategory => "KeyboardCategory",
        Resource::DisplayAttributeCategory => "DisplayAttributeCategory",
    }
}
fn spec(id: &str) -> InstallSpec {
    InstallSpec {
        registration_target: TARGET.into(),
        transaction: id.into(),
        dll_path: r"D:\Program Files\Sumibi\fixture-only\sumibi_tip.dll".into(),
        sha256: "1".repeat(64),
        signer_thumbprint: "2".repeat(40),
    }
}
fn root_name(id: &str) -> tx::Result<String> {
    spec(id).validate()?;
    Ok(format!(r"{FIXTURE_PARENT}\{id}"))
}

#[derive(Default, Clone, Copy)]
struct Cut {
    mode: &'static str,
    index: usize,
}
impl Cut {
    fn checkpoint(self, mode: &str, resource: Resource) {
        if self.mode == mode && RESOURCES[self.index] == resource {
            // Intentionally bypass destructors. The next child must reopen both
            // the journal and registry rather than use this process's memory.
            std::process::exit(INTERRUPTED);
        }
    }
}

struct Fixture {
    root: Key,
    cut: Cut,
}
impl Fixture {
    fn open(id: &str, cut: Cut) -> tx::Result<Self> {
        let root = open(HKEY_CURRENT_USER, &root_name(id)?, KEY_READ | KEY_WRITE)?
            .ok_or_else(|| err("fixture root absent"))?;
        let result = Self { root, cut };
        result.check_root(&spec(id))?;
        Ok(result)
    }
    fn check_root(&self, spec: &InstallSpec) -> tx::Result<()> {
        if spec.registration_target != TARGET {
            return Err(err("fixture cannot operate on another registration target"));
        }
        let (children, values) = counts(self.root.0)?;
        if values != 1
            || get(self.root.0, "Transaction")?.as_deref() != Some(&spec.transaction)
            || children > 4
        {
            return Err(err("fixture root ownership mismatch"));
        }
        let mut known = 0;
        for resource in RESOURCES {
            if open(self.root.0, resource_name(resource), KEY_READ)?.is_some() {
                known += 1;
            }
        }
        if known != children {
            return Err(err("unexpected fixture root child"));
        }
        Ok(())
    }
    fn expected(spec: &InstallSpec, resource: Resource) -> [(&'static str, &str); 6] {
        [
            ("Transaction", &spec.transaction),
            ("Kind", resource_name(resource)),
            ("DllPath", &spec.dll_path),
            ("Sha256", &spec.sha256),
            ("SignerThumbprint", &spec.signer_thumbprint),
            ("EnabledByDefault", "0"),
        ]
    }
}
impl Backend for Fixture {
    fn preflight(&mut self, spec: &InstallSpec) -> tx::Result<()> {
        self.check_root(spec)
    }
    fn observe(&mut self, spec: &InstallSpec, resource: Resource) -> tx::Result<Observation> {
        self.check_root(spec)?;
        let Some(key) = open(self.root.0, resource_name(resource), KEY_READ)? else {
            return Ok(Observation::Absent);
        };
        let (children, values) = counts(key.0)?;
        if children != 0 {
            return Ok(Observation::Conflict);
        }
        let mut found = 0;
        for (name, expected) in Self::expected(spec, resource) {
            if let Some(actual) = get(key.0, name)? {
                if actual != expected {
                    return Ok(Observation::Conflict);
                }
                found += 1;
            }
        }
        if found != values {
            return Ok(Observation::Conflict);
        }
        // Root was exclusively created for this fixture; the journal intent
        // identifies the exact child. This is fixture-only ownership evidence.
        Ok(if found == 6 {
            Observation::Owned
        } else {
            Observation::OwnedIncomplete
        })
    }
    fn create(&mut self, spec: &InstallSpec, resource: Resource) -> tx::Result<()> {
        self.check_root(spec)?;
        self.cut.checkpoint("create-before", resource);
        if self.cut.mode == "create-fail" && RESOURCES[self.cut.index] == resource {
            return Err(err("injected fixture create failure"));
        }
        let key = create_exclusive(self.root.0, resource_name(resource))?;
        flush(self.root.0)?;
        self.cut.checkpoint("create-empty", resource);
        for (index, (name, value)) in Self::expected(spec, resource).into_iter().enumerate() {
            put(key.0, name, value)?;
            if index == 0 {
                flush(key.0)?;
                self.cut.checkpoint("create-partial", resource);
            }
        }
        flush(key.0)?;
        self.cut.checkpoint("create-after", resource);
        Ok(())
    }
    fn remove_owned(&mut self, spec: &InstallSpec, resource: Resource) -> tx::Result<()> {
        self.cut.checkpoint("delete-before", resource);
        if !matches!(
            self.observe(spec, resource)?,
            Observation::Owned | Observation::OwnedIncomplete
        ) {
            return Err(err("refusing to delete unowned fixture resource"));
        }
        delete(self.root.0, resource_name(resource))?;
        flush(self.root.0)?;
        self.cut.checkpoint("delete-after", resource);
        Ok(())
    }
}

fn directory(id: &str) -> tx::Result<PathBuf> {
    spec(id).validate()?;
    // Always anchored to our executable, never a caller-supplied registry/file root.
    let exe = std::env::current_exe().map_err(err)?;
    let parent = exe
        .parent()
        .ok_or_else(|| err("executable parent missing"))?;
    Ok(parent.join("registration-transaction-fixture").join(id))
}
fn journal_path(id: &str) -> tx::Result<PathBuf> {
    Ok(directory(id)?.join("journal.bin"))
}
fn mode(value: &str) -> tx::Result<&'static str> {
    match value {
        "none" => Ok("none"),
        "create-before" => Ok("create-before"),
        "create-empty" => Ok("create-empty"),
        "create-partial" => Ok("create-partial"),
        "create-after" => Ok("create-after"),
        "create-fail" => Ok("create-fail"),
        "delete-before" => Ok("delete-before"),
        "delete-after" => Ok("delete-after"),
        _ => Err(err("unknown fixture interruption mode")),
    }
}

pub(super) fn worker(args: &[OsString]) -> Result<(), Failure> {
    if args.len() != 4 {
        return Err("internal fixture worker requires action/id/mode/index".into());
    }
    let action = args[0].to_str().ok_or("invalid action")?;
    let id = args[1].to_str().ok_or("invalid fixture id")?;
    let cut = Cut {
        mode: mode(args[2].to_str().ok_or("invalid mode")?)?,
        index: args[3].to_str().ok_or("invalid index")?.parse()?,
    };
    if cut.index >= RESOURCES.len() {
        return Err("fixture resource index out of bounds".into());
    }
    let identity = spec(id);
    identity.validate()?;
    if action == "init" {
        std::fs::create_dir_all(directory(id)?.parent().ok_or("fixture parent missing")?)?;
        std::fs::create_dir(directory(id)?)?;
        let root = create_exclusive(HKEY_CURRENT_USER, &root_name(id)?)?;
        put(root.0, "Transaction", id)?;
        flush(root.0)?;
        drop(FileJournal::create(&journal_path(id)?)?);
        return Ok(());
    }
    let mut journal = FileJournal::open(&journal_path(id)?)?;
    let mut backend = Fixture::open(id, cut)?;
    match action {
        "install" => tx::install(&mut backend, &mut journal, identity)?,
        "install-failure" => {
            let failure = tx::install(&mut backend, &mut journal, identity)
                .err()
                .ok_or("fixture expected an installation failure")?;
            if !failure.0.contains("rollback complete") {
                return Err(failure.into());
            }
        }
        "recover" => tx::recover(&mut backend, &mut journal)?,
        "uninstall" => tx::uninstall(&mut backend, &mut journal)?,
        "assert-installed" => {
            if journal.latest()?.ok_or("fixture journal absent")?.phase != tx::Phase::Installed {
                return Err("fixture is not committed".into());
            }
            for resource in RESOURCES {
                if backend.observe(&identity, resource)? != Observation::Owned {
                    return Err("fixture resource is not complete".into());
                }
            }
        }
        "assert-removed" => {
            if journal.latest()?.ok_or("fixture journal absent")?.phase != tx::Phase::Removed {
                return Err("fixture cleanup not committed".into());
            }
            for resource in RESOURCES {
                if backend.observe(&identity, resource)? != Observation::Absent {
                    return Err("fixture resource remains".into());
                }
            }
        }
        "assert-conflict-preserved" => {
            if journal.latest()?.ok_or("fixture journal absent")?.phase != tx::Phase::Installed {
                return Err("conflict unexpectedly advanced cleanup journal".into());
            }
            for resource in RESOURCES {
                if open(backend.root.0, resource_name(resource), KEY_READ)?.is_none() {
                    return Err("conflict unexpectedly removed a fixture resource".into());
                }
            }
            if backend.observe(&identity, RESOURCES[cut.index])? != Observation::Conflict {
                return Err("foreign fixture state was not preserved".into());
            }
        }
        "inject-value" | "restore-value" | "inject-child" | "restore-child" | "inject-owner"
        | "restore-owner" | "inject-enabled" | "restore-enabled" => {
            let key = open(
                backend.root.0,
                resource_name(RESOURCES[cut.index]),
                KEY_READ | KEY_WRITE,
            )?
            .ok_or("fixture resource absent")?;
            match action {
                "inject-value" => put(key.0, "ForeignFixtureValue", "preserve-me")?,
                "restore-value" => {
                    if get(key.0, "ForeignFixtureValue")?.as_deref() != Some("preserve-me") {
                        return Err("foreign fixture value was changed".into());
                    }
                    let name = wide("ForeignFixtureValue");
                    // SAFETY: remove only the exact sentinel inserted by this test.
                    unsafe {
                        RegDeleteValueW(key.0, PCWSTR(name.as_ptr())).ok()?;
                    }
                }
                "inject-child" => {
                    drop(create_exclusive(key.0, "ForeignFixtureChild")?);
                }
                "restore-child" => {
                    let child = open(key.0, "ForeignFixtureChild", KEY_READ)?
                        .ok_or("foreign fixture child disappeared")?;
                    if counts(child.0)? != (0, 0) {
                        return Err("foreign fixture child changed".into());
                    }
                    drop(child);
                    delete(key.0, "ForeignFixtureChild")?;
                }
                "inject-owner" => put(key.0, "Transaction", &"f".repeat(32))?,
                "restore-owner" => {
                    if get(key.0, "Transaction")?.as_deref() != Some(&"f".repeat(32)) {
                        return Err("foreign fixture owner was changed".into());
                    }
                    put(key.0, "Transaction", id)?;
                }
                "inject-enabled" => put(key.0, "EnabledByDefault", "1")?,
                "restore-enabled" => {
                    if get(key.0, "EnabledByDefault")?.as_deref() != Some("1") {
                        return Err("foreign fixture profile flag was changed".into());
                    }
                    put(key.0, "EnabledByDefault", "0")?;
                }
                _ => unreachable!(),
            }
            flush(key.0)?;
        }
        "finish" => {
            if counts(backend.root.0)? != (0, 1) {
                return Err("fixture root not empty; preserved for diagnosis".into());
            }
            drop(backend);
            delete(HKEY_CURRENT_USER, &root_name(id)?)?;
            if open(HKEY_CURRENT_USER, &root_name(id)?, KEY_READ)?.is_some() {
                return Err("fixture root remains".into());
            }
            // Preserve the journal as evidence. Only known registry children are removed.
        }
        _ => return Err("unknown fixture action".into()),
    }
    Ok(())
}

fn child(exe: &Path, action: &str, id: &str, cut: Cut, expected: i32) -> Result<(), Failure> {
    let output = Command::new(exe)
        .args([
            "registration-fixture-worker",
            action,
            id,
            cut.mode,
            &cut.index.to_string(),
        ])
        .output()?;
    if output.status.code() != Some(expected) {
        return Err(format!(
            "fixture {action}/{}/{}, exit {:?}, expected {expected}: {}{}; journal: {}",
            cut.mode,
            cut.index,
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
            journal_path(id)?.display()
        )
        .into());
    }
    Ok(())
}

pub(super) fn run() -> Result<(), Failure> {
    let exe = std::env::current_exe()?;
    let none = Cut {
        mode: "none",
        index: 0,
    };
    let mut cases = vec![("normal", none)];
    for name in [
        "create-before",
        "create-empty",
        "create-partial",
        "create-after",
        "create-fail",
        "delete-before",
        "delete-after",
    ] {
        for index in 0..4 {
            cases.push((name, Cut { mode: name, index }));
        }
    }
    for (name, cut) in &cases {
        // SAFETY: CoCreateGuid needs no COM initialization and returns a new GUID.
        let id = format!("{:032x}", unsafe { CoCreateGuid()? }.to_u128());
        child(&exe, "init", &id, none, 0)?;
        if name.starts_with("delete") || *name == "normal" {
            child(&exe, "install", &id, none, 0)?;
            child(&exe, "recover", &id, none, 0)?;
            child(&exe, "assert-installed", &id, none, 0)?;
            child(
                &exe,
                "uninstall",
                &id,
                *cut,
                if *name == "normal" { 0 } else { INTERRUPTED },
            )?;
        } else if *name == "create-fail" {
            child(&exe, "install-failure", &id, *cut, 0)?;
        } else {
            child(&exe, "install", &id, *cut, INTERRUPTED)?;
        }
        child(&exe, "recover", &id, none, 0)?;
        child(&exe, "assert-removed", &id, none, 0)?;
        child(&exe, "uninstall", &id, none, 0)?;
        child(&exe, "finish", &id, none, 0)?;
        println!(
            "Registration transaction fixture / {name}/{}: OK",
            cut.index
        );
    }
    let mut conflicts = 0;
    for kind in ["value", "child", "owner", "enabled"] {
        for index in 0..4 {
            // SAFETY: no apartment is required for generating a new fixture GUID.
            let id = format!("{:032x}", unsafe { CoCreateGuid()? }.to_u128());
            let cut = Cut {
                mode: "none",
                index,
            };
            child(&exe, "init", &id, none, 0)?;
            child(&exe, "install", &id, none, 0)?;
            child(&exe, &format!("inject-{kind}"), &id, cut, 0)?;
            child(&exe, "recover", &id, none, 1)?;
            child(&exe, "uninstall", &id, none, 1)?;
            child(&exe, "assert-conflict-preserved", &id, cut, 0)?;
            child(&exe, &format!("restore-{kind}"), &id, cut, 0)?;
            child(&exe, "assert-installed", &id, none, 0)?;
            child(&exe, "uninstall", &id, none, 0)?;
            child(&exe, "assert-removed", &id, none, 0)?;
            child(&exe, "finish", &id, none, 0)?;
            conflicts += 1;
            println!("Registration transaction fixture / conflict-{kind}/{index}: OK");
        }
    }
    println!(
        "Registration transaction fixture: {} cases passed; HKCU fixture roots removed",
        cases.len() + conflicts
    );
    println!("No COM/TSF registration, HKLM changes, DLL loading or trust changes performed");
    Ok(())
}
