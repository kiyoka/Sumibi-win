//! Standalone editor for next-start settings. No credential or network access.
use std::{cell::Cell, path::PathBuf, rc::Rc, sync::Arc};
use sumibi_host::{
    settings::Settings,
    settings_task::{SaveTaskError, SettingsSaveTask},
};
use windows::core::{w, Result as WinResult, PCWSTR};
use windows::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Input::KeyboardAndMouse::EnableWindow, WindowsAndMessaging::*},
};

#[derive(Default)]
struct Actions {
    save: Cell<bool>,
    close: Cell<bool>,
}
struct View {
    hwnd: HWND,
    host: HWND,
    model: HWND,
    path: HWND,
    save: HWND,
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
        }
        DefWindowProcW(hwnd, message, key, data)
    })
    .unwrap_or(LRESULT(0))
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
            // One window per command; test processes also construct one view.
            if RegisterClassW(&wc) == 0 {
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
                325,
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
                save: HWND::default(),
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
            view.save = child(w!("BUTTON"), "保存", WS_TABSTOP, 460, 180, 100, 30, 1)?;
            view.status = child(
                w!("STATIC"),
                "送信への同意と API キーの登録は別の操作です。",
                WINDOW_STYLE(0),
                18,
                230,
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
        if view.actions.save.replace(false) && pending.is_none() {
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
        if let Some((task, _)) = pending.as_mut() {
            if let Some(result) = task.try_take() {
                let (_, replacement) = pending.take().unwrap();
                let conflict = matches!(
                    result,
                    Err(SaveTaskError::Save(
                        sumibi_host::settings::SettingsError::Conflict
                    ))
                );
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
                unsafe {
                    let _ = EnableWindow(view.save, !conflict);
                }
            }
        }
        if view.actions.close.get() && pending.is_none() {
            break;
        }
        if view.actions.close.get() {
            set_text(view.status, "保存の完了を待ってから閉じます…");
        }
    }
    Ok(())
}
