//! Explicit rescue UI. Callbacks enqueue scalar actions only; the owner loop
//! checks ticket/lifetime before any copy. No TSF or editor references here.
use std::{cell::Cell, rc::Rc, sync::OnceLock};
use sumibi_core::recovery::{Metadata, Operation, Outcome, Ticket};
use windows::core::{w, Error, Result, HRESULT, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{GetStockObject, HBRUSH, WHITE_BRUSH};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::*;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Copy,
    Discard,
    Hide,
}
struct State {
    action: Cell<Option<Action>>,
    copy: Cell<HWND>,
    discard: Cell<HWND>,
}
pub(crate) struct View {
    hwnd: HWND,
    label: HWND,
    preview: HWND,
    pub(crate) ticket: Ticket,
    pub(crate) copyable: bool,
    state: Rc<State>,
}
static CLASS: OnceLock<std::result::Result<(), HRESULT>> = OnceLock::new();
// Winuser.h STATIC styles (SystemServices constants in windows-rs).
const STATIC_NOTIFY: WINDOW_STYLE = WINDOW_STYLE(256);
const STATIC_NOPREFIX: WINDOW_STYLE = WINDOW_STYLE(128);
const CLASS_NAME: PCWSTR = w!("SumibiInputRecovery");
unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    key: WPARAM,
    data: LPARAM,
) -> LRESULT {
    std::panic::catch_unwind(|| unsafe {
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const State;
        if !pointer.is_null() {
            Rc::increment_strong_count(pointer);
            let state = Rc::from_raw(pointer);
            if message == WM_NCDESTROY {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            if message == WM_MOUSEACTIVATE {
                return LRESULT(MA_NOACTIVATE as isize);
            }
            if message == WM_CLOSE {
                state.action.set(Some(Action::Hide));
                let _ = ShowWindow(hwnd, SW_HIDE);
                return LRESULT(0);
            }
            if message == WM_COMMAND && key.0 >> 16 == 0 {
                let child = HWND(data.0 as *mut _);
                let action = match key.0 & 0xffff {
                    1 if child == state.copy.get() => Some(Action::Copy),
                    2 if child == state.discard.get() => Some(Action::Discard),
                    _ => None,
                };
                if let Some(action) = action {
                    state.action.set(Some(action));
                    return LRESULT(0);
                }
            }
        }
        DefWindowProcW(hwnd, message, key, data)
    })
    .unwrap_or(LRESULT(0))
}
impl View {
    pub(crate) fn new(owner: HWND, metadata: Metadata, units: &[u16]) -> Result<Self> {
        unsafe {
            let instance = GetModuleHandleW(None)?;
            (*CLASS.get_or_init(|| {
                let class = WNDCLASSW {
                    lpfnWndProc: Some(procedure),
                    hInstance: instance.into(),
                    hCursor: LoadCursorW(None, IDC_ARROW).map_err(|e| e.code())?,
                    hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0),
                    lpszClassName: CLASS_NAME,
                    ..Default::default()
                };
                if RegisterClassW(&class) == 0 {
                    Err(Error::from_thread().code())
                } else {
                    Ok(())
                }
            }))
            .map_err(Error::from)?;
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                CLASS_NAME,
                w!("Sumibi 未確認入力の確認"),
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
                140,
                140,
                670,
                340,
                Some(owner),
                None,
                Some(instance.into()),
                None,
            )?;
            let state = Rc::new(State {
                action: Cell::new(None),
                copy: Cell::new(HWND::default()),
                discard: Cell::new(HWND::default()),
            });
            let mut view = Self {
                hwnd,
                label: HWND::default(),
                preview: HWND::default(),
                ticket: metadata.ticket,
                copyable: metadata.copyable,
                state,
            };
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Rc::as_ptr(&view.state) as isize);
            view.label = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                None,
                WS_CHILD | WS_VISIBLE,
                12,
                12,
                630,
                68,
                Some(hwnd),
                None,
                Some(instance.into()),
                None,
            )?;
            view.preview = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                None,
                WS_CHILD | WS_VISIBLE | WS_BORDER | STATIC_NOPREFIX,
                12,
                84,
                630,
                155,
                Some(hwnd),
                None,
                Some(instance.into()),
                None,
            )?;
            view.state.copy.set(CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("入力の全文をコピーして閉じる (F10)"),
                WS_CHILD | WS_VISIBLE | WS_BORDER | STATIC_NOTIFY | STATIC_NOPREFIX,
                12,
                254,
                300,
                32,
                Some(hwnd),
                Some(HMENU(std::ptr::without_provenance_mut(1))),
                Some(instance.into()),
                None,
            )?);
            view.state.discard.set(CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("救済データを破棄 (F9)"),
                WS_CHILD | WS_VISIBLE | WS_BORDER | STATIC_NOTIFY | STATIC_NOPREFIX,
                330,
                254,
                250,
                32,
                Some(hwnd),
                Some(HMENU(std::ptr::without_provenance_mut(2))),
                Some(instance.into()),
                None,
            )?);
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(
                view.state.copy.get(),
                metadata.copyable,
            );
            let preview = if metadata.copyable {
                // Static preview has no native clipboard/edit operation. The
                // explicit button copies the full bounded input, never a field.
                let mut end = units.len().min(160);
                if end < units.len() && end > 0 && (0xd800..=0xdbff).contains(&units[end - 1]) {
                    end -= 1;
                }
                let text =
                    String::from_utf16(&units[..end]).map_err(|_| Error::from(E_INVALIDARG))?;
                if end < units.len() {
                    format!("{text}\r\n…（長い入力のため一部を省略しています。コピーは全文です）")
                } else {
                    text
                }
            } else {
                "救済本文は保存していません。文書の結果を確認してください。".into()
            };
            set_text(view.preview, &preview)?;
            view.update(metadata, false)?;
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            Ok(view)
        }
    }
    pub(crate) fn update(&self, metadata: Metadata, failed: bool) -> Result<()> {
        let operation = match metadata.operation {
            Operation::Insert(_) => "文字入力".into(),
            Operation::Backspace(count) => format!("Backspace × {count}"),
            Operation::DeleteSelection => "選択範囲の削除".into(),
        };
        let result = if metadata.outcome == Outcome::AppliedUntracked {
            "本文操作は成功。追跡を停止しました"
        } else {
            "本文操作の結果は不明です"
        };
        let remaining = metadata.remaining.as_secs().max(1);
        set_text(self.label, &format!("{operation}：{result}。\r\n本文への自動適用は行いません。残り約{remaining}秒で破棄します。\r\n×は保持して閉じます。{}", if failed { " コピー失敗。保持しています。" } else { "" }))
    }
    pub(crate) fn take_action(&self) -> Option<(Ticket, Action)> {
        self.state.action.take().map(|a| (self.ticket, a))
    }
    pub(crate) fn click_copy(&self) {
        unsafe {
            SendMessageW(self.state.copy.get(), WM_LBUTTONDOWN, None, None);
            SendMessageW(self.state.copy.get(), WM_LBUTTONUP, None, None);
        }
    }
    pub(crate) fn click_discard(&self) {
        unsafe {
            SendMessageW(self.state.discard.get(), WM_LBUTTONDOWN, None, None);
            SendMessageW(self.state.discard.get(), WM_LBUTTONUP, None, None);
        }
    }
    pub(crate) fn close_request(&self) {
        unsafe {
            SendMessageW(self.hwnd, WM_CLOSE, None, None);
        }
    }
}
impl Drop for View {
    fn drop(&mut self) {
        unsafe {
            let _ = SetWindowTextW(self.preview, w!(""));
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
fn set_text(hwnd: HWND, text: &str) -> Result<()> {
    let text: Vec<_> = text.encode_utf16().chain(Some(0)).collect();
    unsafe { SetWindowTextW(hwnd, PCWSTR(text.as_ptr())) }
}
pub(crate) trait Clipboard {
    fn write(&self, owner: HWND, text: &[u16], permitted: &dyn Fn() -> bool) -> Result<()>;
}
pub(crate) struct SystemClipboard;
struct ClipboardGuard;
impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}
struct Memory(Option<HGLOBAL>);
impl Drop for Memory {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            unsafe {
                let _ = GlobalFree(Some(handle));
            }
        }
    }
}
impl Clipboard for SystemClipboard {
    fn write(&self, owner: HWND, text: &[u16], permitted: &dyn Fn() -> bool) -> Result<()> {
        if text.is_empty() || text.len() > sumibi_core::recovery::MAX_UNITS {
            return Err(E_INVALIDARG.into());
        }
        unsafe {
            let mut memory = Memory(Some(GlobalAlloc(GMEM_MOVEABLE, (text.len() + 1) * 2)?));
            let handle = memory.0.ok_or(E_FAIL)?;
            let pointer = GlobalLock(handle).cast::<u16>();
            if pointer.is_null() {
                return Err(Error::from_thread());
            }
            std::ptr::copy_nonoverlapping(text.as_ptr(), pointer, text.len());
            pointer.add(text.len()).write(0);
            // A zero lock count can report false without an error. We do not
            // retain a pointer or touch the allocation after publishing it.
            SetLastError(ERROR_SUCCESS);
            if GlobalUnlock(handle).is_err() && GetLastError() != ERROR_SUCCESS {
                return Err(Error::from_thread());
            }
            OpenClipboard(Some(owner))?;
            let _clipboard = ClipboardGuard;
            if !permitted() {
                return Err(E_ABORT.into());
            }
            EmptyClipboard()?;
            if !permitted() {
                return Err(E_ABORT.into());
            }
            SetClipboardData(13, Some(HANDLE(handle.0)))?; // CF_UNICODETEXT
            memory.0 = None; // ownership transfers only after successful publish
            Ok(())
        }
    }
}
