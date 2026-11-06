//! Standalone editor for next-start settings and explicit credential writes.
use std::{cell::Cell, path::PathBuf, rc::Rc, sync::Arc};
use sumibi_host::{
    credential_store::CredentialTask,
    provider::Provider,
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
    provider: Cell<bool>,
    key_save: Cell<bool>,
    key_delete: Cell<bool>,
    close: Cell<bool>,
}
struct View {
    hwnd: HWND,
    provider: HWND,
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
    key_hint: HWND,
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
            if message == WM_COMMAND
                && key.0 & 0xffff == 30
                && key.0 >> 16 == CBN_SELCHANGE as usize
            {
                (*pointer).provider.set(true);
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
    fn update_key_labels(&self) {
        let host = text(self.host).unwrap_or_default().to_ascii_lowercase();
        let (button, hint) = match host.as_str() {
            "generativelanguage.googleapis.com" => (
                "Gemini API キーをこのPCに保存",
                "保存した API キーは、Gemini での変換に使用します。",
            ),
            "api.openai.com" => (
                "OpenAI API キーをこのPCに保存",
                "保存した API キーは、OpenAI での変換に使用します。",
            ),
            _ => (
                "API キーをこのPCに保存",
                "保存した API キーは、この接続先での変換に使用します。",
            ),
        };
        for (control, label) in [(self.key_save, button), (self.key_hint, hint)] {
            if text(control).as_deref() != Ok(label) {
                set_text(control, label);
            }
        }
    }
    fn scheme_and_port(&self, baseline: &Settings) -> (sumibi_host::manual::Scheme, u16) {
        let selection = unsafe { SendMessageW(self.provider, CB_GETCURSEL, None, None).0 };
        if selection == 0 || selection == 1 {
            (sumibi_host::manual::Scheme::Https, 443)
        } else {
            (
                baseline.destination().scheme(),
                baseline.destination().port(),
            )
        }
    }
    fn matches_destination(&self, saved: &Settings) -> bool {
        let (scheme, port) = self.scheme_and_port(saved);
        let Ok(host) = text(self.host) else {
            return false;
        };
        let Ok(path) = text(self.path) else {
            return false;
        };
        sumibi_host::manual::Destination::for_chat_completions(scheme, &host, port, &path)
            .is_some_and(|destination| destination == *saved.destination())
    }
    fn new(settings: Option<&Settings>, visible: bool) -> WinResult<Self> {
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
                535,
                None,
                None,
                Some(instance.into()),
                None,
            )?;
            let actions = Rc::new(Actions::default());
            let mut view = Self {
                hwnd,
                provider: HWND::default(),
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
                key_hint: HWND::default(),
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
            child(
                w!("STATIC"),
                "プロバイダー",
                WINDOW_STYLE(0),
                18,
                59,
                90,
                24,
                0,
            )?;
            view.provider = child(
                w!("COMBOBOX"),
                "",
                WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(3),
                115,
                55,
                445,
                160,
                30,
            )?;
            for label in ["OpenAI", "Google Gemini", "その他（互換 API）"] {
                let label = wide(label);
                SendMessageW(
                    view.provider,
                    CB_ADDSTRING,
                    None,
                    Some(LPARAM(label.as_ptr() as isize)),
                );
            }
            let selected = match settings.map(|s| Provider::from_destination(s.destination())) {
                Some(Provider::OpenAi) => 0,
                Some(Provider::Google) => 1,
                _ => 2,
            };
            SendMessageW(view.provider, CB_SETCURSEL, Some(WPARAM(selected)), None);
            let mut fields = Vec::new();
            for (i, (label, value)) in [
                (
                    "ホスト名",
                    settings.map(|s| s.destination().host()).unwrap_or(""),
                ),
                ("モデル", settings.map(|s| s.model()).unwrap_or("")),
                (
                    "パス",
                    settings
                        .map(|s| s.destination().base_path())
                        .unwrap_or("/v1"),
                ),
            ]
            .iter()
            .enumerate()
            {
                let y = 105 + i as i32 * 40;
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
                225 + 2,
                90,
                24,
                0,
            )?;
            view.profile_standard = child(
                w!("BUTTON"),
                "標準 (Standard: Notepad等一般アプリ対応)",
                BS_AUTORADIOBUTTON | WS_GROUP | WS_TABSTOP,
                115,
                225,
                270,
                24,
                21,
            )?;
            view.profile_strict = child(
                w!("BUTTON"),
                "厳格 (Strict: 高機密監査環境)",
                BS_AUTORADIOBUTTON,
                395,
                225,
                165,
                24,
                22,
            )?;

            let (std_check, strict_check) = match settings
                .map(Settings::protection_profile)
                .unwrap_or(ProtectionProfile::Standard)
            {
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

            view.save = child(
                w!("BUTTON"),
                "接続先とモデルを保存",
                WS_TABSTOP,
                320,
                265,
                240,
                30,
                1,
            )?;
            child(
                w!("STATIC"),
                "API キー",
                WINDOW_STYLE(0),
                18,
                315,
                90,
                24,
                0,
            )?;
            view.secret = child(
                w!("EDIT"),
                "",
                WS_BORDER | WS_TABSTOP | WINDOW_STYLE(0xa0),
                115,
                310,
                445,
                28,
                13,
            )?; // ES_PASSWORD | ES_AUTOHSCROLL
            SendMessageW(view.secret, 0x00c5, Some(WPARAM(2560)), Some(LPARAM(0)));
            view.key_save = child(
                w!("BUTTON"),
                "API キーをこのPCに保存",
                WS_TABSTOP,
                250,
                350,
                310,
                30,
                3,
            )?;
            view.key_delete = child(
                w!("BUTTON"),
                "API キーを削除",
                WS_TABSTOP,
                115,
                350,
                120,
                30,
                4,
            )?;
            view.key_hint = child(
                w!("STATIC"),
                "保存した API キーは、この接続先での変換に使用します。",
                WINDOW_STYLE(0),
                18,
                390,
                545,
                24,
                0,
            )?;
            view.status = child(
                w!("STATIC"),
                "キーの保存だけでは送信は許可されません。",
                WINDOW_STYLE(0),
                18,
                430,
                545,
                40,
                0,
            )?;
            view.update_key_labels();
            if settings.is_none() {
                set_text(
                    view.status,
                    "ホスト名・モデル・パスを入力し、先に「接続先とモデルを保存」を押してください。",
                );
                let _ = EnableWindow(view.key_save, false);
                let _ = EnableWindow(view.key_delete, false);
                let _ = EnableWindow(view.secret, false);
            }
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
    let mut baseline = load_for_editor(&path)?;
    if baseline.is_some() {
        Settings::upgrade_credentials(&path)
            .map_err(|_| "credential migration failed; settings not changed")?;
        baseline = load_for_editor(&path)?;
    }
    // In-memory placeholder only, never displayed or saved. The first save
    // replaces it with user-entered settings; key controls stay disabled until then.
    let draft = Settings::new(
        sumibi_host::manual::Scheme::Https,
        "setup.invalid",
        443,
        "/v1",
        "configure-model",
    )
    .map_err(|_| "settings unavailable")?;
    let authority = Arc::new(
        baseline
            .as_ref()
            .unwrap_or(&draft)
            .new_authority()
            .map_err(|_| "settings unavailable")?,
    );
    let view = View::new(baseline.as_ref(), true).map_err(|_| "settings window unavailable")?;
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
        if view.actions.provider.replace(false) && pending.is_none() && key_pending.is_none() {
            let selection = unsafe { SendMessageW(view.provider, CB_GETCURSEL, None, None).0 };
            let provider = match selection {
                0 => Provider::OpenAi,
                1 => Provider::Google,
                _ => Provider::Other,
            };
            if let Some((host, path, model)) = provider.preset() {
                set_text(view.host, host);
                set_text(view.path, path);
                set_text(view.model, model);
            }
            set_text(view.secret, "");
            set_text(
                view.status,
                "先に「接続先とモデルを保存」を押すと、API キーを入力できます。",
            );
        }
        view.update_key_labels();
        let key_destination_saved = baseline
            .as_ref()
            .is_some_and(|saved| view.matches_destination(saved));
        if !key_destination_saved {
            set_text(view.secret, "");
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
                            view.scheme_and_port(baseline.as_ref().unwrap_or(&draft)).0,
                            &host,
                            view.scheme_and_port(baseline.as_ref().unwrap_or(&draft)).1,
                            &base,
                            &model,
                        )
                        .map(|s| s.with_protection_profile(profile))
                        .map_err(|_| "invalid settings")
                    })
                })
            });
            match settings {
                Ok(replacement) => match match baseline.clone() {
                    Some(expected) => SettingsSaveTask::start(
                        authority.clone(),
                        path.clone(),
                        expected,
                        replacement.clone(),
                    ),
                    None => SettingsSaveTask::create(
                        authority.clone(),
                        path.clone(),
                        replacement.clone(),
                    ),
                } {
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
        if view.actions.key_save.replace(false)
            && pending.is_none()
            && key_pending.is_none()
            && key_destination_saved
            && !settings_conflict
        {
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
        if view.actions.key_delete.replace(false)
            && pending.is_none()
            && key_pending.is_none()
            && key_destination_saved
            && !settings_conflict
        {
            // Default to No. Only this explicit confirmation can start deletion.
            let confirmed = unsafe {
                MessageBoxW(Some(view.hwnd), w!("保存済みの接続先の Sumibi API キーを削除します。\n接続先とモデルの設定は残ります。削除しますか？"), w!("API キーの削除"), MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2)
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
                            | sumibi_host::settings::SettingsError::AlreadyExists
                    ))
                );
                settings_conflict |= conflict;
                match result {
                    Ok(saved) => {
                        // This editor owns a fresh, never-advertised authority
                        // with no Broker or subscribers. The file is for the next launch.
                        let _version = saved.version();
                        baseline = Some(replacement);
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
            let _ = EnableWindow(
                view.key_save,
                idle && key_destination_saved && !settings_conflict,
            );
            let _ = EnableWindow(
                view.key_delete,
                idle && key_destination_saved && !settings_conflict,
            );
            let _ = EnableWindow(
                view.secret,
                idle && key_destination_saved && !settings_conflict,
            );
            for field in [view.provider, view.host, view.path, view.model] {
                let _ = EnableWindow(field, idle);
            }
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

fn load_for_editor(path: &std::path::Path) -> Result<Option<Settings>, &'static str> {
    if !path.is_absolute() {
        return Err("settings path must be absolute");
    }
    match path.try_exists() {
        Ok(false) => Ok(None),
        Ok(true) => Settings::load(path)
            .map(Some)
            .map_err(|_| "settings unavailable; existing file was not changed"),
        Err(_) => Err("settings unavailable"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gemini_view_restores_provider_and_disables_key_binding_for_unsaved_host() {
        let settings = Settings::new(
            sumibi_host::manual::Scheme::Https,
            "generativelanguage.googleapis.com",
            443,
            "/v1beta/openai",
            "custom-gemini",
        )
        .unwrap();
        let view = View::new(Some(&settings), false).unwrap();
        assert_eq!(
            unsafe { SendMessageW(view.provider, CB_GETCURSEL, None, None).0 },
            1
        );
        assert_eq!(text(view.model).unwrap(), "custom-gemini");
        assert!(view.matches_destination(&settings));
        set_text(view.path, "/v1beta/openai/");
        assert!(view.matches_destination(&settings));
        set_text(view.host, "other.test");
        assert!(!view.matches_destination(&settings));
    }
    #[test]
    fn first_run_view_has_empty_destination_and_disabled_key_controls() {
        use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
        let view = View::new(None, false).expect("create first-run view");
        assert_eq!(text(view.host).unwrap(), "");
        assert_eq!(text(view.model).unwrap(), "");
        assert_eq!(text(view.path).unwrap(), "/v1");
        unsafe {
            assert!(!IsWindowEnabled(view.key_save).as_bool());
            assert!(!IsWindowEnabled(view.key_delete).as_bool());
            assert!(!IsWindowEnabled(view.secret).as_bool());
        }
    }

    #[test]
    fn missing_settings_open_first_run_but_corrupt_settings_are_preserved() {
        let dir = std::env::temp_dir().join(format!("sumibi-first-run-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("settings.json");
        assert!(load_for_editor(&path).unwrap().is_none());
        assert!(!path.exists());
        std::fs::write(&path, b"broken fixture").unwrap();
        assert!(load_for_editor(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"broken fixture");
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

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

        let view_standard =
            View::new(Some(&standard_settings), false).expect("create standard view");
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

        let view_strict = View::new(Some(&strict_settings), false).expect("create strict view");
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
