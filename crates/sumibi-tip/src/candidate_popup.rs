//! Non-activating candidate presentation. The TSF key sink handles selection.
use crate::candidate_session::CandidateSession;
use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, GetStockObject, MonitorFromPoint, DEFAULT_GUI_FONT, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::*;

pub(crate) struct CandidatePopup(HWND);

impl CandidatePopup {
    pub(crate) fn new(bounds: Option<RECT>, candidates: &CandidateSession) -> Result<Self> {
        let mut point = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut point);
        }
        if let Some(bounds) = bounds {
            point = POINT {
                x: bounds.left,
                y: bounds.bottom,
            };
        }
        let width = 400;
        let height = 38 + candidates.len() as i32 * 18;
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        unsafe {
            let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
            if GetMonitorInfoW(monitor, &mut info).as_bool() {
                point.x = point.x.clamp(
                    info.rcWork.left,
                    (info.rcWork.right - width).max(info.rcWork.left),
                );
                point.y = point.y.clamp(
                    info.rcWork.top,
                    (info.rcWork.bottom - height).max(info.rcWork.top),
                );
            }
            let hwnd = CreateWindowExW(
                WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_TRANSPARENT,
                w!("STATIC"),
                w!(""),
                WS_POPUP | WS_BORDER | WINDOW_STYLE(0x80),
                point.x,
                point.y,
                width,
                height,
                None,
                None,
                None,
                None,
            )?;
            let popup = Self(hwnd);
            SendMessageW(
                hwnd,
                WM_SETFONT,
                Some(WPARAM(GetStockObject(DEFAULT_GUI_FONT).0 as usize)),
                Some(LPARAM(1)),
            );
            popup.render(candidates);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            Ok(popup)
        }
    }

    pub(crate) fn render(&self, candidates: &CandidateSession) {
        use std::fmt::Write as _;
        let mut text = String::from("変換候補  ↑↓・Enter / 数字 / Esc\r\n");
        for (index, item) in candidates.candidates.iter().enumerate() {
            let marker = if index == candidates.selected_index {
                "▶"
            } else {
                "　"
            };
            let number = match index {
                0..=8 => (index + 1).to_string(),
                9 => "0".into(),
                _ => " ".into(),
            };
            let mut visible: String = item
                .text
                .chars()
                .take(24)
                .map(|ch| if ch.is_control() { ' ' } else { ch })
                .collect();
            if item.text.chars().nth(24).is_some() {
                visible.push('…');
            }
            let _ = write!(
                text,
                "{marker} {number}  {visible} [{}]\r\n",
                item.source.label()
            );
        }
        let text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let _ = SetWindowTextW(self.0, PCWSTR(text.as_ptr()));
        }
    }
}

impl Drop for CandidatePopup {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}
