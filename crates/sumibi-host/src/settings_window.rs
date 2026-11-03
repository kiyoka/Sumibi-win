//! Standalone editor for next-start settings and explicit credential writes.
use std::{cell::Cell, path::PathBuf, rc::Rc, sync::Arc};
use sumibi_host::{
    credential_store::CredentialTask,
    settings::{ProtectionProfile, Settings},
    settings_task::{SaveTaskError, SettingsSaveTask},
};
use windows::core::{w, Result as WinResult, PCWSTR};
use windows::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Input::KeyboardAndMouse::EnableWindow, WindowsAndMessaging::*},
};

const BS_AUTORADIOBUTTON: WINDOW_STYLE = WINDOW_STYLE(0x00000009);
const BST_CHECKED: usize = 1;
const BM_GETCHECK: u32 = 0x00F0;
const BM_SETCHECK: u32 = 0x00F1;

#[derive(Default)]
struct Actions {
    save: Cell<bool>,
    key_save: Cell<bool>,
    key_delete: Cell<bool>,
    close: Cell<bool>,
}
struct View {
    hwnd: HWND,
    host: HWND,
    model: HWND,
    path: HWND,
    profile_standard: HWND,
    profile_strict: HWND,
    save: HWND,
    secret: HWND,
    key_save: HWND,
    key_delete: HWND,
    status: HWND,
    actions: Rc<Actions>,
}
impl Drop for View {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    key: WPARAM,
    data: LPARAM,
) -> LRESULT {
    std::panic::catch_unwind(|| unsafe {
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Actions;
        if !pointer.is_null() {
            if message == WM_NCDESTROY {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            if message == WM_CLOSE {
                (*pointer).close.set(true);
                return LRESULT(0);
            }
            if message == WM_COMMAND && key.0 & 0xffff == 1 && key.0 >> 16 == 0 {
                (*pointer).save.set(true);
                return LRESULT(0);
            }
            if message == WM_COMMAND && key.0 & 0xffff == 2 && key.0 >> 16 == 0 {
                (*pointer).close.set(true);
                return LRESULT(0);
            }
            if message == WM_COMMAND && key.0 & 0xffff == 3 && key.0 >> 16 == 0 {
                (*pointer).key_save.set(true);
                return LRESULT(0);
            }
            if message == WM_COMMAND && key.0 & 0xffff == 4 && key.0 >> 16 == 0 {
                (*pointer).key_delete.set(true);
                return LRESULT(0);
            }
        }
        DefWindowProcW(hwnd, message, key, data)
    })
    .unwrap_or(LRESULT(0))
}

fn secret_value(units: &[u16]) -> Result<String, ()> {
    if units.is_empty() || units.len() > 2560 || !units.iter().all(|unit| (33..=126).contains(unit))
    {
        return Err(());
    }
    String::from_utf16(units).map_err(|_| ())
}
fn read_secret(hwnd: HWND) -> Result<String, ()> {
    let mut buffer = [0u16; 2562];
    let count = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    let result = if count < 0 || count as usize >= buffer.len() - 1 {
        Err(())
    } else {
        secret_value(&buffer[..count as usize])
    };
    buffer.fill(0);
    result
}
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
fn set_text(hwnd: HWND, text: &str) {
    let text = wide(text);
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(text.as_ptr()));
    }
}
fn text(hwnd: HWND) -> Result<String, &'static str> {
    let mut buffer = [0u16; 2050];
    let count = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    if count < 0 || count as usize >= buffer.len() - 1 {
        return Err("invalid field");
    }
    String::from_utf16(&buffer[..count as usize]).map_err(|_| "invalid field")
}
impl View {
    fn new(settings: &Settings, visible: bool) -> WinResult<Self> {
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = w!("SumibiHostSettings");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: instance.into(),
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as usize as *mut _),
                lpszClassName: class,
                ..Default::default()
            };
            // One window per command; allow existing class registration across tests.
            if RegisterClassW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                return Err(windows::core::Error::from_thread());
            }
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!("Sumibi 設定"),
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                600,
                485,
                None,
                None,
                Some(instance.into()),
                None,
            )?;
            let actions = Rc::new(Actions::default());
            let mut view = Self {
                hwnd,
                host: HWND::default(),
                model: HWND::default(),
                path: HWND::default(),
                profile_standard: HWND::default(),
                profile_strict: HWND::default(),
                save: HWND::default(),
                secret: HWND::default(),
                key_save: HWND::default(),
                key_delete: HWND::default(),
                status: HWND::default(),
                actions,
            };
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Rc::as_ptr(&view.actions) as isize);
            let child = |kind: PCWSTR,
                         label: &str,
                         style: WINDOW_STYLE,
                         x,
                         y,
                         width,
                         height,
                         id|
             -> WinResult<HWND> {
                let label = wide(label);
                let handle = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    kind,
                    PCWSTR(label.as_ptr()),
                    WS_CHILD | WS_VISIBLE | style,
                    x,
                    y,
                    width,
                    height,
                    Some(hwnd),
                    Some(HMENU(id as *mut _)),
                    Some(instance.into()),
                    None,
                )?;
                SendMessageW(
                    handle,
                    WM_SETFONT,
                    Some(WPARAM(GetStockObject(DEFAULT_GUI_FONT).0 as usize)),
                    Some(LPARAM(1)),
                );
                Ok(handle)
            };
            child(
                w!("STATIC"),
                "保存した設定は次回起動時に使用します。",
                WINDOW_STYLE(0),
                18,
                16,
                550,
                24,
                0,
            )?;
            let mut fields = Vec::new();
            for (i, (label, value)) in [
                ("ホスト名", settings.destination().host()),
                ("モデル", settings.model()),
                ("パス", settings.destination().base_path()),
            ]
            .iter()
            .enumerate()
            {
                let y = 55 + i as i32 * 40;
                child(w!("STATIC"), label, WINDOW_STYLE(0), 18, y + 4, 90, 24, 0)?;
                let edit = child(
                    w!("EDIT"),
                    value,
                    WS_BORDER | WS_TABSTOP | WINDOW_STYLE(0x80),
                    115,
                    y,
                    445,
                    28,
                    10 + i,
                )?;
                SendMessageW(edit, 0x00c5, Some(WPARAM(2048)), Some(LPARAM(0))); // EM_SETLIMITTEXT
                fields.push(edit);
            }
            view.host = fields[0];
            view.model = fields[1];
            view.path = fields[2];

            child(
                w!("STATIC"),
                "保護設定",
                WINDOW_STYLE(0),
                18,
                175 + 2,
                90,
                24,
                0,
            )?;
            view.profile_standard = child(
                w!("BUTTON"),
                "標準 (Standard: Notepad等一般アプリ対応)",
                BS_AUTORADIOBUTTON | WS_GROUP | WS_TABSTOP,
                115,
                175,
                270,
                24,
                21,
            )?;
            view.profile_strict = child(
                w!("BUTTON"),
                "厳格 (Strict: 高機密監査環境)",
                BS_AUTORADIOBUTTON,
                395,
                175,
                165,
                24,
                22,
            )?;

            let (std_check, strict_check) = match settings.protection_profile() {
                ProtectionProfile::Standard => (BST_CHECKED, 0),
                ProtectionProfile::Strict => (0, BST_CHECKED),
            };
            SendMessageW(
                view.profile_standard,
                BM_SETCHECK,
                Some(WPARAM(std_check)),
                None,
            );
            SendMessageW(
                view.profile_strict,
                BM_SETCHECK,
                Some(WPARAM(strict_check)),
                None,
            );

            view.save = child(w!("BUTTON"), "設定を保存", WS_TABSTOP, 430, 215, 130, 30, 1)?;
            child(
                w!("STATIC"),
                "API キー",
                WINDOW_STYLE(0),
                18,
                265,
                90,
                24,
                0,
            )?;
            view.secret = child(
                w!("EDIT"),
                "",
                WS_BORDER | WS_TABSTOP | WINDOW_STYLE(0xa0),
                115,
                260,
                445,
                28,
                13,
            )?; // ES_PASSWORD | ES_AUTOHSCROLL
            SendMessageW(view.secret, 0x00c5, Some(WPARAM(2560)), Some(LPARAM(0)));
            view.key_save = child(w!("BUTTON"), "キーを保存", WS_TABSTOP, 430, 300, 130, 30, 3)?;
            view.key_delete = child(w!("BUTTON"), "キーを削除", WS_TABSTOP, 275, 300, 130, 30, 4)?;
            child(
                w!("STATIC"),
                "この PC の現在ユーザーのキーを置き換えます。",
                WINDOW_STYLE(0),
                18,
                340,
                545,
                24,
                0,
            )?;
            view.status = child(
                w!("STATIC"),
                "キーの保存だけでは送信は許可されません。",
                WINDOW_STYLE(0),
                18,
                380,
                545,
                40,
                0,
            )?;
            if SetTimer(Some(hwnd), 1, 50, None) == 0 {
                return Err(windows::core::Error::from_thread());
            }
            if visible {
                let _ = ShowWindow(hwnd, SW_SHOW);
            }
            Ok(view)
        }
    }
}

#[derive(Clone, Copy)]
enum KeyOperation {
    Save,
    Delete,
}

pub fn run(path: PathBuf) -> Result<(), &'static str> {
    let mut baseline =
        Settings::load(&path).map_err(|_| "settings unavailable; run settings-init first")?;
    let authority = Arc::new(
        baseline
            .new_authority()
            .map_err(|_| "settings unavailable")?,
    );
    let view = View::new(&baseline, true).map_err(|_| "settings window unavailable")?;
    let mut pending: Option<(SettingsSaveTask, Settings)> = None;
    let mut key_pending: Option<(CredentialTask, KeyOperation)> = None;
    let mut settings_conflict = false;
    let mut message = MSG::default();
    loop {
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if result.0 < 0 {
            return Err("settings message loop failed");
        }
        if result.0 == 0 {
            break;
        }
        unsafe {
            if !IsDialogMessageW(view.hwnd, &message).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        if view.actions.save.replace(false) && pending.is_none() && key_pending.is_none() {
            let profile = if unsafe { SendMessageW(view.profile_strict, BM_GETCHECK, None, None).0 }
                == BST_CHECKED as isize
            {
                ProtectionProfile::Strict
            } else {
                ProtectionProfile::Standard
            };
            let settings = text(view.host).and_then(|host| {
                text(view.model).and_then(|model| {
                    text(view.path).and_then(|base| {
                        Settings::new(
                            baseline.destination().scheme(),
                            &host,
                            baseline.destination().port(),
                            &base,
                            &model,
                        )
                        .map(|s| s.with_protection_profile(profile))
                        .map_err(|_| "invalid settings")
                    })
                })
            });
            match settings {
                Ok(replacement) => match SettingsSaveTask::start(
                    authority.clone(),
                    path.clone(),
                    baseline.clone(),
                    replacement.clone(),
                ) {
                    Ok(task) => {
                        pending = Some((task, replacement));
                        unsafe {
                            let _ = EnableWindow(view.save, false);
                        }
                        set_text(view.status, "保存中…");
                    }
                    Err(_) => set_text(view.status, "保存処理を開始できませんでした。"),
                },
                Err(_) => set_text(view.status, "ホスト名・モデル・パスを確認してください。"),
            }
        }
        if view.actions.key_save.replace(false) && pending.is_none() && key_pending.is_none() {
            match read_secret(view.secret) {
                Ok(secret) => match CredentialTask::save(authority.clone(), secret) {
                    Ok(task) => {
                        key_pending = Some((task, KeyOperation::Save));
                        set_text(view.secret, "");
                        set_text(view.status, "キーを保存中…");
                    }
                    Err(_) => set_text(view.status, "キーの保存処理を開始できませんでした。"),
                },
                Err(()) => set_text(
                    view.status,
                    "キーは空白・改行なしの半角文字で入力してください。",
                ),
            }
        }
        if view.actions.key_delete.replace(false) && pending.is_none() && key_pending.is_none() {
            // Default to No. Only this explicit confirmation can start deletion.
            let confirmed = unsafe {
                MessageBoxW(Some(view.hwnd), w!("この PC の現在ユーザーの Sumibi API キーを削除します。\n接続先とモデルの設定は残ります。削除しますか？"), w!("API キーの削除"), MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2)
            } == IDYES;
            if confirmed {
                match CredentialTask::delete(authority.clone()) {
                    Ok(task) => {
                        key_pending = Some((task, KeyOperation::Delete));
                        set_text(view.secret, "");
                        set_text(view.status, "キーを削除中…");
                    }
                    Err(_) => set_text(view.status, "キーの削除処理を開始できませんでした。"),
                }
            }
        }
        if let Some((task, _)) = pending.as_mut() {
            if let Some(result) = task.try_take() {
                let (_, replacement) = pending.take().unwrap();
                let conflict = matches!(
                    result,
                    Err(SaveTaskError::Save(
                        sumibi_host::settings::SettingsError::Conflict
                    ))
                );
                settings_conflict |= conflict;
                match result {
                    Ok(saved) => {
                        // This editor owns a fresh, never-advertised authority
                        // with no Broker or subscribers. The file is for the next launch.
                        let _version = saved.version();
                        baseline = replacement;
                        set_text(view.status, "保存しました。次回起動時に適用されます。");
                    }
                    Err(_) if conflict => set_text(
                        view.status,
                        "別の保存を検出しました。画面を開き直してください。",
                    ),
                    Err(SaveTaskError::WorkerFailed) => set_text(
                        view.status,
                        "保存結果を確認できません。画面を開き直してください。",
                    ),
                    Err(_) => set_text(
                        view.status,
                        "保存できませんでした。設定は変更していません。",
                    ),
                }
            }
        }
        if let Some((task, operation)) = key_pending.as_mut() {
            if let Some(result) = task.try_take() {
                let operation = *operation;
                key_pending = None;
                match (operation, result) {
                    (KeyOperation::Save, Ok(update)) if update.outcome().is_ok() => set_text(
                        view.status,
                        "キーを保存しました。送信への同意は別途必要です。",
                    ),
                    (KeyOperation::Delete, Ok(update)) if update.outcome().is_ok() => {
                        set_text(view.status, "キーを削除しました。設定は残っています。")
                    }
                    (KeyOperation::Save, Ok(_)) => set_text(
                        view.status,
                        "キーを保存できませんでした。もう一度入力してください。",
                    ),
                    (KeyOperation::Delete, Ok(_)) => set_text(
                        view.status,
                        "キーを削除できませんでした。もう一度お試しください。",
                    ),
                    (_, Err(_)) => set_text(view.status, "キー操作の結果を確認できませんでした。"),
                }
            }
        }
        unsafe {
            let idle = pending.is_none() && key_pending.is_none();
            let _ = EnableWindow(view.key_save, idle);
            let _ = EnableWindow(view.key_delete, idle);
            let _ = EnableWindow(view.secret, idle);
            let _ = EnableWindow(view.profile_standard, idle);
            let _ = EnableWindow(view.profile_strict, idle);
            let _ = EnableWindow(
                view.save,
                idle && !settings_conflict && !view.actions.close.get(),
            );
        }
        if view.actions.close.get() && pending.is_none() && key_pending.is_none() {
            break;
        }
        if view.actions.close.get() {
            set_text(view.status, "保存の完了を待ってから閉じます…");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secret_input_rejects_invalid_text_without_accessing_the_store() {
        assert!(secret_value(&[]).is_err());
        for value in ["has space", "line\nbreak", "日本語", "\0"] {
            assert!(secret_value(&value.encode_utf16().collect::<Vec<_>>()).is_err());
        }
        assert!(secret_value(&vec![b'a' as u16; 2561]).is_err());
        assert!(secret_value(&vec![b'a' as u16; 2560]).is_ok());
        assert!(secret_value(&"synthetic-key".encode_utf16().collect::<Vec<_>>()).is_ok());
    }

    #[test]
    fn view_initializes_and_reads_protection_profile_radio_buttons() {
        use sumibi_host::manual::Scheme;

        // 1. Standard profile initialization
        let standard_settings =
            Settings::new(Scheme::Https, "api.example.test", 443, "/v1", "gpt-4o").unwrap();
        assert_eq!(
            standard_settings.protection_profile(),
            ProtectionProfile::Standard
        );

        let view_standard = View::new(&standard_settings, false).expect("create standard view");
        unsafe {
            assert_eq!(
                SendMessageW(view_standard.profile_standard, BM_GETCHECK, None, None).0,
                BST_CHECKED as isize
            );
            assert_eq!(
                SendMessageW(view_standard.profile_strict, BM_GETCHECK, None, None).0,
                0
            );
        }
        drop(view_standard);

        // 2. Strict profile initialization
        let strict_settings = standard_settings
            .clone()
            .with_protection_profile(ProtectionProfile::Strict);
        assert_eq!(
            strict_settings.protection_profile(),
            ProtectionProfile::Strict
        );

        let view_strict = View::new(&strict_settings, false).expect("create strict view");
        unsafe {
            assert_eq!(
                SendMessageW(view_strict.profile_strict, BM_GETCHECK, None, None).0,
                BST_CHECKED as isize
            );
            assert_eq!(
                SendMessageW(view_strict.profile_standard, BM_GETCHECK, None, None).0,
                0
            );

            // 3. Simulate switching from Strict to Standard via radio button check
            let _ = SendMessageW(
                view_strict.profile_standard,
                BM_SETCHECK,
                Some(WPARAM(BST_CHECKED)),
                None,
            );
            let _ = SendMessageW(
                view_strict.profile_strict,
                BM_SETCHECK,
                Some(WPARAM(0)),
                None,
            );

            assert_eq!(
                SendMessageW(view_strict.profile_standard, BM_GETCHECK, None, None).0,
                BST_CHECKED as isize
            );
            assert_eq!(
                SendMessageW(view_strict.profile_strict, BM_GETCHECK, None, None).0,
                0
            );
        }
        drop(view_strict);
    }
}
