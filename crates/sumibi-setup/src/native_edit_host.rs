//! Disposable Windows RichEdit control, without a synthetic text store.
use super::pump_tsf_messages;
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use sumibi_core::{RequestId, Revision};
use sumibi_tip::edit::{input_status_denial, manual::ManualEdit, EditPolicy, InputStatusDenial};
use windows::core::{s, w, Interface, Result, HRESULT, PCWSTR};
use windows::Win32::Foundation::{
    FreeLibrary, E_ABORT, E_ACCESSDENIED, E_NOINTERFACE, HMODULE, HWND, LPARAM, S_OK, WPARAM,
};
use windows::Win32::System::LibraryLoader::{
    GetModuleHandleW, GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
};
use windows::Win32::UI::Controls::RichEdit::*;
use windows::Win32::UI::Controls::{EM_CANUNDO, EM_EMPTYUNDOBUFFER, EM_SETREADONLY};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
use windows::Win32::UI::TextServices::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// The pinned windows crate does not bind inputscope.h's SetInputScope.
// This is its documented system ABI, used only on our same-thread HWND.
fn set_input_scope(hwnd: HWND, scope: InputScope) -> Result<()> {
    unsafe {
        let module = GetModuleHandleW(w!("msctf.dll"))?;
        let address = GetProcAddress(module, s!("SetInputScope"))
            .ok_or(windows::Win32::Foundation::E_NOTIMPL)?;
        let call: unsafe extern "system" fn(HWND, InputScope) -> HRESULT =
            std::mem::transmute(address);
        call(hwnd, scope).ok()
    }
}

struct NativeEdit {
    hwnd: HWND,
    module: HMODULE,
}
impl NativeEdit {
    fn close(mut self) -> Result<()> {
        // Clear the same-thread input scope before destroying the control.
        unsafe {
            set_input_scope(self.hwnd, IS_DEFAULT)?;
            DestroyWindow(self.hwnd)?;
            self.hwnd = HWND::default();
            FreeLibrary(self.module)?;
            self.module = HMODULE::default();
        }
        Ok(())
    }
}
impl Drop for NativeEdit {
    fn drop(&mut self) {
        // SAFETY: this guard owns the child HWND and loader reference on this
        // STA. Never unload its window procedure if destruction fails.
        unsafe {
            if !self.hwnd.is_invalid() {
                let _ = set_input_scope(self.hwnd, IS_DEFAULT);
                if DestroyWindow(self.hwnd).is_err() {
                    eprintln!("retaining RichEdit module: control destruction failed");
                    return;
                }
            }
            if !self.module.is_invalid() {
                let _ = FreeLibrary(self.module);
            }
        }
    }
}

pub(crate) fn exercise_native_edit(
    manager: &ITfThreadMgr,
    client: u32,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let messages: ITfMessagePump = manager.cast()?;
    // The stock RichEdit context reports possible hidden text/regions on the
    // tested Windows. This is a rejection test, never a conversion/Undo claim.
    for (index, case) in [
        "plain-text",
        "rich-text",
        "private",
        "default-scope",
        "password",
        "readonly",
        "readonly-restored",
    ]
    .into_iter()
    .enumerate()
    {
        unsafe {
            let parent = crate::window_host::Window::new(manager)?;
            let module = LoadLibraryExW(w!("Msftedit.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)?;
            let password = if case == "password" { ES_PASSWORD } else { 0 };
            let hwnd = match CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("RICHEDIT50W"),
                w!(""),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE((ES_MULTILINE | password) as u32),
                10,
                10,
                620,
                140,
                Some(parent.hwnd()),
                None,
                None,
                None,
            ) {
                Ok(hwnd) => hwnd,
                Err(error) => {
                    let _ = FreeLibrary(module);
                    return Err(error.into());
                }
            };
            let edit = NativeEdit { hwnd, module };
            if case != "rich-text" {
                assert_eq!(
                    SendMessageW(
                        hwnd,
                        EM_SETTEXTMODE,
                        Some(WPARAM(TM_PLAINTEXT.0 as usize)),
                        Some(LPARAM(0))
                    )
                    .0,
                    0
                );
            }
            let flags = SendMessageW(
                hwnd,
                EM_SETEDITSTYLE,
                Some(WPARAM(SES_USECTF as usize)),
                Some(LPARAM(SES_USECTF as isize)),
            );
            assert_ne!(flags.0 as u32 & SES_USECTF, 0);
            let scope = match case {
                "private" => IS_PRIVATE,
                "default-scope" => IS_DEFAULT,
                _ => IS_TEXT,
            };
            set_input_scope(hwnd, scope)
                .map_err(|e| format!("{case}: native SetInputScope: {e}"))?;
            // All contents are fixed synthetic fixture data, including password.
            let original = "prefix nihongo suffix";
            let text: Vec<u16> = original.encode_utf16().chain(Some(0)).collect();
            SetWindowTextW(hwnd, PCWSTR(text.as_ptr()))?;
            SendMessageW(hwnd, EM_EMPTYUNDOBUFFER, None, None);
            let _ = SetFocus(Some(hwnd));
            pump_tsf_messages(&messages)?;
            assert_eq!(GetFocus(), hwnd);
            // Do not CreateContext, AssociateFocus, supply a TextStore, or
            // initialize compartments: inspect RichEdit's actual native context.
            let document = manager
                .GetFocus()
                .map_err(|e| format!("{case}: native TSF focus: {e}"))?;
            let context = document
                .GetTop()
                .map_err(|e| format!("{case}: native TSF context: {e}"))?;
            let selected = CHARRANGE {
                cpMin: 7,
                cpMax: 14,
            };
            SendMessageW(
                hwnd,
                EM_EXSETSEL,
                None,
                Some(LPARAM((&selected as *const CHARRANGE) as isize)),
            );
            if case.starts_with("readonly") {
                assert_ne!(
                    SendMessageW(hwnd, EM_SETREADONLY, Some(WPARAM(1)), None).0,
                    0
                );
            }
            pump_tsf_messages(&messages)?;
            let status = match context.GetStatus() {
                Err(error) if case == "password" && error.code() == TF_E_EMPTYCONTEXT => None,
                result => Some(result.map_err(|e| format!("{case}: native GetStatus: {e}"))?),
            };
            if case == "password" {
                assert!(
                    status.is_none(),
                    "password: native context contract changed; review compatibility"
                );
            } else {
                let expected = if case.starts_with("readonly") {
                    InputStatusDenial::ReadOnly
                } else {
                    InputStatusDenial::HiddenTextPossible
                };
                assert_eq!(
                    status.and_then(input_status_denial),
                    Some(expected),
                    "{case}: native status contract changed; review compatibility"
                );
            }
            let policy = Rc::new(Cell::new(EditPolicy {
                allowed: true,
                revision: Revision {
                    input: 1,
                    settings: 1,
                    authorization: 1,
                },
            }));
            let before = sumibi_tip::spike::SumibiSpikeBodyReadCount();
            let attempt = Cell::new(index as u128 * 2 + 1);
            let capture_denied = |expected_error: HRESULT| -> Result<()> {
                let id = attempt.get();
                attempt.set(id + 1);
                let ticket = ManualEdit::capture(
                    manager,
                    &context,
                    client,
                    policy.clone(),
                    RequestId(id),
                    id,
                    Instant::now() + Duration::from_secs(5),
                );
                match ticket {
                    Ok(_) => Err(windows::Win32::Foundation::E_UNEXPECTED.into()),
                    Err(error) if error.code() == expected_error => Ok(()),
                    Err(error) => Err(error),
                }
            };
            let capture_error = if case == "password" {
                E_NOINTERFACE
            } else if case.starts_with("readonly") {
                E_ABORT
            } else {
                E_ACCESSDENIED
            };
            capture_denied(capture_error)
                .map_err(|e| format!("{case}: expected protection rejection: {e}"))?;
            if case == "readonly-restored" {
                assert_ne!(
                    SendMessageW(hwnd, EM_SETREADONLY, Some(WPARAM(0)), None).0,
                    0
                );
                pump_tsf_messages(&messages)?;
                assert_eq!(
                    input_status_denial(context.GetStatus()?),
                    Some(InputStatusDenial::HiddenTextPossible)
                );
                capture_denied(E_ACCESSDENIED)?;
            }
            assert_eq!(
                sumibi_tip::spike::SumibiSpikeBodyReadCount(),
                before,
                "{case}: no adapter body read"
            );
            assert_eq!(SendMessageW(hwnd, EM_CANUNDO, None, None).0, 0);
            // The native HWND read is only an assertion on our fixed fixture;
            // it is not part of ManualEdit or the status diagnostic.
            let mut buffer = vec![0u16; 64];
            let len = GetWindowTextW(hwnd, &mut buffer);
            assert_eq!(String::from_utf16(&buffer[..len as usize])?, original);
            drop(context);
            drop(document);
            edit.close()?;
            pump_tsf_messages(&messages)?;
            if let Some(status) = status {
                println!("Native RichEdit / {case}: denied ({:?}, dynamic={:#x}, static={:#x}), adapter body reads=0, native Undo=0: OK",input_status_denial(status),status.dwDynamicFlags,status.dwStaticFlags);
            } else {
                println!("Native RichEdit / {case}: denied (status TF_E_EMPTYCONTEXT, capture E_NOINTERFACE), adapter body reads=0, native Undo=0: OK");
            }
        }
    }
    println!("Native RichEdit: 7 rejection scenarios passed; conversion and standard Undo remain unsupported");
    inspect_standard_edit(manager, &messages)?;
    Ok(())
}

fn inspect_standard_edit(manager: &ITfThreadMgr, messages: &ITfMessagePump) -> Result<()> {
    for (case, scope) in [("default", None), ("text-scope", Some(IS_TEXT))] {
        inspect_standard_edit_case(manager, messages, case, scope)?;
    }
    Ok(())
}

fn inspect_standard_edit_case(
    manager: &ITfThreadMgr,
    messages: &ITfMessagePump,
    case: &str,
    scope: Option<InputScope>,
) -> Result<()> {
    // A stock EDIT child is a separate native target from RichEdit. Only
    // inspect metadata; never infer support from successful window focus.
    unsafe {
        let parent = crate::window_host::Window::new(manager)?;
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("EDIT"),
            w!(""),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(ES_MULTILINE as u32),
            10,
            10,
            620,
            140,
            Some(parent.hwnd()),
            None,
            None,
            None,
        )?;
        struct EditWindow {
            hwnd: HWND,
            scoped: bool,
        }
        impl Drop for EditWindow {
            fn drop(&mut self) {
                unsafe {
                    if self.scoped {
                        let _ = set_input_scope(self.hwnd, IS_DEFAULT);
                    }
                    let _ = DestroyWindow(self.hwnd);
                }
            }
        }
        let mut edit = EditWindow {
            hwnd,
            scoped: false,
        };
        if let Some(scope) = scope {
            set_input_scope(hwnd, scope)?;
            edit.scoped = true;
        }
        SetWindowTextW(hwnd, w!("prefix nihongo suffix"))?;
        SendMessageW(hwnd, EM_EMPTYUNDOBUFFER, None, None);
        let before = sumibi_tip::spike::SumibiSpikeBodyReadCount();
        let _ = SetFocus(Some(hwnd));
        pump_tsf_messages(messages)?;
        assert_eq!(GetFocus(), hwnd);
        // A successful COM call with a null interface pointer is surfaced by
        // the windows crate as Err(S_OK). Distinguish both TSF stages.
        let document = match manager.GetFocus() {
            Ok(document) => Some(document),
            Err(error) if error.code() == S_OK => {
                println!("Native EDIT / {case}: no TSF focused document");
                None
            }
            Err(error) => return Err(error),
        };
        if let Some(document) = document {
            match document.GetTop() {
                Ok(context) => match context.GetStatus() {
                    Ok(status) => {
                        let denial = input_status_denial(status);
                        assert!(
                            denial.is_some(),
                            "standard EDIT status changed; review compatibility before authorizing body access"
                        );
                        println!(
                            "Native EDIT / {case}: denied ({denial:?}, dynamic={:#x}, static={:#x})",
                            status.dwDynamicFlags, status.dwStaticFlags
                        );
                    }
                    Err(error) if error.code() == TF_E_EMPTYCONTEXT => {
                        println!(
                            "Native EDIT / {case}: empty TSF context: {:?}",
                            error.code()
                        )
                    }
                    Err(error) => return Err(error),
                },
                Err(error) if error.code() == S_OK => {
                    println!("Native EDIT / {case}: no TSF top context")
                }
                Err(error) => return Err(error),
            }
        }
        assert_eq!(sumibi_tip::spike::SumibiSpikeBodyReadCount(), before);
        assert_eq!(SendMessageW(hwnd, EM_CANUNDO, None, None).0, 0);
        let mut buffer = vec![0u16; 64];
        let len = GetWindowTextW(hwnd, &mut buffer);
        assert_eq!(
            String::from_utf16(&buffer[..len as usize]).unwrap(),
            "prefix nihongo suffix"
        );
    }
    Ok(())
}
