//! Disposable native views for process-local TSF diagnostics.
use crate::render::{bitmap_info, Frame, HEIGHT, WIDTH};
use std::cell::{Cell, RefCell};
use std::ptr;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::{self, ThreadId};
use windows::core::{Interface, Result, HRESULT};
use windows::Win32::Foundation::{
    E_ABORT, E_FAIL, E_UNEXPECTED, HWND, LPARAM, LRESULT, RPC_E_WRONG_THREAD, WPARAM,
};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
use windows::Win32::UI::TextServices::{ITfDocumentMgr, ITfThreadMgr};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetForegroundWindow, GetWindowLongPtrW, PostMessageW, SetForegroundWindow, SetWindowLongPtrW,
    SetWindowTextW, TranslateMessage, GWLP_USERDATA, GWLP_WNDPROC, MSG, WM_APP, WM_CHAR, WM_CLOSE,
    WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_NCDESTROY, WM_PAINT, WNDPROC, WS_EX_TOOLWINDOW,
    WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

pub(crate) struct Window {
    hwnd: HWND,
    previous_focus: HWND,
    previous_foreground: HWND,
    view: Rc<View>,
}

#[derive(Default)]
struct View {
    frame: RefCell<Option<Frame>>,
    painted: Cell<usize>,
    failed: Cell<bool>,
    previous: Cell<WNDPROC>,
    active: Cell<bool>,
    token: Cell<usize>,
    queued: Cell<bool>,
    refresh: RefCell<Option<Refresh>>,
    refresh_error: Cell<Option<HRESULT>>,
    input: RefCell<Option<Input>>,
    input_error: Cell<Option<HRESULT>>,
}
type Refresh = Rc<dyn Fn() -> Result<Frame>>;
pub(crate) type Input = Rc<dyn Fn(u32, WPARAM, LPARAM) -> Result<bool>>;

const WM_REFRESH: u32 = WM_APP + 17;
static NEXT_RENDER_TOKEN: AtomicUsize = AtomicUsize::new(1);

#[derive(Clone)]
pub(crate) struct RefreshHandle {
    view: Weak<View>,
    hwnd: HWND,
    token: usize,
    owner: ThreadId,
}
impl RefreshHandle {
    pub(crate) fn request(&self) -> Result<()> {
        if self.owner != thread::current().id() {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        let view = self.view.upgrade().ok_or(E_ABORT)?;
        if !view.active.get() || view.token.get() != self.token {
            return Err(E_ABORT.into());
        }
        if view.queued.replace(true) {
            return Ok(());
        }
        // SAFETY: live owned HWND and scalar registration token, no raw message
        // payload. A closed/rebound view rejects this token before doing work.
        if let Err(error) =
            unsafe { PostMessageW(Some(self.hwnd), WM_REFRESH, WPARAM(self.token), LPARAM(0)) }
        {
            view.queued.set(false);
            return Err(error);
        }
        Ok(())
    }
    pub(crate) fn detach(&self) {
        if self.owner != thread::current().id() {
            return;
        }
        if let Some(view) = self.view.upgrade() {
            if view.token.get() == self.token {
                view.token.set(0);
                view.queued.set(false);
                view.refresh.borrow_mut().take();
                view.frame.borrow_mut().take();
                // SAFETY: only invalidate a still-live owned view; no read here.
                if view.active.get() {
                    unsafe {
                        let _ = InvalidateRect(Some(self.hwnd), None, false);
                    }
                }
            }
        }
    }
}

struct Paint {
    hwnd: HWND,
    data: PAINTSTRUCT,
}
impl Drop for Paint {
    fn drop(&mut self) {
        // SAFETY: balances BeginPaint for this callback's owned PAINTSTRUCT.
        unsafe {
            let _ = EndPaint(self.hwnd, &self.data);
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: userdata points into Window's Rc allocation. Take a strong
        // callback reference before COM/GDI can reenter and destroy the window.
        unsafe {
            let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const View;
            if pointer.is_null() {
                return DefWindowProcW(hwnd, message, wparam, lparam);
            }
            Rc::increment_strong_count(pointer);
            let view = Rc::from_raw(pointer);
            if message == WM_NCDESTROY {
                view.active.set(false);
                view.queued.set(false);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                return CallWindowProcW(view.previous.get(), hwnd, message, wparam, lparam);
            }
            if matches!(message, WM_CHAR | WM_KILLFOCUS | WM_CLOSE) && view.active.get() {
                let input = view.input.borrow().clone();
                if let Some(input) = input {
                    match input(message, wparam, lparam) {
                        Ok(true) => return LRESULT(0),
                        Ok(false) => {}
                        Err(error) => {
                            view.input_error.set(Some(error.code()));
                            return LRESULT(0);
                        }
                    }
                }
            }
            if message == WM_REFRESH {
                if !view.active.get() || wparam.0 != view.token.get() {
                    return LRESULT(0);
                }
                view.queued.set(false);
                let refresh = view.refresh.borrow().clone();
                if let Some(refresh) = refresh {
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| refresh()))
                            .unwrap_or_else(|_| Err(E_FAIL.into()));
                    if !view.active.get() || wparam.0 != view.token.get() {
                        return LRESULT(0);
                    }
                    view.failed.set(false);
                    match result {
                        Ok(frame) if frame.pixels.len() == WIDTH * HEIGHT * 4 => {
                            *view.frame.borrow_mut() = Some(frame);
                            view.refresh_error.set(None);
                        }
                        other => {
                            view.frame.borrow_mut().take();
                            view.refresh_error
                                .set(Some(other.err().map_or(E_FAIL, |error| error.code())));
                        }
                    }
                    if !InvalidateRect(Some(hwnd), None, false).as_bool() {
                        view.failed.set(true);
                    }
                    let _ = UpdateWindow(hwnd);
                }
                return LRESULT(0);
            }
            if message == WM_PAINT {
                let mut paint = Paint {
                    hwnd,
                    data: PAINTSTRUCT::default(),
                };
                let dc = BeginPaint(hwnd, &mut paint.data);
                if dc.0.is_null() {
                    view.failed.set(true);
                    return LRESULT(0);
                }
                if FillRect(
                    dc,
                    &paint.data.rcPaint,
                    HBRUSH(GetStockObject(WHITE_BRUSH).0),
                ) == 0
                {
                    view.failed.set(true);
                }
                match view.frame.try_borrow() {
                    Ok(frame) => {
                        if let Some(frame) = frame.as_ref() {
                            if SetDIBitsToDevice(
                                dc,
                                0,
                                0,
                                WIDTH as u32,
                                HEIGHT as u32,
                                0,
                                0,
                                0,
                                HEIGHT as u32,
                                frame.pixels.as_ptr().cast(),
                                &bitmap_info(),
                                DIB_RGB_COLORS,
                            ) == 0
                            {
                                view.failed.set(true);
                            }
                        }
                    }
                    Err(_) => view.failed.set(true),
                }
                view.painted.set(view.painted.get().saturating_add(1));
                return LRESULT(0);
            }
            CallWindowProcW(view.previous.get(), hwnd, message, wparam, lparam)
        }
    }))
    .unwrap_or(LRESULT(0))
}
impl Window {
    pub(crate) fn new() -> Result<Self> {
        // SAFETY: create a built-in STATIC, then subclass its paint handling
        // while Window owns the callback's stable boxed state.
        // Creates a short-lived diagnostic view; no injected keyboard events.
        unsafe {
            let previous_focus = GetFocus();
            let previous_foreground = GetForegroundWindow();
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                windows::core::w!("STATIC"),
                windows::core::w!("Sumibi TSF diagnostic (automatic test)"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                80,
                80,
                680,
                240,
                None,
                None,
                None,
                None,
            )?;
            let window = Self {
                hwnd,
                previous_focus,
                previous_foreground,
                view: Rc::new(View {
                    active: Cell::new(true),
                    ..Default::default()
                }),
            };
            let pointer = Rc::as_ptr(&window.view) as isize;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, pointer);
            if GetWindowLongPtrW(hwnd, GWLP_USERDATA) != pointer {
                return Err(E_FAIL.into());
            }
            let previous = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, window_proc as *const () as isize);
            if previous == 0 {
                return Err(E_FAIL.into());
            }
            // SAFETY: non-null previous procedure returned by the subclass API.
            window
                .view
                .previous
                .set(std::mem::transmute::<isize, WNDPROC>(previous));
            Ok(window)
        }
    }
    pub(crate) fn hwnd(&self) -> HWND {
        self.hwnd
    }
    pub(crate) fn set_title(&self, title: &str) -> Result<()> {
        let text: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
        // SAFETY: live owned view and a null-terminated string for this call.
        unsafe { SetWindowTextW(self.hwnd, windows::core::PCWSTR(text.as_ptr())) }
    }
    pub(crate) fn set_input(&self, input: Input) -> Result<()> {
        if !self.view.active.get() {
            return Err(E_ABORT.into());
        }
        if self.view.input.borrow().is_some() {
            return Err(E_UNEXPECTED.into());
        }
        self.view.input_error.set(None);
        *self.view.input.borrow_mut() = Some(input);
        Ok(())
    }
    pub(crate) fn clear_input(&self) {
        self.view.input.borrow_mut().take();
    }
    pub(crate) fn input_status(&self) -> Result<()> {
        self.view
            .input_error
            .get()
            .map_or(Ok(()), |error| Err(error.into()))
    }
    pub(crate) fn is_open(&self) -> bool {
        self.view.active.get()
    }
    pub(crate) fn route_message(&self, message: &MSG) -> Result<()> {
        if message.hwnd == self.hwnd && !self.view.active.get() {
            return Ok(());
        }
        if message.hwnd == self.hwnd
            && self.view.active.get()
            && matches!(message.message, WM_KEYDOWN | WM_KEYUP)
        {
            let input = self.view.input.borrow().clone();
            if let Some(input) = input {
                // Route before TranslateMessage: a consumed key must not
                // generate a second WM_CHAR and edit the document twice.
                if input(message.message, message.wParam, message.lParam)? {
                    return Ok(());
                }
            }
        }
        // SAFETY: dispatch only this thread's retrieved MSG. No key injection.
        unsafe {
            let _ = TranslateMessage(message);
            DispatchMessageW(message);
        }
        self.input_status()
    }
    pub(crate) fn focus(&self) -> Result<()> {
        if !self.view.active.get() {
            return Err(E_ABORT.into());
        }
        // SAFETY: owned live window on this thread. SetFocus returns the old
        // focus, which can legitimately be null; verify the new focus directly.
        unsafe {
            let _ = SetFocus(Some(self.hwnd));
            if GetFocus() != self.hwnd {
                return Err(E_FAIL.into());
            }
        }
        Ok(())
    }
    pub(crate) fn attach_refresh(&self, refresh: Refresh) -> Result<RefreshHandle> {
        if !self.view.active.get() {
            return Err(E_ABORT.into());
        }
        if self.view.refresh.borrow().is_some() {
            return Err(E_UNEXPECTED.into());
        }
        // Process-wide uniqueness also covers HWND reuse after window closure.
        let token = NEXT_RENDER_TOKEN
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| windows::core::Error::from_hresult(E_ABORT))?;
        self.view.token.set(token);
        self.view.queued.set(false);
        *self.view.refresh.borrow_mut() = Some(refresh);
        Ok(RefreshHandle {
            view: Rc::downgrade(&self.view),
            hwnd: self.hwnd,
            token,
            owner: thread::current().id(),
        })
    }
    pub(crate) fn frame(&self) -> Result<std::cell::Ref<'_, Frame>> {
        if !self.view.active.get() {
            return Err(E_ABORT.into());
        }
        if self.view.failed.get() {
            return Err(E_FAIL.into());
        }
        if let Some(error) = self.view.refresh_error.get() {
            return Err(error.into());
        }
        std::cell::Ref::filter_map(self.view.frame.borrow(), |frame| frame.as_ref())
            .map_err(|_| E_ABORT.into())
    }
    pub(crate) fn painted(&self) -> usize {
        self.view.painted.get()
    }
    pub(crate) fn is_blank(&self) -> bool {
        !self.view.failed.get() && self.view.frame.borrow().is_none()
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        let was_active = self.view.active.replace(false);
        self.view.refresh.borrow_mut().take();
        self.view.input.borrow_mut().take();
        // SAFETY: all TSF associations are released before destroying this view.
        unsafe {
            let restore_foreground = was_active && GetForegroundWindow() == self.hwnd;
            if was_active && GetFocus() == self.hwnd {
                let _ = SetFocus(if self.previous_focus.0.is_null() {
                    None
                } else {
                    Some(self.previous_focus)
                });
            }
            if was_active {
                if let Err(error) = DestroyWindow(self.hwnd) {
                    eprintln!("diagnostic window cleanup: {error}");
                }
            }
            if restore_foreground && !self.previous_foreground.0.is_null() {
                let _ = SetForegroundWindow(self.previous_foreground);
            }
        }
    }
}

pub(crate) struct Association {
    manager: ITfThreadMgr,
    window: HWND,
    // AssociateFocus does not retain the document: keep it alive ourselves.
    _document: ITfDocumentMgr,
    previous: Option<ITfDocumentMgr>,
    armed: bool,
    view: Weak<View>,
}

fn associate(
    manager: &ITfThreadMgr,
    window: HWND,
    document: Option<&ITfDocumentMgr>,
) -> Result<Option<ITfDocumentMgr>> {
    let mut previous = ptr::null_mut();
    // SAFETY: nullable previous document is valid. The generated convenience
    // method expects non-null output, so use the ABI and preserve the null case.
    unsafe {
        (manager.vtable().AssociateFocus)(
            manager.as_raw(),
            window,
            document.map_or(ptr::null_mut(), Interface::as_raw),
            &mut previous,
        )
        .ok()?;
        Ok(if previous.is_null() {
            None
        } else {
            Some(ITfDocumentMgr::from_raw(previous))
        })
    }
}

impl Association {
    pub(crate) fn new(
        manager: &ITfThreadMgr,
        window: &Window,
        document: &ITfDocumentMgr,
    ) -> Result<Self> {
        let previous = associate(manager, window.hwnd, Some(document))?;
        Ok(Self {
            manager: manager.clone(),
            window: window.hwnd,
            _document: document.clone(),
            previous,
            armed: true,
            view: Rc::downgrade(&window.view),
        })
    }
    pub(crate) fn close(&mut self) -> Result<()> {
        if self.armed {
            if !self.view.upgrade().is_some_and(|view| view.active.get()) {
                // A destroyed HWND has no association to restore. Never act
                // on a recycled handle belonging to a later native window.
                self.armed = false;
                return Ok(());
            }
            let _ = associate(&self.manager, self.window, self.previous.as_ref())?;
            self.armed = false;
        }
        Ok(())
    }
}
impl Drop for Association {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            eprintln!("TSF window association cleanup: {error}");
        }
    }
}
