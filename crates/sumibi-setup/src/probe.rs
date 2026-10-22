//! Process-local COM/TSF registration; never modifies the default input profile.
use std::cell::RefCell;
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;
use std::rc::Rc;
use windows::core::{Interface, GUID, HRESULT, PCWSTR};
use windows::Win32::Foundation::{FreeLibrary, HMODULE, S_OK};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoRegisterClassObject, CoRevokeClassObject, CoUninitialize,
    IClassFactory, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, REGCLS_MULTIPLEUSE,
};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
};
use windows::Win32::UI::Input::KeyboardAndMouse::HKL;
use windows::Win32::UI::TextServices::*;

type Failure = Box<dyn std::error::Error>;
const PROFILE: GUID = GUID::from_u128(0x166150ad_12ef_4f52_a75b_cbb19ff97d27);
const JAPANESE: u16 = 0x0411;
type GetClass = unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut c_void) -> HRESULT;
type CanUnload = unsafe extern "system" fn() -> HRESULT;

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: created only after successful initialization on this thread.
        unsafe { CoUninitialize() };
    }
}

struct Library(HMODULE, bool);
impl Library {
    fn close(mut self) -> windows::core::Result<()> {
        // SAFETY: caller verified DllCanUnloadNow after revoking all registrations.
        unsafe { FreeLibrary(self.0)? };
        self.1 = true;
        Ok(())
    }
}
impl Drop for Library {
    fn drop(&mut self) {
        if self.1 {
            return;
        }
        // SAFETY: export signature is the COM ABI for this build artifact.
        // A failed cleanup must not unload code still referenced by COM.
        // In that case deliberately retain the module until process exit.
        unsafe {
            if let Some(address) = GetProcAddress(self.0, windows::core::s!("DllCanUnloadNow")) {
                let can_unload: CanUnload = std::mem::transmute(address);
                if can_unload() != S_OK {
                    eprintln!("retaining DLL until process exit: live COM objects remain");
                    return;
                }
            }
            let _ = FreeLibrary(self.0);
        }
    }
}

type CleanupLog = Rc<RefCell<Vec<String>>>;

struct ClassRegistration(u32, CleanupLog);
impl Drop for ClassRegistration {
    fn drop(&mut self) {
        // SAFETY: cookie was returned by CoRegisterClassObject in this apartment.
        if let Err(error) = unsafe { CoRevokeClassObject(self.0) } {
            self.1
                .borrow_mut()
                .push(format!("class registration cleanup: {error}"));
        }
    }
}

struct ProfileRegistration(ITfInputProcessorProfileMgr, CleanupLog);
impl Drop for ProfileRegistration {
    fn drop(&mut self) {
        // SAFETY: unregister the exact process-local profile created by this tool.
        if let Err(error) = unsafe {
            self.0.UnregisterProfile(
                &sumibi_tip::CLSID_SUMIBI_TIP,
                JAPANESE,
                &PROFILE,
                TF_URP_LOCALPROCESS,
            )
        } {
            self.1
                .borrow_mut()
                .push(format!("profile cleanup: {error}"));
        }
    }
}

struct ThreadManager(ITfThreadMgr, CleanupLog);
impl Drop for ThreadManager {
    fn drop(&mut self) {
        // SAFETY: matches successful Activate on this same thread.
        if let Err(error) = unsafe { self.0.Deactivate() } {
            self.1
                .borrow_mut()
                .push(format!("thread manager cleanup: {error}"));
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FailAfter {
    None,
    Class,
    Profile,
    Activation,
}

#[derive(Clone, Copy)]
enum Exercise {
    None,
    Edit,
    Key,
    Window,
    Input,
    Close,
    Delayed,
    Broker,
    Ranges,
    BrokerKeys,
    BrokerInput,
    Protection,
    Composition,
    OwnedComposition,
    BrokerDemo,
    TrackingDemo,
    Tracking,
    ComposingDemo,
    Composing,
    Notifications,
    Transactions,
    Backspace,
    Recovery,
    RecoveryDemo,
    TransactionDemo,
    Demo,
}

#[derive(Debug)]
struct Injected;
impl std::fmt::Display for Injected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("intentional rollback test failure")
    }
}
impl std::error::Error for Injected {}

fn checkpoint(point: FailAfter, requested: FailAfter) -> Result<(), Failure> {
    if point == requested {
        Err(Box::new(Injected))
    } else {
        Ok(())
    }
}

pub(super) fn run() -> Result<(), Failure> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "broker-worker-test" {
        crate::worker_probe::run()?;
        return Ok(());
    }
    if args.len() == 1
        && (args[0] == "broker-channel-test" || args[0] == "broker-channel-denied-test")
    {
        crate::channel_probe::run(args[0] == "broker-channel-denied-test")?;
        return Ok(());
    }
    if args.len() == 1 && args[0] == "ipc-peer-test" {
        crate::pipe_peer::run()?;
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "ipc-peer-child") {
        crate::pipe_peer::child(&args[1..])?;
        return Ok(());
    }
    if args.len() == 1 && args[0] == "ipc-test" {
        crate::pipe_probe::run()?;
        return Ok(());
    }
    if args.len() != 3
        || args[1] != "--dll"
        || (args[0] != "plan"
            && args[0] != "probe"
            && args[0] != "self-test"
            && args[0] != "edit-test"
            && args[0] != "key-test"
            && args[0] != "window-test"
            && args[0] != "input-test"
            && args[0] != "close-test"
            && args[0] != "delayed-test"
            && args[0] != "broker-test"
            && args[0] != "range-target-test"
            && args[0] != "broker-key-test"
            && args[0] != "broker-input-test"
            && args[0] != "protection-test"
            && args[0] != "composition-test"
            && args[0] != "owned-composition-test"
            && args[0] != "broker-demo"
            && args[0] != "tracking-demo"
            && args[0] != "tracking-input-test"
            && args[0] != "composing-demo"
            && args[0] != "composing-input-test"
            && args[0] != "input-notify-test"
            && args[0] != "transaction-input-test"
            && args[0] != "backspace-input-test"
            && args[0] != "recovery-input-test"
            && args[0] != "recovery-demo"
            && args[0] != "transaction-demo"
            && args[0] != "demo")
    {
        return Err(
            "usage: sumibi-setup <plan|probe|self-test|edit-test|key-test|window-test|input-test|close-test|delayed-test|broker-test|range-target-test|broker-key-test|broker-input-test|protection-test|composition-test|owned-composition-test|tracking-input-test|tracking-demo|composing-input-test|composing-demo|input-notify-test|transaction-input-test|backspace-input-test|recovery-input-test|recovery-demo|transaction-demo|broker-demo|demo> --dll <sumibi_tip.dll>"
                .into(),
        );
    }
    let path = std::fs::canonicalize(Path::new(&args[2]))?;
    if !path.is_file() {
        return Err("DLL must be a file".into());
    }
    println!("DLL: {}", path.display());
    println!("Scope: this process only; default IME unchanged");
    println!("Steps: load DLL -> register COM factory -> register local TSF profile -> activate -> unregister");
    if args[0] == "plan" {
        return Ok(());
    }

    // SAFETY: dedicated single-threaded console process; guards release COM
    // interfaces before balancing initialization and unloading the test DLL.
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let _apartment = Apartment;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let module = Library(
            LoadLibraryExW(
                PCWSTR(wide.as_ptr()),
                None,
                LOAD_LIBRARY_SEARCH_DEFAULT_DIRS | LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
            )?,
            false,
        );
        let get_class: GetClass = std::mem::transmute(
            GetProcAddress(module.0, windows::core::s!("DllGetClassObject"))
                .ok_or("missing DllGetClassObject")?,
        );
        let can_unload: CanUnload = std::mem::transmute(
            GetProcAddress(module.0, windows::core::s!("DllCanUnloadNow"))
                .ok_or("missing DllCanUnloadNow")?,
        );
        let points = if args[0] == "self-test" {
            &[
                FailAfter::Class,
                FailAfter::Profile,
                FailAfter::Activation,
                FailAfter::None,
            ][..]
        } else {
            &[FailAfter::None][..]
        };
        for point in points {
            let exercise = match args[0].to_str() {
                Some("edit-test") => Exercise::Edit,
                Some("key-test") => Exercise::Key,
                Some("window-test") => Exercise::Window,
                Some("input-test") => Exercise::Input,
                Some("close-test") => Exercise::Close,
                Some("delayed-test") => Exercise::Delayed,
                Some("broker-test") => Exercise::Broker,
                Some("range-target-test") => Exercise::Ranges,
                Some("broker-key-test") => Exercise::BrokerKeys,
                Some("broker-input-test") => Exercise::BrokerInput,
                Some("protection-test") => Exercise::Protection,
                Some("composition-test") => Exercise::Composition,
                Some("owned-composition-test") => Exercise::OwnedComposition,
                Some("broker-demo") => Exercise::BrokerDemo,
                Some("tracking-demo") => Exercise::TrackingDemo,
                Some("tracking-input-test") => Exercise::Tracking,
                Some("composing-demo") => Exercise::ComposingDemo,
                Some("composing-input-test") => Exercise::Composing,
                Some("input-notify-test") => Exercise::Notifications,
                Some("transaction-input-test") => Exercise::Transactions,
                Some("backspace-input-test") => Exercise::Backspace,
                Some("recovery-input-test") => Exercise::Recovery,
                Some("recovery-demo") => Exercise::RecoveryDemo,
                Some("transaction-demo") => Exercise::TransactionDemo,
                Some("demo") => Exercise::Demo,
                _ => Exercise::None,
            };
            let cleanup = CleanupLog::default();
            let result = registered_probe(get_class, *point, &cleanup, exercise);
            if !cleanup.borrow().is_empty() {
                return Err(format!("cleanup failed: {}", cleanup.borrow().join("; ")).into());
            }
            if can_unload() != S_OK {
                return Err(format!("DLL has live objects after {point:?} cleanup").into());
            }
            match result {
                Err(error) if *point != FailAfter::None && error.is::<Injected>() => {
                    println!("Rollback after {point:?}: OK");
                }
                Ok(()) if *point != FailAfter::None => {
                    return Err(format!("rollback checkpoint {point:?} was not reached").into());
                }
                other => other?,
            }
        }
        println!("COM/TSF unregister and DLL unload check: OK");
        module.close()?;
    }
    Ok(())
}

fn registered_probe(
    get_class: GetClass,
    fail_after: FailAfter,
    cleanup: &CleanupLog,
    exercise: Exercise,
) -> Result<(), Failure> {
    // SAFETY: caller keeps the DLL and STA apartment alive for this entire
    // function. All guards are released in reverse registration order.
    unsafe {
        let mut output = ptr::null_mut();
        get_class(
            &sumibi_tip::CLSID_SUMIBI_TIP,
            &IClassFactory::IID,
            &mut output,
        )
        .ok()?;
        let factory = IClassFactory::from_raw(output);
        let class = ClassRegistration(
            CoRegisterClassObject(
                &sumibi_tip::CLSID_SUMIBI_TIP,
                &factory,
                CLSCTX_INPROC_SERVER,
                REGCLS_MULTIPLEUSE,
            )
            .map_err(|e| format!("register local COM class: {e}"))?,
            cleanup.clone(),
        );
        println!("COM factory registered in process");
        checkpoint(FailAfter::Class, fail_after)?;
        let profiles: ITfInputProcessorProfileMgr =
            CoCreateInstance(&CLSID_TF_InputProcessorProfiles, None, CLSCTX_INPROC_SERVER)?;
        let description: Vec<u16> = "Sumibi development probe".encode_utf16().collect();
        profiles
            .RegisterProfile(
                &sumibi_tip::CLSID_SUMIBI_TIP,
                JAPANESE,
                &PROFILE,
                &description,
                &[],
                0,
                HKL::default(),
                0,
                true,
                TF_RP_LOCALPROCESS | TF_RP_HIDDENINSETTINGUI,
            )
            .map_err(|e| format!("register local TSF profile: {e}"))?;
        let profiles = ProfileRegistration(profiles, cleanup.clone());
        println!("TSF profile registered in process");
        checkpoint(FailAfter::Profile, fail_after)?;
        let manager: ITfThreadMgr =
            CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER)?;
        let app_client = manager.Activate()?;
        let manager = ThreadManager(manager, cleanup.clone());
        // Call raw ABI so S_FALSE (profile disabled) cannot masquerade as success.
        let status = (profiles.0.vtable().ActivateProfile)(
            profiles.0.as_raw(),
            TF_PROFILETYPE_INPUTPROCESSOR,
            JAPANESE,
            &sumibi_tip::CLSID_SUMIBI_TIP,
            &PROFILE,
            HKL::default(),
            TF_IPPMF_FORPROCESS,
        );
        if status != S_OK {
            return Err(format!("activate local TSF profile: {status:?}").into());
        }
        println!("TSF ActivateProfile returned S_OK");
        let keys: ITfKeystrokeMgr = manager.0.cast()?;
        let foreground = keys.GetForeground()?;
        if foreground != sumibi_tip::CLSID_SUMIBI_TIP {
            return Err(format!("foreground service is not Sumibi: {foreground:?}").into());
        }
        println!("Real TSF foreground service is Sumibi");
        checkpoint(FailAfter::Activation, fail_after)?;
        match exercise {
            Exercise::None => {}
            Exercise::Edit => crate::edit_host::exercise(&manager.0, app_client)?,
            Exercise::Key => crate::edit_host::exercise_keys(&manager.0, app_client)?,
            Exercise::Window => crate::edit_host::exercise_window_keys(&manager.0, app_client)?,
            Exercise::Input => crate::edit_host::exercise_input(&manager.0, app_client)?,
            Exercise::Close => crate::edit_host::exercise_close(&manager.0, app_client)?,
            Exercise::Delayed => crate::edit_host::exercise_delayed(&manager.0, app_client)?,
            Exercise::Broker => crate::edit_host::exercise_broker(&manager.0, app_client)?,
            Exercise::Ranges => crate::edit_host::exercise_ranges(&manager.0, app_client)?,
            Exercise::BrokerKeys => crate::edit_host::exercise_broker_keys(&manager.0, app_client)?,
            Exercise::BrokerInput => {
                crate::edit_host::exercise_broker_input(&manager.0, app_client)?
            }
            Exercise::BrokerDemo => crate::edit_host::run_broker_demo(&manager.0, app_client)?,
            Exercise::TrackingDemo => crate::edit_host::run_tracking_demo(&manager.0, app_client)?,
            Exercise::Tracking => crate::edit_host::exercise_tracking(&manager.0, app_client)?,
            Exercise::ComposingDemo => {
                crate::edit_host::run_composing_demo(&manager.0, app_client)?
            }
            Exercise::Composing => crate::edit_host::exercise_composing(&manager.0, app_client)?,
            Exercise::Transactions => {
                crate::edit_host::exercise_transactions(&manager.0, app_client)?
            }
            Exercise::Backspace => crate::edit_host::exercise_backspace(&manager.0, app_client)?,
            Exercise::Recovery => crate::edit_host::exercise_recovery(&manager.0, app_client)?,
            Exercise::RecoveryDemo => crate::edit_host::run_recovery_demo(&manager.0, app_client)?,
            Exercise::TransactionDemo => {
                crate::edit_host::run_transaction_demo(&manager.0, app_client)?
            }
            Exercise::Notifications => {
                crate::edit_host::exercise_notifications(&manager.0, app_client)?
            }
            Exercise::Protection => crate::edit_host::exercise_protection(&manager.0, app_client)?,
            Exercise::Composition => {
                crate::edit_host::exercise_composition(&manager.0, app_client)?
            }
            Exercise::OwnedComposition => {
                crate::edit_host::exercise_owned_composition(&manager.0, app_client)?
            }
            Exercise::Demo => crate::edit_host::run_demo(&manager.0, app_client)?,
        }
        drop(keys);
        drop(manager);
        drop(profiles);
        drop(class);
        drop(factory);
    }
    Ok(())
}
