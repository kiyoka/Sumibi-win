//! System tray notification icon for Sumibi Host.
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, POINT, WPARAM};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, LoadIconW, PostMessageW,
    SetForegroundWindow, TrackPopupMenu, IDI_APPLICATION, MF_SEPARATOR, MF_STRING, TPM_BOTTOMALIGN,
    TPM_RIGHTBUTTON, WM_APP, WM_COMMAND, WM_CONTEXTMENU, WM_LBUTTONDBLCLK, WM_LBUTTONUP,
    WM_RBUTTONUP,
};

pub const WM_SUMIBI_TRAY: u32 = WM_APP + 101;

pub const TRAY_CMD_OPEN: usize = 2001;
pub const TRAY_CMD_SETTINGS: usize = 2002;
pub const TRAY_CMD_EXIT: usize = 2003;

pub struct TrayIcon {
    _icon: Option<crate::app_icon::Icon>,
    hwnd: HWND,
    added: bool,
}

impl TrayIcon {
    pub fn new(hwnd: HWND, instance: HINSTANCE) -> Self {
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_SUMIBI_TRAY,
            ..Default::default()
        };

        let icon = crate::app_icon::small(instance);
        if let Some(handle) = icon
            .as_ref()
            .map(|icon| icon.0)
            .or_else(|| unsafe { LoadIconW(None, IDI_APPLICATION).ok() })
        {
            nid.hIcon = handle;
        }

        let tip = "Sumibi Host (待受中)";
        let tip_wide: Vec<u16> = tip.encode_utf16().collect();
        let len = tip_wide.len().min(nid.szTip.len() - 1);
        nid.szTip[..len].copy_from_slice(&tip_wide[..len]);
        nid.szTip[len] = 0;

        let added = unsafe { Shell_NotifyIconW(NIM_ADD, &nid).as_bool() };
        Self {
            _icon: icon,
            hwnd,
            added,
        }
    }

    pub fn handle_message(&self, lparam: LPARAM) -> bool {
        let msg = lparam.0 as u32;
        match msg {
            WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
                unsafe {
                    let _ = PostMessageW(
                        Some(self.hwnd),
                        WM_COMMAND,
                        WPARAM(TRAY_CMD_OPEN),
                        LPARAM(0),
                    );
                }
                true
            }
            WM_RBUTTONUP | WM_CONTEXTMENU => {
                self.show_context_menu();
                true
            }
            _ => false,
        }
    }

    fn show_context_menu(&self) {
        unsafe {
            let menu = match CreatePopupMenu() {
                Ok(m) => m,
                Err(_) => return,
            };

            let _ = AppendMenuW(
                menu,
                MF_STRING,
                TRAY_CMD_OPEN,
                w!("送信管理画面を開く (&O)"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                TRAY_CMD_SETTINGS,
                w!("設定画面を開く (&S)"),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, TRAY_CMD_EXIT, w!("Host を終了 (&X)"));

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);

            let _ = SetForegroundWindow(self.hwnd);
            let _ = TrackPopupMenu(
                menu,
                TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
                pt.x,
                pt.y,
                None,
                self.hwnd,
                None,
            );
            let _ = DestroyMenu(menu);
        }
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        if self.added {
            let nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: 1,
                ..Default::default()
            };
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_constants_are_consistent() {
        assert_eq!(WM_SUMIBI_TRAY, WM_APP + 101);
        assert_ne!(TRAY_CMD_OPEN, TRAY_CMD_SETTINGS);
        assert_ne!(TRAY_CMD_SETTINGS, TRAY_CMD_EXIT);
        assert_ne!(TRAY_CMD_OPEN, TRAY_CMD_EXIT);
    }

    #[test]
    fn tray_handle_message_ignores_unrelated() {
        let tray = TrayIcon {
            _icon: None,
            hwnd: HWND::default(),
            added: false,
        };
        // Unrelated mouse moves should be ignored
        assert!(!tray.handle_message(LPARAM(0)));
    }
}
