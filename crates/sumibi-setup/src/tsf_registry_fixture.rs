//! Real TSF API experiment isolated by process-local registry remapping and MIC.
use super::registration_backend::{self as backend, Cut, Layout, RedirectedBackend};
use super::registration_native::{Apartment, Factories};
use super::registration_registry::{self as reg, Failure, Key};
use std::ffi::OsString;
use sumibi_setup::install_journal::FileJournal;
use sumibi_setup::install_transaction::{
    self as tx, Backend, Journal, Observation, Phase, Resource, RESOURCES,
};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_ACCESS_DENIED, ERROR_NO_TOKEN, HANDLE, HLOCAL,
};
use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows::Win32::Security::*;
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken,
};

const PARENT: &str = r"Software\Sumibi\NativeRegistrationFixture";
fn name(id: &str) -> Result<String, Failure> {
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) || id != id.to_ascii_lowercase()
    {
        return Err("fixture ID must be 32 hex digits".into());
    }
    Ok(format!(r"{PARENT}\{id}"))
}
fn low_label(key: HKEY) -> Result<(), Failure> {
    let sddl = reg::wide("S:(ML;CI;NW;;;LW)");
    let mut sd = PSECURITY_DESCRIPTOR::default();
    // SAFETY: system allocates the descriptor; apply a low mandatory label only
    // to this newly created owned fixture key, then release the descriptor.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            1,
            &mut sd,
            None,
        )?;
        let result = RegSetKeySecurity(key, LABEL_SECURITY_INFORMATION, sd).ok();
        let _ = LocalFree(Some(HLOCAL(sd.0)));
        result?;
    }
    Ok(())
}
fn token_info(token: HANDLE, kind: TOKEN_INFORMATION_CLASS) -> Result<Vec<usize>, Failure> {
    let mut length = 0;
    // SAFETY: first query size, then fill an aligned bounded buffer.
    unsafe {
        let _ = GetTokenInformation(token, kind, None, 0, &mut length);
        if length == 0 || length > 65536 {
            return Err("unsupported fixture token information".into());
        }
        let mut bytes = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
        GetTokenInformation(
            token,
            kind,
            Some(bytes.as_mut_ptr().cast()),
            length,
            &mut length,
        )?;
        Ok(bytes)
    }
}
fn require_low() -> Result<(), Failure> {
    let mut thread_token = HANDLE::default();
    // SAFETY: reject impersonation before examining the primary-token boundary.
    match unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut thread_token) } {
        Err(error) if error.code() == windows::core::HRESULT::from_win32(ERROR_NO_TOKEN.0) => {}
        Ok(()) => {
            let _ = unsafe { CloseHandle(thread_token) };
            return Err("native fixture rejects impersonating threads".into());
        }
        Err(error) => return Err(error.into()),
    }
    let mut token = HANDLE::default();
    // SAFETY: read current primary token only, close it on every result path.
    unsafe {
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
    }
    let result = (|| {
        let info = token_info(token, TokenIntegrityLevel)?;
        let policy = token_info(token, TokenMandatoryPolicy)?;
        let app_container = token_info(token, TokenIsAppContainer)?;
        unsafe {
            let sid = (*info.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()).Label.Sid;
            if !IsValidSid(sid).as_bool() {
                return Err("invalid fixture token SID".into());
            }
            let count = *GetSidSubAuthorityCount(sid);
            if count == 0
                || *app_container.as_ptr().cast::<u32>() != 0
                || *GetSidSubAuthority(sid, u32::from(count - 1)) != 0x1000
                || (*policy.as_ptr().cast::<TOKEN_MANDATORY_POLICY>()).Policy.0
                    & TOKEN_MANDATORY_POLICY_NO_WRITE_UP.0
                    == 0
            {
                return Err(
                    "real TSF registry fixture requires low integrity with NoWriteUp".into(),
                );
            }
        }
        for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            let mut key = HKEY::default();
            let path = reg::wide("Software");
            // SAFETY: an access probe, not a create/write. Require OS denial of
            // write access to the real hives before obtaining COM factories.
            let status = unsafe {
                RegOpenKeyExW(
                    root,
                    PCWSTR(path.as_ptr()),
                    None,
                    KEY_WRITE | KEY_WOW64_64KEY,
                    &mut key,
                )
            };
            if status != ERROR_ACCESS_DENIED {
                if status.is_ok() {
                    drop(Key(key));
                }
                return Err("real registry write protection could not be established".into());
            }
        }
        Ok(())
    })();
    let _ = unsafe { CloseHandle(token) };
    result
}
struct Redirect(Vec<HKEY>);
fn mutate(
    hives: &[Key; 3],
    layout: &Layout,
    action: &str,
    kind: &str,
    index: usize,
) -> Result<(), Failure> {
    let path = match index {
        0 => layout.com.clone(),
        1 => layout.language.clone(),
        _ => layout.categories[index - 2].0.clone(),
    };
    let (hive, path) = match kind {
        "user" => (1, String::new()),
        "classes" => (2, String::new()),
        _ => (0, path),
    };
    let target = reg::open(hives[hive].0, &path, KEY_READ | KEY_WRITE)?
        .ok_or("native mutation target absent")?;
    let restore = action == "restore";
    let (name, injected, original) = match kind {
        "value" | "user" | "classes" => (
            "unknownfixturevalue",
            reg::string("native-fixture-sentinel"),
            None,
        ),
        "owner" if index == 0 => (
            "sumibiregistrationowner",
            reg::string("foreign-fixture-owner"),
            layout
                .expected
                .at(&layout.com)
                .and_then(|tree| tree.values.get("sumibiregistrationowner"))
                .cloned(),
        ),
        "enabled" if index == 1 => ("enable", reg::dword(1), Some(reg::dword(0))),
        "child" | "second-profile" => {
            let (parent, leaf) = if kind == "second-profile" {
                let parent = layout
                    .language
                    .rsplit_once('\\')
                    .ok_or("language parent missing")?
                    .0;
                (
                    reg::open(hives[0].0, parent, KEY_READ | KEY_WRITE)?
                        .ok_or("language parent absent")?,
                    format!(
                        "{{{:?}}}",
                        windows::core::GUID::from_u128(layout.profile.to_u128() ^ 2)
                    ),
                )
            } else {
                (target, "UnknownFixtureChild".into())
            };
            if restore {
                let child =
                    reg::open(parent.0, &leaf, KEY_READ)?.ok_or("fixture child disappeared")?;
                if reg::snapshot(child.0)? != reg::Tree::default() {
                    return Err("fixture child changed; preserving".into());
                }
                drop(child);
                reg::delete_key(parent.0, &leaf)?;
            } else {
                drop(reg::create_new(parent.0, &leaf)?);
            }
            return reg::flush(hives[hive].0);
        }
        _ => return Err("unknown native fixture mutation".into()),
    };
    let current = reg::snapshot(target.0)?;
    if restore {
        if current.values.get(name) != Some(&injected) {
            return Err("fixture sentinel changed; preserving".into());
        }
        if let Some(value) = original {
            reg::put_value(target.0, name, &value)?;
        } else {
            reg::delete_value(target.0, name)?;
        }
    } else {
        if current.values.get(name) != original.as_ref() {
            return Err("fixture original value mismatch".into());
        }
        reg::put_value(target.0, name, &injected)?;
    }
    reg::flush(target.0)
}
impl Redirect {
    fn new(roots: &[Key; 3]) -> Result<Self, Failure> {
        let mut result = Self(vec![]);
        for (predefined, target) in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER, HKEY_CLASSES_ROOT]
            .into_iter()
            .zip(roots)
        {
            let classes_alias = if predefined == HKEY_CLASSES_ROOT {
                reg::open(roots[0].0, r"Software\Classes", KEY_READ | KEY_WRITE)?
            } else {
                None
            };
            let target = classes_alias.as_ref().unwrap_or(target);
            // SAFETY: worker-only process, no other threads do registry I/O.
            unsafe {
                RegOverridePredefKey(predefined, Some(target.0)).ok()?;
            }
            result.0.push(predefined);
        }
        Ok(result)
    }
}
impl Drop for Redirect {
    fn drop(&mut self) {
        for root in self.0.iter().rev() {
            // SAFETY: undo each successfully installed process-local mapping.
            let _ = unsafe { RegOverridePredefKey(*root, None) };
        }
    }
}
pub(super) fn run(args: &[OsString]) -> Result<(), Failure> {
    if std::mem::size_of::<usize>() != 8 {
        return Err("native registration fixture requires Windows x64".into());
    }
    if args.len() != 2 && args.len() != 4 {
        return Err(
            "usage: sumibi-setup tsf-registry-fixture <action> <32-hex-id> [stage index]".into(),
        );
    }
    let action = args[0].to_str().ok_or("invalid action")?;
    if ![
        "init",
        "inspect",
        "finish",
        "assert-real-absent",
        "install",
        "recover",
        "uninstall",
        "assert-installed",
        "assert-removed",
        "inject",
        "restore",
        "drop-enable",
        "drop-alias",
        "prune-empty",
    ]
    .contains(&action)
    {
        return Err("unknown native fixture action".into());
    }
    let id = args[1].to_str().ok_or("invalid ID")?;
    let path = name(id)?;
    let identity = backend::spec(id);
    let layout = Layout::new(&identity)?;
    let check_real = || -> Result<(), Failure> {
        for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            for path in [&layout.com, &layout.service] {
                if reg::open(root, path, KEY_READ)?.is_some() {
                    return Err("fixture GUID exists outside redirected registry".into());
                }
            }
        }
        Ok(())
    };
    if action == "assert-real-absent" {
        check_real()?;
        return Ok(());
    }
    if action == "init" {
        check_real()?;
        let root = reg::create_new(HKEY_CURRENT_USER, &path)?;
        reg::put_string(root.0, "Transaction", id)?;
        for label in ["Machine", "User", "Classes"] {
            let child = reg::create_new(root.0, label)?;
            low_label(child.0)?;
        }
        low_label(root.0)?;
        println!("Native TSF fixture initialized: {id}");
        return Ok(());
    }
    let root =
        reg::open(HKEY_CURRENT_USER, &path, KEY_READ)?.ok_or("native fixture root missing")?;
    let state = reg::snapshot(root.0)?;
    if state.values.len() != 1
        || state.values.get("transaction") != Some(&reg::string(id))
        || state.children.len() != 3
        || ["machine", "user", "classes"]
            .iter()
            .any(|name| !state.children.contains_key(*name))
    {
        return Err("native fixture root ownership mismatch".into());
    }
    if action == "inspect" {
        println!("{}", serde_json::to_string(&state)?);
        return Ok(());
    }
    if action == "finish" {
        if state
            .children
            .values()
            .any(|tree| *tree != reg::Tree::default())
        {
            return Err("native fixture not empty; preserved".into());
        }
        for child in ["Machine", "User", "Classes"] {
            reg::delete_key(root.0, child)?;
        }
        drop(root);
        reg::delete_key(HKEY_CURRENT_USER, &path)?;
        check_real()?;
        return Ok(());
    }
    require_low()?;
    check_real()?;
    let roots: [Result<Key, Failure>; 3] = ["Machine", "User", "Classes"].map(|label| {
        reg::open(root.0, label, KEY_READ | KEY_WRITE)?
            .ok_or_else(|| "native fixture hive missing".into())
    });
    let [machine, user, classes] = roots;
    let roots = [machine?, user?, classes?];
    let stage = if args.len() == 4 {
        args[2].to_str().ok_or("invalid interruption stage")?
    } else {
        "none"
    };
    if !["inject", "restore"].contains(&action)
        && ![
            "none",
            "create-before",
            "create-after",
            "create-fail",
            "com-empty",
            "com-partial",
            "delete-before",
            "delete-after-api",
            "delete-after",
            "delete-during-prune",
        ]
        .contains(&stage)
    {
        return Err("unknown native fixture interruption stage".into());
    }
    let index = if args.len() == 4 {
        args[3]
            .to_str()
            .ok_or("invalid interruption index")?
            .parse::<usize>()?
    } else {
        0
    };
    if index >= RESOURCES.len() {
        return Err("native fixture interruption index out of bounds".into());
    }
    if ["inject", "restore"].contains(&action) {
        return mutate(&roots, &layout, action, stage, index);
    }
    if action == "drop-enable" {
        let key = reg::open(roots[0].0, &layout.language, KEY_READ | KEY_WRITE)?
            .ok_or("partial profile absent")?;
        if reg::snapshot(key.0)?
            != *layout
                .expected
                .at(&layout.language)
                .ok_or("profile model missing")?
        {
            return Err("partial fixture profile changed".into());
        }
        reg::delete_value(key.0, "Enable")?;
        return reg::flush(key.0);
    }
    if action == "drop-alias" {
        if index < 2 {
            return Err("partial alias requires a category".into());
        }
        let path = &layout.categories[index - 2].1;
        let key = reg::open(roots[0].0, path, KEY_READ)?.ok_or("partial category alias absent")?;
        if reg::snapshot(key.0)? != reg::Tree::default() {
            return Err("partial category alias changed".into());
        }
        drop(key);
        reg::delete_key(roots[0].0, path)?;
        return reg::flush(roots[0].0);
    }
    let _apartment = Apartment::new()?;
    let _redirect = Redirect::new(&roots)?;
    let factories = Factories::new()?;
    let native = factories.instantiate()?;
    drop(factories);
    let mut backend = RedirectedBackend {
        hives: &roots,
        native: &native,
        cut: Cut { stage, index },
    };
    if action == "prune-empty" {
        backend.prune_empty(&identity, Resource::ComServer)?;
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let journal_path = exe
        .parent()
        .ok_or("native worker directory missing")?
        .join(format!("journal-{id}.bin"));
    let mut journal = if action == "install" {
        FileJournal::create(&journal_path)?
    } else {
        FileJournal::open(&journal_path)?
    };
    match action {
        "install" => tx::install(&mut backend, &mut journal, identity)?,
        "recover" => tx::recover(&mut backend, &mut journal)?,
        "uninstall" => tx::uninstall(&mut backend, &mut journal)?,
        "assert-installed" => {
            if journal
                .latest()?
                .ok_or("native fixture journal missing")?
                .phase
                != Phase::Installed
            {
                return Err("native fixture is not committed".into());
            }
            for resource in RESOURCES {
                if backend.observe(&identity, resource)? != Observation::Owned {
                    return Err("native fixture registration incomplete".into());
                }
            }
        }
        "assert-removed" => {
            if journal
                .latest()?
                .ok_or("native fixture journal missing")?
                .phase
                != Phase::Removed
            {
                return Err("native fixture cleanup not committed".into());
            }
            for hive in &roots {
                if reg::snapshot(hive.0)? != reg::Tree::default() {
                    return Err("native fixture left registry shells".into());
                }
            }
        }
        _ => return Err("unknown native TSF fixture action".into()),
    }
    Ok(())
}
