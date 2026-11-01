//! Explicit consent UI. Only nonsecret offers cross the native UI boundary.
use std::{cell::Cell, path::PathBuf, rc::Rc};
use sumibi_host::{
    consent_task::ConsentOffer,
    host_control::{ControlError, HostStatus, Phase},
    host_service::HostService,
    manual::Scheme,
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
    grant: Cell<bool>,
    revoke: Cell<bool>,
    close: Cell<bool>,
}
struct View {
    hwnd: HWND,
    details: HWND,
    status: HWND,
    grant: HWND,
    revoke: HWND,
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
            if message == WM_COMMAND && key.0 >> 16 == 0 {
                match key.0 & 0xffff {
                    1 => (*pointer).grant.set(true),
                    2 => (*pointer).revoke.set(true),
                    3 => (*pointer).close.set(true),
                    _ => return DefWindowProcW(hwnd, message, key, data),
                }
                return LRESULT(0);
            }
        }
        DefWindowProcW(hwnd, message, key, data)
    })
    .unwrap_or(LRESULT(0))
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn set_text(hwnd: HWND, value: &str) {
    let value = wide(value);
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(value.as_ptr()));
    }
}
fn endpoint(offer: &ConsentOffer) -> String {
    let destination = offer.destination();
    let scheme = if destination.scheme() == Scheme::Https {
        "https"
    } else {
        "http"
    };
    let host = if destination.host().contains(':') {
        format!("[{}]", destination.host())
    } else {
        destination.host().to_owned()
    };
    format!(
        "{scheme}://{host}:{}{}",
        destination.port(),
        destination.base_path()
    )
}
fn description(offer: &ConsentOffer) -> String {
    format!(
        "送信先：{}\r\nモデル：{}\r\nAPI キー：{}",
        endpoint(offer),
        offer.model().unwrap_or("未設定"),
        if offer.credential_present() {
            "登録済み（表示しません）"
        } else {
            "未登録・読取不可"
        }
    )
}
fn status_text(status: &HostStatus) -> &'static str {
    match status.phase {
        Phase::Starting => "設定と資格情報を確認しています…",
        Phase::Updating => "同意の変更と通知を待っています…",
        Phase::Stopping => "送信を停止し、処理の終了を待っています…",
        Phase::Stopped => "Host を終了しました。",
        Phase::Failed if status.error == Some(ControlError::AlreadyRunning) => {
            "Host の起動枠が使用中です。既存の送信管理画面を確認してください。"
        }
        Phase::Failed => "Host を起動・継続できませんでした。設定を確認してください。",
        Phase::Ready => match status.error {
            Some(ControlError::StaleOffer) => {
                "表示後に設定が変わりました。内容を確認して再度操作してください。"
            }
            Some(_) => "同意を変更できませんでした。内容を確認して再度操作してください。",
            None if status.credential_error => {
                "キーを読み取れません。設定画面で確認し、Host を開き直してください。"
            }
            None if status.offer.as_ref().is_some_and(ConsentOffer::allowed) => {
                "この Host の起動中は、手動変換の送信を許可しています。"
            }
            None if status
                .offer
                .as_ref()
                .is_some_and(|offer| !offer.credential_present()) =>
            {
                "キーが未登録です。設定画面で登録し、Host を開き直してください。"
            }
            None if status.offer.is_none() => "処理中です。しばらくお待ちください…",
            None => "手動変換の送信は許可されていません。",
        },
    }
}
impl View {
    fn new() -> WinResult<Self> {
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = w!("SumibiHostConsent");
            if RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(procedure),
                hInstance: instance.into(),
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as usize as *mut _),
                lpszClassName: class,
                ..Default::default()
            }) == 0
            {
                return Err(windows::core::Error::from_thread());
            }
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!("Sumibi Host — 送信管理"),
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                660,
                390,
                None,
                None,
                Some(instance.into()),
                None,
            )?;
            let actions = Rc::new(Actions::default());
            let mut view = Self {
                hwnd,
                details: HWND::default(),
                status: HWND::default(),
                grant: HWND::default(),
                revoke: HWND::default(),
                actions,
            };
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Rc::as_ptr(&view.actions) as isize);
            let child = |kind, label: &str, style, x, y, width, height, id| -> WinResult<HWND> {
                let label = wide(label);
                let control = CreateWindowExW(
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
                    control,
                    WM_SETFONT,
                    Some(WPARAM(GetStockObject(DEFAULT_GUI_FONT).0 as usize)),
                    Some(LPARAM(1)),
                );
                Ok(control)
            };
            child(w!("STATIC"), "この起動中だけ、手動変換の送信を許可できます。\r\n入力アプリとの接続は準備中です。", WINDOW_STYLE(0), 18, 16, 605, 43, 0)?;
            view.details = child(
                w!("EDIT"),
                "設定を読み込んでいます…",
                WS_BORDER | WS_VSCROLL | WS_TABSTOP | WINDOW_STYLE(0x0844),
                18,
                70,
                605,
                125,
                10,
            )?; // READONLY | MULTILINE | AUTOVSCROLL
            view.status = child(
                w!("STATIC"),
                "設定と資格情報を確認しています…",
                WINDOW_STYLE(0),
                18,
                210,
                605,
                48,
                0,
            )?;
            view.grant = child(w!("BUTTON"), "送信を許可…", WS_TABSTOP, 18, 280, 170, 32, 1)?;
            view.revoke = child(
                w!("BUTTON"),
                "許可を取り消す",
                WS_TABSTOP,
                205,
                280,
                170,
                32,
                2,
            )?;
            child(
                w!("BUTTON"),
                "Host を終了",
                WS_TABSTOP,
                453,
                280,
                170,
                32,
                3,
            )?;
            let _ = EnableWindow(view.grant, false);
            let _ = EnableWindow(view.revoke, false);
            if SetTimer(Some(hwnd), 1, 50, None) == 0 {
                return Err(windows::core::Error::from_thread());
            }
            let _ = ShowWindow(hwnd, SW_SHOW);
            Ok(view)
        }
    }
}

pub fn run(path: PathBuf) -> Result<(), &'static str> {
    // Construct the native view first, so a UI creation failure cannot leave
    // an unpolled Host running while main exits the process.
    let view = View::new().map_err(|_| "host window unavailable")?;
    let mut service = HostService::start(path).map_err(|_| "host service unavailable")?;
    let mut displayed: Option<ConsentOffer> = None;
    let mut finished = None;
    let mut pump = true;
    let mut loop_error = false;
    loop {
        if pump {
            let mut message = MSG::default();
            let result = unsafe { GetMessageW(&mut message, None, 0, 0) };
            if result.0 <= 0 {
                loop_error = result.0 < 0;
                pump = false;
                view.actions.close.set(true);
            } else {
                unsafe {
                    if !IsDialogMessageW(view.hwnd, &message).as_bool() {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
            }
        } else {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if view.actions.close.get() {
            service.control().request_close();
        }
        if !view.actions.close.get() && finished.is_none() {
            if view.actions.grant.replace(false) {
                if let Some(offer) = displayed
                    .take()
                    .filter(|offer| offer.can_grant() && !offer.allowed())
                {
                    let prompt = wide(&format!("手動変換の入力本文と API キーを、次の接続先へ送信することを許可しますか？\n\n{}\nモデル：{}\n\nこの Host を終了すると許可を取り消します。", endpoint(&offer), offer.model().unwrap_or("未設定")));
                    let confirmed = unsafe {
                        MessageBoxW(
                            Some(view.hwnd),
                            PCWSTR(prompt.as_ptr()),
                            w!("手動変換の送信許可"),
                            MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2,
                        )
                    } == IDYES;
                    if confirmed {
                        let _ = service.control().try_grant(offer);
                    }
                }
            }
            if view.actions.revoke.replace(false) {
                let _ = service.control().try_revoke();
            }
        }
        if let Some(result) = service.try_finish() {
            finished = Some(result);
        }
        match service.try_status() {
            Ok(Some(status)) => {
                set_text(view.status, status_text(&status));
                if status.phase == Phase::Failed {
                    set_text(
                        view.details,
                        "Host を利用できません。下の案内を確認してください。",
                    );
                }
                let ready = status.phase == Phase::Ready
                    && !service.control().is_updating()
                    && !view.actions.close.get()
                    && finished.is_none();
                let offer = status.offer.filter(|_| ready);
                if let Some(offer) = &offer {
                    if displayed
                        .as_ref()
                        .is_none_or(|old| old.version() != offer.version())
                    {
                        set_text(view.details, &description(offer));
                    }
                }
                unsafe {
                    let _ = EnableWindow(
                        view.grant,
                        offer
                            .as_ref()
                            .is_some_and(|offer| offer.can_grant() && !offer.allowed()),
                    );
                    let _ = EnableWindow(
                        view.revoke,
                        offer.as_ref().is_some_and(ConsentOffer::allowed),
                    );
                }
                displayed = offer;
            }
            Ok(None) => {
                unsafe {
                    let _ = EnableWindow(view.grant, false);
                    let _ = EnableWindow(view.revoke, false);
                }
                displayed = None;
            }
            Err(_) => {
                loop_error = true;
                view.actions.close.set(true);
                service.control().request_close();
            }
        }
        if view.actions.close.get() {
            set_text(view.status, "送信を停止し、処理の終了を待っています…");
            if let Some(result) = finished {
                let _ = service.stop();
                return if loop_error || result.is_err() {
                    Err("host stopped with an error")
                } else {
                    Ok(())
                };
            }
        }
    }
}
