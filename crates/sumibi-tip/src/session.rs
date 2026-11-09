//! Self-healing logon-scoped Host discovery, session lifecycle and protected conversion dispatch.
#![allow(non_snake_case)]

use crate::candidate_popup::CandidatePopup;
use crate::candidate_session::CandidateSession;
use crate::edit::manual::ManualEdit;
use crate::edit::{EditPolicy, ProtectionProfile};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Arc;
use std::thread::{self, JoinHandle, ThreadId};
use std::time::{Duration, Instant};
use sumibi_core::candidate::CandidateItem;
use sumibi_core::{RequestId, Revision};
use sumibi_ipc::bootstrap::Mode;
#[cfg(not(feature = "spike"))]
use sumibi_ipc::current_bootstrap_pipe_name;
#[cfg(feature = "spike")]
use sumibi_ipc::fixture_bootstrap_pipe_name as current_bootstrap_pipe_name;
use sumibi_ipc::job::{Request, Reservation};
use sumibi_ipc::worker::Worker;
use sumibi_ipc::Stop;
use windows::core::{Interface, Result, BOOL, GUID, PCWSTR};
use windows::Win32::Foundation::{
    E_FAIL, HINSTANCE, HWND, LPARAM, LRESULT, RPC_E_WRONG_THREAD, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::TextServices::*;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowLongPtrW, PostMessageW,
    RegisterClassExW, SetWindowLongPtrW, UnregisterClassW, GWLP_USERDATA, HWND_MESSAGE, WM_APP,
    WM_NCDESTROY, WNDCLASSEXW,
};

pub const COMMAND_CONVERT: GUID = GUID::from_u128(0x42e4ca10_7cae_4a74_bf57_d497c5667a2c);
pub const CONVERT_KEY: TF_PRESERVEDKEY = TF_PRESERVEDKEY {
    uVKey: 0x4a, // 'J'
    uModifiers: TF_MOD_CONTROL,
};

const WM_SUMIBI_DRAIN: u32 = WM_APP + 42;

/// Lock-free observable state shared between the background IPC worker and the STA thread.
pub struct SharedSessionState {
    connected: AtomicBool,
    allowed: AtomicBool,
    input: AtomicU64,
    settings: AtomicU64,
    authorization: AtomicU64,
    protection_profile: std::sync::atomic::AtomicU8,
}

impl SharedSessionState {
    fn new() -> Self {
        Self {
            connected: AtomicBool::new(false),
            allowed: AtomicBool::new(false),
            input: AtomicU64::new(1),
            settings: AtomicU64::new(0),
            authorization: AtomicU64::new(0),
            protection_profile: std::sync::atomic::AtomicU8::new(0),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.connected.load(Ordering::Acquire) && self.allowed.load(Ordering::Acquire)
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    pub fn revision(&self) -> Revision {
        Revision {
            input: self.input.load(Ordering::Acquire),
            settings: self.settings.load(Ordering::Acquire),
            authorization: self.authorization.load(Ordering::Acquire),
        }
    }

    pub fn protection_profile(&self) -> ProtectionProfile {
        match self.protection_profile.load(Ordering::Acquire) {
            1 => ProtectionProfile::Strict,
            _ => ProtectionProfile::Standard,
        }
    }

    pub fn set_protection_profile(&self, profile: ProtectionProfile) {
        let val = match profile {
            ProtectionProfile::Standard => 0,
            ProtectionProfile::Strict => 1,
        };
        self.protection_profile.store(val, Ordering::Release);
    }
}

enum WorkerCommand {
    Convert {
        current_result: Option<String>,
        request_id: u64,
        revision: Revision,
        text: String,
        timeout: Duration,
    },
}

enum WorkerEvent {
    Completed {
        request_id: u64,
        text: String,
        alternatives: Option<Vec<CandidateItem>>,
    },
    Failed {
        request_id: u64,
    },
}

#[inline]
pub(crate) fn debug_log(msg: &str) {
    let wide: Vec<u16> = format!("[Sumibi-TIP] {msg}\0").encode_utf16().collect();
    unsafe {
        windows::Win32::System::Diagnostics::Debug::OutputDebugStringW(windows::core::PCWSTR(
            wide.as_ptr(),
        ));
    }
}

struct ConvertedText {
    source: String,
    current: String,
    ticket: ManualEdit,
}

struct PendingEdit {
    id: u64,
    target: ConvertedText,
    additional: bool,
}

struct CandidateSelection {
    target: ConvertedText,
    session: CandidateSession,
    popup: CandidatePopup,
}

struct DrainSink {
    owner: ThreadId,
    manager: ITfThreadMgr,
    client: u32,
    state: Arc<SharedSessionState>,
    event_receiver: Receiver<WorkerEvent>,
    pending_ticket: RefCell<Option<PendingEdit>>,
    last_conversion: RefCell<Option<ConvertedText>>,
    selection: RefCell<Option<CandidateSelection>>,
}

impl DrainSink {
    fn ready_for(&self, ticket: &ManualEdit) -> bool {
        self.state.is_ready() && self.state.revision() == ticket.revision() && ticket.is_current()
    }

    fn apply(&self, target: ConvertedText, replacement: String) -> Result<()> {
        let ConvertedText { source, ticket, .. } = target;
        if !self.ready_for(&ticket) {
            return Err(windows::Win32::Foundation::E_ABORT.into());
        }
        let context = ticket.context().clone();
        let request = ticket.request();
        let revision = ticket.revision();
        let anchor = ticket.finish_retaining(
            &self.manager,
            self.client,
            request,
            revision,
            replacement.clone(),
            {
                let state = self.state.clone();
                move || state.is_ready() && state.revision() == revision
            },
        )?;
        // Initial conversion has already succeeded. Reconversion is available
        // only when the same TSF range can be captured after the write.
        if let Some(anchor) = anchor {
            let policy = Rc::new(Cell::new(EditPolicy {
                revision,
                allowed: true,
                profile: self.state.protection_profile(),
            }));
            let ticket = ManualEdit::capture_with_run(
                &self.manager,
                &context,
                self.client,
                policy,
                request,
                request.0,
                Instant::now() + Duration::from_secs(60),
                Some(replacement.encode_utf16().collect()),
            );
            if let Ok(ticket) = ticket {
                if self.ready_for(&ticket) && ticket.matches_anchor(anchor).is_ok() {
                    self.last_conversion.replace(Some(ConvertedText {
                        source,
                        current: replacement,
                        ticket,
                    }));
                }
            }
        }
        Ok(())
    }

    fn drain(&self) -> Result<bool> {
        if self.owner != thread::current().id() {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        let mut handled = false;
        while let Ok(event) = self.event_receiver.try_recv() {
            let request_id = match &event {
                WorkerEvent::Completed { request_id, .. } | WorkerEvent::Failed { request_id } => {
                    *request_id
                }
            };
            if !self
                .pending_ticket
                .borrow()
                .as_ref()
                .is_some_and(|pending| pending.id == request_id)
            {
                continue;
            }
            let Some(pending) = self.pending_ticket.take() else {
                continue;
            };
            let target = pending.target;
            if pending.additional && matches!(event, WorkerEvent::Failed { .. }) {
                if target.ticket.is_current() && self.state.revision() == target.ticket.revision() {
                    self.last_conversion.replace(Some(target));
                }
                continue;
            }
            if !self.ready_for(&target.ticket) {
                debug_log("Drain discarded stale target");
                continue;
            }
            match event {
                WorkerEvent::Completed {
                    text,
                    alternatives: None,
                    ..
                } if !pending.additional => match self.apply(target, text) {
                    Ok(()) => handled = true,
                    Err(error) => debug_log(&format!(
                        "Drain edit rejected: HRESULT 0x{:08x}",
                        error.code().0 as u32
                    )),
                },
                WorkerEvent::Completed {
                    alternatives: Some(items),
                    ..
                } if pending.additional && items.len() > 1 => {
                    if target.ticket.source().ok().as_deref() != Some(&target.current) {
                        continue;
                    }
                    let session = CandidateSession::new(
                        target.source.clone(),
                        target.current.clone(),
                        items,
                        request_id,
                    );
                    let bounds = target.ticket.candidate_bounds().ok();
                    if self.ready_for(&target.ticket) {
                        if let Ok(popup) = CandidatePopup::new(bounds, &session) {
                            self.selection.replace(Some(CandidateSelection {
                                target,
                                session,
                                popup,
                            }));
                            handled = true;
                        }
                    }
                }
                _ if pending.additional => {
                    self.last_conversion.replace(Some(target));
                }
                _ => {}
            }
        }
        Ok(handled)
    }

    fn cancel_pending(&self) {
        if let Some(mut pending) = self.pending_ticket.take() {
            let _ = pending.target.ticket.cancel();
        }
    }

    fn expire_targets(&self) {
        let expired = self
            .last_conversion
            .borrow()
            .as_ref()
            .is_some_and(|target| {
                !target.ticket.is_current() || self.state.revision() != target.ticket.revision()
            });
        if expired {
            self.last_conversion.take();
        }
        self.expire_selection();
    }

    fn expire_selection(&self) {
        let expired = self
            .selection
            .borrow()
            .as_ref()
            .is_some_and(|selection| !self.ready_for(&selection.target.ticket));
        if expired {
            self.selection.take();
        }
    }

    fn candidate_key(&self, context: Option<&ITfContext>, vk: u32, test: bool) -> bool {
        self.expire_selection();
        let Some(mut active) = self.selection.take() else {
            return false;
        };
        let same_context = context.is_some_and(|context| {
            let token = context_token(context);
            token != 0 && token == context_token(active.target.ticket.context())
        });
        if !same_context || command_modifier() {
            return false;
        }
        let digit = char::from_u32(vk).filter(|digit| digit.is_ascii_digit());
        let accepted = match vk {
            0x0d | 0x1b | 0x26 | 0x28 => true,
            _ => digit.is_some_and(|digit| active.session.digit_index(digit).is_some()),
        };
        if !accepted {
            return false;
        }
        if test {
            self.selection.replace(Some(active));
            return true;
        }
        match vk {
            0x26 | 0x28 => {
                if vk == 0x26 {
                    active.session.prev();
                } else {
                    active.session.next();
                }
                active.popup.render(&active.session);
                self.selection.replace(Some(active));
            }
            0x1b => {
                self.last_conversion.replace(Some(active.target));
            }
            _ => {
                if let Some(digit) = digit {
                    active.session.select_digit(digit);
                }
                if let Some(text) = active.session.confirm() {
                    if text == active.target.current {
                        self.last_conversion.replace(Some(active.target));
                    } else {
                        let _ = self.apply(active.target, text);
                    }
                }
            }
        }
        true
    }
}

struct DrainWindow {
    hwnd: HWND,
    class_name: Vec<u16>,
}

impl DrainWindow {
    fn create(sink: *const DrainSink) -> Option<Self> {
        let instance = unsafe { GetModuleHandleW(None).ok()? };
        let hinstance: HINSTANCE = instance.into();
        let class_name_str = format!("Sumibi.TipDrain.{:x}\0", sink as usize);
        let class_name: Vec<u16> = class_name_str.encode_utf16().collect();

        let wnd_class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(drain_window_proc),
            hInstance: hinstance,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };

        let atom = unsafe { RegisterClassExW(&wnd_class) };
        if atom == 0 {
            return None;
        }

        let hwnd = unsafe {
            CreateWindowExW(
                Default::default(),
                PCWSTR(class_name.as_ptr()),
                PCWSTR::null(),
                Default::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(hinstance),
                None,
            )
        };

        if let Ok(hwnd) = hwnd {
            unsafe {
                let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, sink as isize);
            }
            Some(Self { hwnd, class_name })
        } else {
            unsafe {
                let _ = UnregisterClassW(PCWSTR(class_name.as_ptr()), Some(hinstance));
            }
            None
        }
    }

    fn destroy(&mut self) {
        unsafe {
            let instance: Option<HINSTANCE> = GetModuleHandleW(None).ok().map(Into::into);
            let _ = SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
            let _ = UnregisterClassW(PCWSTR(self.class_name.as_ptr()), instance);
        }
    }
}

unsafe extern "system" fn drain_window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_SUMIBI_DRAIN => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const DrainSink;
            if !ptr.is_null() {
                let sink = unsafe { &*ptr };
                let _ = sink.drain();
                if sink.selection.borrow().is_some() || sink.last_conversion.borrow().is_some() {
                    unsafe {
                        windows::Win32::UI::WindowsAndMessaging::SetTimer(Some(hwnd), 1, 250, None);
                    }
                }
            }
            LRESULT(0)
        }
        windows::Win32::UI::WindowsAndMessaging::WM_TIMER => {
            let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const DrainSink;
            if let Some(sink) = unsafe { ptr.as_ref() } {
                sink.expire_targets();
                if sink.selection.borrow().is_none() && sink.last_conversion.borrow().is_none() {
                    unsafe {
                        let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(hwnd), 1);
                    }
                }
            }
            LRESULT(0)
        }
        WM_NCDESTROY => unsafe {
            let _ = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DefWindowProcW(hwnd, msg, wparam, lparam)
        },
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Layout translation of a key press using the current shift/caps state and
/// without consuming dead-key state (flag 0x4). `None` for non-character keys.
fn translate_key(vk: u32, flags: isize) -> Option<u16> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyState, GetKeyboardLayout, ToUnicodeEx, VK_CAPITAL, VK_CONTROL, VK_MENU, VK_SHIFT,
    };
    let mut state = [0u8; 256];
    for key in [VK_SHIFT, VK_CONTROL, VK_MENU, VK_CAPITAL] {
        // SAFETY: reads the calling thread's keyboard state only.
        state[key.0 as usize] = unsafe { GetKeyState(key.0 as i32) } as u8;
    }
    let scan = ((flags >> 16) & 0xff) as u32;
    let mut buffer = [0u16; 4];
    // SAFETY: all buffers are live for the call; flag 0x4 leaves the
    // thread's keyboard (dead-key) state unchanged.
    let count = unsafe {
        ToUnicodeEx(
            vk,
            scan,
            &state,
            &mut buffer,
            0x4,
            Some(GetKeyboardLayout(0)),
        )
    };
    (count == 1).then_some(buffer[0])
}

/// Ctrl, Alt or Windows key held: a command, not text.
fn command_modifier() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN,
    };
    // SAFETY: reads the calling thread's keyboard state only.
    [VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
        .iter()
        .any(|key| unsafe { GetKeyState(key.0 as i32) } < 0)
}

pub struct TipHostSession {
    owner: ThreadId,
    manager: ITfThreadMgr,
    client: u32,
    state: Arc<SharedSessionState>,
    stop: Stop,
    command_sender: SyncSender<WorkerCommand>,
    sink: Rc<DrainSink>,
    worker_thread: RefCell<Option<JoinHandle<()>>>,
    preserved_registered: Cell<bool>,
    next_request_id: Cell<u64>,
    drain_window: RefCell<Option<DrainWindow>>,
    /// Passive romaji run for caret conversion (ADR 0104); owner thread only.
    run: RefCell<crate::run_record::RunRecord>,
}

/// Identity token of a TSF context: the IUnknown identity of the object, so
/// every interface/proxy of the same context maps to one value. Never
/// dereferenced.
fn context_token(context: &ITfContext) -> usize {
    context
        .cast::<windows::core::IUnknown>()
        .map(|unknown| unknown.as_raw() as usize)
        .unwrap_or(0)
}

impl TipHostSession {
    /// Start the autonomous session manager and register the conversion hotkey.
    pub fn start(manager: &ITfThreadMgr, client: u32) -> Result<Self> {
        let stop = Stop::new().map_err(|_| windows::core::Error::from(E_FAIL))?;
        let state = Arc::new(SharedSessionState::new());

        let (cmd_tx, cmd_rx) = mpsc::sync_channel(4);
        let (evt_tx, evt_rx) = mpsc::sync_channel(4);

        let sink = Rc::new(DrainSink {
            owner: thread::current().id(),
            manager: manager.clone(),
            client,
            state: state.clone(),
            event_receiver: evt_rx,
            pending_ticket: RefCell::new(None),
            last_conversion: RefCell::new(None),
            selection: RefCell::new(None),
        });

        let drain_window = DrainWindow::create(Rc::as_ptr(&sink));
        let drain_hwnd = drain_window.as_ref().map(|w| w.hwnd.0 as isize);

        let thread_state = state.clone();
        let thread_stop = stop.clone();

        let worker_thread = thread::Builder::new()
            .name("sumibi-tip-session".into())
            .spawn(move || {
                worker_loop(thread_state, thread_stop, cmd_rx, evt_tx, drain_hwnd);
            })
            .map_err(|_| windows::core::Error::from(E_FAIL))?;

        let session = Self {
            owner: thread::current().id(),
            manager: manager.clone(),
            client,
            state,
            stop,
            command_sender: cmd_tx,
            sink,
            worker_thread: RefCell::new(Some(worker_thread)),
            preserved_registered: Cell::new(false),
            next_request_id: Cell::new(0),
            drain_window: RefCell::new(drain_window),
            run: RefCell::new(crate::run_record::RunRecord::default()),
        };

        // Register preserved key (Ctrl+J)
        session.register_preserved_key()?;

        Ok(session)
    }

    pub fn state(&self) -> &Arc<SharedSessionState> {
        &self.state
    }

    fn register_preserved_key(&self) -> Result<()> {
        if self.owner != thread::current().id() {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        if self.preserved_registered.get() {
            return Ok(());
        }
        let keys: ITfKeystrokeMgr = self.manager.cast()?;
        let res = unsafe {
            keys.PreserveKey(
                self.client,
                &COMMAND_CONVERT,
                &CONVERT_KEY,
                &"Sumibi conversion".encode_utf16().collect::<Vec<_>>(),
            )
        };
        if let Err(error) = res {
            let code = error.code().0 as u32;
            if code != 0x80040205 && code != 0x80040503 {
                return Err(error);
            }
        }
        self.preserved_registered.set(true);
        Ok(())
    }

    fn unregister_preserved_key(&self) -> Result<()> {
        if self.owner != thread::current().id() {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        if !self.preserved_registered.get() {
            return Ok(());
        }
        let keys: ITfKeystrokeMgr = self.manager.cast()?;
        unsafe {
            keys.UnpreserveKey(&COMMAND_CONVERT, &CONVERT_KEY)?;
        }
        self.preserved_registered.set(false);
        Ok(())
    }

    /// Discard the passive romaji run (focus loss, deactivation, boundaries).
    pub fn clear_run(&self) {
        if self.owner == thread::current().id() {
            self.run.borrow_mut().clear();
            self.sink.selection.take();
            self.sink.last_conversion.take();
            self.sink.cancel_pending();
        }
    }

    /// Observe one key press without ever consuming it. Records only romaji
    /// units, and only while the Host session is ready and the context passes
    /// the metadata-only status gate; every other key is a boundary.
    pub fn observe_key(&self, context: Option<&ITfContext>, vk: u32, flags: isize) {
        if self.owner != thread::current().id() {
            return;
        }
        // Ctrl and Ctrl+J are part of a repeat conversion gesture.
        if !matches!(vk, 0x11 | 0xa2 | 0xa3)
            && !(vk == 0x4a
                && unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x11) < 0 })
        {
            self.sink.last_conversion.take();
        }
        let Some(context) = context else {
            self.run.borrow_mut().clear();
            return;
        };
        let token = context_token(context);
        let is_ready = self.state.is_ready();
        let status_res = unsafe { context.GetStatus() };
        let status_ok = status_res
            .as_ref()
            .map(|status| {
                crate::protection::input_status_denial_with_profile(
                    *status,
                    self.state.protection_profile(),
                )
                .is_none()
            })
            .unwrap_or(false);
        let gated = is_ready && status_ok;
        if !gated {
            debug_log(&format!(
                "Key observed: gated=false (is_ready={is_ready} [connected={}, allowed={}], status_ok={status_ok})",
                self.state.is_connected(),
                self.state.allowed.load(Ordering::Acquire)
            ));
            self.run.borrow_mut().clear();
            return;
        }
        let action = crate::run_record::classify(vk, translate_key(vk, flags), command_modifier());
        let mut record = self.run.borrow_mut();
        record.apply(token, action);
        debug_log(&format!(
            "Key observed: gated=true, action={}, run_units={}",
            action.label(),
            record.len()
        ));
    }

    pub(crate) fn candidate_key(&self, context: Option<&ITfContext>, vk: u32, test: bool) -> bool {
        self.owner == thread::current().id() && self.sink.candidate_key(context, vk, test)
    }

    /// Handle preserved hotkey event safely. Returns Ok(TRUE) if conversion is admitted and dispatched.
    pub fn on_preserved_key(&self, context: Option<&ITfContext>, key: &GUID) -> Result<BOOL> {
        if self.owner != thread::current().id() {
            return Err(RPC_E_WRONG_THREAD.into());
        }
        if *key != COMMAND_CONVERT {
            return Ok(false.into());
        }

        // 1. Consent and readiness gate: if not ready, ignore key immediately (never read text!)
        if !self.state.is_ready() {
            debug_log(
                "Preserved key ignored: host session is not ready (not connected or not allowed)",
            );
            return Ok(false.into());
        }

        let Some(context) = context else {
            debug_log("Preserved key ignored: TSF context is None");
            return Ok(false.into());
        };

        self.sink.selection.take();
        if self
            .sink
            .pending_ticket
            .borrow()
            .as_ref()
            .is_some_and(|pending| pending.additional)
        {
            return Ok(true.into());
        }
        if let Some(target) = self.sink.last_conversion.take() {
            let token = context_token(context);
            if token != 0
                && token == context_token(target.ticket.context())
                && self.sink.ready_for(&target.ticket)
                && target.ticket.source().ok().as_deref() == Some(&target.current)
            {
                let request_id = self.next_request_id.get().wrapping_add(1).max(1);
                self.next_request_id.set(request_id);
                return Ok(self.dispatch(request_id, target, true).into());
            }
        }

        // Log non-confidential TSF protection diagnostics for manual app verification
        if let Ok(status) = unsafe { context.GetStatus() } {
            let denial = crate::protection::input_status_denial_with_profile(
                status,
                self.state.protection_profile(),
            );
            debug_log(&format!(
                "Preserved key status: dyn=0x{:08x}, stat=0x{:08x}, denial={:?}",
                status.dwDynamicFlags, status.dwStaticFlags, denial
            ));
        } else {
            debug_log("Preserved key status query failed");
        }

        if let Ok(compartments) = context.cast::<ITfCompartmentMgr>() {
            let kbd_disabled =
                unsafe { compartments.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_DISABLED) }
                    .and_then(|c| unsafe { c.GetValue() })
                    .map(|v| unsafe { v.Anonymous.Anonymous.Anonymous.lVal })
                    .unwrap_or(-1);
            let empty_ctx = unsafe { compartments.GetCompartment(&GUID_COMPARTMENT_EMPTYCONTEXT) }
                .and_then(|c| unsafe { c.GetValue() })
                .map(|v| unsafe { v.Anonymous.Anonymous.Anonymous.lVal })
                .unwrap_or(-1);
            debug_log(&format!(
                "Preserved key compartments: kbd_disabled={kbd_disabled}, empty_ctx={empty_ctx}"
            ));
        }

        // Retain the verified run without changing the application's caret.
        // An explicit selection takes precedence; the record is single-use.
        let recorded = self.run.borrow_mut().take(context_token(context));
        // 2. Capture selection and check protection inside an edit session
        let edit_policy = Rc::new(Cell::new(EditPolicy {
            revision: self.state.revision(),
            allowed: true,
            profile: self.state.protection_profile(),
        }));

        let request_id = self.next_request_id.get().wrapping_add(1).max(1);
        self.next_request_id.set(request_id);

        let deadline = Instant::now() + Duration::from_secs(5);
        let ticket = match ManualEdit::capture_with_run(
            &self.manager,
            context,
            self.client,
            edit_policy,
            RequestId(request_id.into()),
            request_id.into(),
            deadline,
            recorded,
        ) {
            Ok(ticket) => ticket,
            Err(error) => {
                debug_log(&format!(
                    "Preserved key capture rejected: HRESULT 0x{:08x}",
                    error.code().0 as u32
                ));
                return Ok(false.into());
            }
        };

        let text = match ticket.source() {
            Ok(source) => source,
            Err(error) => {
                debug_log(&format!(
                    "Preserved key source rejected: HRESULT 0x{:08x}",
                    error.code().0 as u32
                ));
                return Ok(false.into());
            }
        };

        Ok(self
            .dispatch(
                request_id,
                ConvertedText {
                    source: text,
                    current: String::new(),
                    ticket,
                },
                false,
            )
            .into())
    }

    fn dispatch(&self, request_id: u64, target: ConvertedText, additional: bool) -> bool {
        let command = WorkerCommand::Convert {
            request_id,
            revision: target.ticket.revision(),
            text: target.source.clone(),
            current_result: additional.then(|| target.current.clone()),
            timeout: Duration::from_secs(5),
        };
        self.sink.pending_ticket.replace(Some(PendingEdit {
            id: request_id,
            target,
            additional,
        }));
        if self.command_sender.try_send(command).is_err() {
            self.sink.cancel_pending();
            return false;
        }
        true
    }

    /// Drain completed conversion events on the STA thread and apply text replacement.
    pub fn drain(&self) -> Result<bool> {
        self.sink.drain()
    }

    /// Stop the session gracefully.
    pub fn close(&self) -> Result<()> {
        self.clear_run();
        let _ = self.unregister_preserved_key();
        if let Some(mut window) = self.drain_window.borrow_mut().take() {
            window.destroy();
        }
        self.sink.cancel_pending();
        let _ = self.stop.cancel();
        if let Some(thread) = self.worker_thread.borrow_mut().take() {
            let _ = thread.join();
        }
        Ok(())
    }
}

impl Drop for TipHostSession {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn notify_drain(hwnd: Option<isize>, request_id: u64) {
    if let Some(hwnd_val) = hwnd {
        unsafe {
            let _ = PostMessageW(
                Some(HWND(hwnd_val as *mut _)),
                WM_SUMIBI_DRAIN,
                WPARAM(request_id as usize),
                LPARAM(0),
            );
        }
    }
}

fn worker_loop(
    state: Arc<SharedSessionState>,
    stop: Stop,
    receiver: Receiver<WorkerCommand>,
    event_sender: SyncSender<WorkerEvent>,
    drain_hwnd: Option<isize>,
) {
    let mut current_worker: Option<Worker> = None;
    let mut attempted = false;

    while !stop.is_cancelled().unwrap_or(true) {
        // 1. Ensure worker connection
        if current_worker.is_none() || current_worker.as_ref().is_some_and(|w| w.is_closed()) {
            state.connected.store(false, Ordering::Release);
            state.allowed.store(false, Ordering::Release);
            current_worker = None;

            // Preparation failures also clear current_worker. Throttle every
            // retry, including failures before the first policy arrives.
            if attempted {
                for _ in 0..10 {
                    if stop.is_cancelled().unwrap_or(true) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }
            attempted = true;

            match current_bootstrap_pipe_name() {
                Ok(pipe_name) => {
                    debug_log(&format!("bootstrap pipe name resolved: {pipe_name}"));
                    match Reservation::acquire() {
                        Ok(reservation) => {
                            let deadline = Instant::now() + Duration::from_secs(3);
                            match Worker::prepare_discovery(reservation, pipe_name, deadline) {
                                Ok(worker) => {
                                    debug_log("Worker::prepare_discovery succeeded");
                                    current_worker = Some(worker);
                                }
                                Err(err) => {
                                    debug_log(&format!(
                                        "Worker::prepare_discovery failed: {err:?}"
                                    ));
                                }
                            }
                        }
                        Err(err) => {
                            debug_log(&format!("Reservation::acquire failed: {err:?}"));
                        }
                    }
                }
                Err(err) => {
                    debug_log(&format!("current_bootstrap_pipe_name failed: {err:?}"));
                }
            }

            if current_worker.is_none() {
                for _ in 0..10 {
                    if stop.is_cancelled().unwrap_or(true) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                continue;
            }

            // Await initial policy preparation on the newly created worker
            let prep_deadline = Instant::now() + Duration::from_secs(3);
            let mut prep_ok = false;
            while Instant::now() < prep_deadline {
                if stop.is_cancelled().unwrap_or(true) {
                    return;
                }
                match current_worker.as_ref().unwrap().prepared_policy() {
                    Ok(Some(policy)) => {
                        debug_log(&format!(
                            "Policy received: allowed={}, settings={}, auth={}",
                            policy.allowed, policy.settings, policy.authorization
                        ));
                        state.settings.store(policy.settings, Ordering::Release);
                        state
                            .authorization
                            .store(policy.authorization, Ordering::Release);
                        state.allowed.store(policy.allowed, Ordering::Release);
                        state.set_protection_profile(policy.protection_profile);
                        state.connected.store(true, Ordering::Release);
                        prep_ok = true;
                        break;
                    }
                    Ok(None) => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(err) => {
                        debug_log(&format!("prepared_policy failed: {err:?}"));
                        break;
                    }
                }
            }

            if !prep_ok {
                state.connected.store(false, Ordering::Release);
                state.allowed.store(false, Ordering::Release);
                current_worker = None;
                continue;
            }
        }

        let worker = current_worker.as_mut().unwrap();

        // 2. Poll policy updates
        match worker.prepared_policy() {
            Ok(Some(policy)) => {
                state.settings.store(policy.settings, Ordering::Release);
                state
                    .authorization
                    .store(policy.authorization, Ordering::Release);
                state.allowed.store(policy.allowed, Ordering::Release);
                state.set_protection_profile(policy.protection_profile);
                state.connected.store(true, Ordering::Release);
            }
            Ok(None) => {}
            Err(_) => {
                // Connection broken or revoked
                state.connected.store(false, Ordering::Release);
                state.allowed.store(false, Ordering::Release);
                current_worker = None;
                continue;
            }
        }

        // 3. Check for outgoing conversion commands
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(WorkerCommand::Convert {
                request_id,
                revision,
                text,
                current_result,
                timeout,
            }) => {
                let deadline = Instant::now() + timeout;

                // Reserve permit
                match worker.reserve(request_id) {
                    Ok(permit) => {
                        let request = Request {
                            id: request_id,
                            revision: sumibi_protocol::message::Revision {
                                input: revision.input,
                                settings: revision.settings,
                                authorization: revision.authorization,
                            },
                            text,
                            current_result,
                            deadline,
                            mode: Mode::Pinned,
                        };

                        if worker.submit(permit, request).is_err() {
                            let _ = event_sender.send(WorkerEvent::Failed { request_id });
                            notify_drain(drain_hwnd, request_id);
                            continue;
                        }

                        // Wait for result
                        loop {
                            if stop.is_cancelled().unwrap_or(true) {
                                break;
                            }
                            match worker.try_result() {
                                Ok(Some(completion)) => {
                                    if completion.id == request_id {
                                        match (completion.outcome(), completion.alternatives()) {
                                            (Ok(result_text), Ok(alternatives)) => {
                                                let _ = event_sender.send(WorkerEvent::Completed {
                                                    request_id,
                                                    text: result_text.to_owned(),
                                                    alternatives: alternatives.map(<[_]>::to_vec),
                                                });
                                                notify_drain(drain_hwnd, request_id);
                                            }
                                            (outcome, _) => {
                                                debug_log(&format!(
                                                    "Worker completion failed: {:?}",
                                                    outcome.err()
                                                ));
                                                let _ = event_sender
                                                    .send(WorkerEvent::Failed { request_id });
                                                notify_drain(drain_hwnd, request_id);
                                            }
                                        }
                                    }
                                    break;
                                }
                                Ok(None) => {
                                    if Instant::now() >= deadline {
                                        let _ = worker.cancel(request_id);
                                        let _ =
                                            event_sender.send(WorkerEvent::Failed { request_id });
                                        notify_drain(drain_hwnd, request_id);
                                        break;
                                    }
                                    thread::sleep(Duration::from_millis(5));
                                }
                                Err(_) => {
                                    let _ = event_sender.send(WorkerEvent::Failed { request_id });
                                    notify_drain(drain_hwnd, request_id);
                                    break;
                                }
                            }
                        }
                    }
                    Err(_) => {
                        let _ = event_sender.send(WorkerEvent::Failed { request_id });
                        notify_drain(drain_hwnd, request_id);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    if let Some(mut worker) = current_worker {
        let _ = worker.shutdown(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sumibi_host::host_control::HostControl;
    use sumibi_host::host_service::HostService;
    use sumibi_host::manual::{Credential, Destination, Scheme};
    use sumibi_host::policy::PolicyAuthority;

    static HOST_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn configured_authority() -> Arc<PolicyAuthority> {
        let authority = Arc::new(PolicyAuthority::new(
            Destination::for_chat_completions(Scheme::Https, "example.test", 443, "/").unwrap(),
        ));
        authority.set_model("test-model").unwrap();
        authority
            .replace_credential(Credential::new("synthetic-test-key".into()))
            .unwrap();
        authority
    }

    #[test]
    fn shared_session_state_reflects_atomic_updates() {
        let state = SharedSessionState::new();
        assert!(!state.is_ready());
        assert!(!state.is_connected());

        state.connected.store(true, Ordering::Release);
        assert!(!state.is_ready());
        assert!(state.is_connected());

        state.allowed.store(true, Ordering::Release);
        assert!(state.is_ready());

        state.settings.store(3, Ordering::Release);
        state.authorization.store(5, Ordering::Release);
        let rev = state.revision();
        assert_eq!(rev.input, 1);
        assert_eq!(rev.settings, 3);
        assert_eq!(rev.authorization, 5);

        state.allowed.store(false, Ordering::Release);
        assert!(!state.is_ready());
    }

    fn supported() -> bool {
        match sumibi_ipc::PinnedProcess::current() {
            Ok(_) => true,
            Err(
                sumibi_ipc::Error::UnsupportedAppContainer
                | sumibi_ipc::Error::UnsupportedElevation
                | sumibi_ipc::Error::UnsupportedIntegrity
                | sumibi_ipc::Error::UnsupportedToken(_),
            ) => false,
            Err(error) => panic!("cannot inspect fixture identity: {error:?}"),
        }
    }

    #[test]
    fn session_on_preserved_key_denies_when_not_ready() {
        if !supported() {
            return;
        }
        let manager = crate::mock::new_manager().to_interface::<ITfThreadMgr>();
        let session = TipHostSession::start(&manager, 7).expect("start session");

        // When state is not ready, on_preserved_key immediately returns FALSE without reading
        let result = session
            .on_preserved_key(None, &COMMAND_CONVERT)
            .expect("on_preserved_key");
        assert!(!result.as_bool());

        session.close().expect("close session");
    }

    #[test]
    fn session_autonomously_discovers_host_and_updates_policy() {
        if !supported() {
            return;
        }
        let _lock = HOST_TEST_LOCK.lock().unwrap();

        let authority = configured_authority();
        let control =
            Arc::new(HostControl::start_diagnostic(move || Ok((authority, false))).unwrap());
        let mut host_service =
            HostService::start_with_control(control).expect("start host service");

        let manager = crate::mock::new_manager().to_interface::<ITfThreadMgr>();
        let session = TipHostSession::start(&manager, 7).expect("start session");

        // 1. Initially without consent, TIP session is not ready and denies hotkey
        assert!(!session.state().is_ready());
        let res = session
            .on_preserved_key(None, &COMMAND_CONVERT)
            .expect("on_preserved_key unconsented");
        assert!(!res.as_bool());

        // 2. Grant consent via host control
        let offer_deadline = Instant::now() + Duration::from_secs(5);
        let offer = loop {
            if let Ok(Some(status)) = host_service.try_status() {
                if status.phase == sumibi_host::host_control::Phase::Ready {
                    if let Some(offer) = status.offer {
                        break offer;
                    }
                }
            }
            if Instant::now() >= offer_deadline {
                panic!("timed out waiting for host offer");
            }
            thread::sleep(Duration::from_millis(10));
        };
        host_service
            .control()
            .try_grant(offer)
            .expect("grant consent");

        // 3. Wait for background worker to discover host and achieve ready state
        let ready_deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if session.state().is_ready() {
                break;
            }
            if Instant::now() >= ready_deadline {
                panic!("timed out waiting for session to become ready");
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(session.state().is_ready());
        assert!(session.state().is_connected());

        // 4. Revoke consent and confirm TIP observes revocation
        host_service.control().try_revoke().expect("revoke consent");
        let revoke_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if !session.state().is_ready() {
                break;
            }
            if Instant::now() >= revoke_deadline {
                panic!("timed out waiting for session to observe revocation");
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(!session.state().is_ready());

        session.close().expect("close session");
        host_service.stop().expect("stop host service");
    }

    #[test]
    fn session_reconnects_after_host_restart_and_recovers_ready() {
        if !supported() {
            return;
        }
        let _lock = HOST_TEST_LOCK.lock().unwrap();

        // 1. Start Host #1
        let authority1 = configured_authority();
        let control1 =
            Arc::new(HostControl::start_diagnostic(move || Ok((authority1, false))).unwrap());
        let mut host_service1 =
            HostService::start_with_control(control1).expect("start host service 1");

        let manager = crate::mock::new_manager().to_interface::<ITfThreadMgr>();
        let session = TipHostSession::start(&manager, 7).expect("start session");

        // Grant consent on Host #1
        let offer_deadline = Instant::now() + Duration::from_secs(5);
        let offer1 = loop {
            if let Ok(Some(status)) = host_service1.try_status() {
                if status.phase == sumibi_host::host_control::Phase::Ready {
                    if let Some(offer) = status.offer {
                        break offer;
                    }
                }
            }
            if Instant::now() >= offer_deadline {
                panic!("timed out waiting for host 1 offer");
            }
            thread::sleep(Duration::from_millis(10));
        };
        host_service1
            .control()
            .try_grant(offer1)
            .expect("grant consent 1");

        // Wait for TIP session to achieve ready
        let ready_deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if session.state().is_ready() {
                break;
            }
            if Instant::now() >= ready_deadline {
                panic!("timed out waiting for session ready with host 1");
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(session.state().is_ready());

        // 2. Stop Host #1 (simulating host shutdown or crash)
        host_service1.stop().expect("stop host service 1");

        // Confirm TIP session observes disconnection and is no longer ready
        let disconnect_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if !session.state().is_connected() && !session.state().is_ready() {
                break;
            }
            if Instant::now() >= disconnect_deadline {
                panic!("timed out waiting for session to disconnect after host 1 stop");
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(!session.state().is_connected());
        assert!(!session.state().is_ready());

        // 3. Start Host #2 (simulating host restart)
        let authority2 = configured_authority();
        let control2 =
            Arc::new(HostControl::start_diagnostic(move || Ok((authority2, false))).unwrap());
        let mut host_service2 =
            HostService::start_with_control(control2).expect("start host service 2");

        // Grant consent on Host #2
        let offer_deadline2 = Instant::now() + Duration::from_secs(5);
        let offer2 = loop {
            if let Ok(Some(status)) = host_service2.try_status() {
                if status.phase == sumibi_host::host_control::Phase::Ready {
                    if let Some(offer) = status.offer {
                        break offer;
                    }
                }
            }
            if Instant::now() >= offer_deadline2 {
                panic!("timed out waiting for host 2 offer");
            }
            thread::sleep(Duration::from_millis(10));
        };
        host_service2
            .control()
            .try_grant(offer2)
            .expect("grant consent 2");

        // 4. Wait for TIP session to autonomously self-heal and reconnect to Host #2
        let recover_deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if session.state().is_ready() {
                break;
            }
            if Instant::now() >= recover_deadline {
                panic!("timed out waiting for session to recover ready with host 2");
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(session.state().is_ready());
        assert!(session.state().is_connected());

        // Cleanup
        session.close().expect("close session");
        host_service2.stop().expect("stop host service 2");
    }

    #[test]
    fn drain_window_posts_and_processes_message_on_sta_thread() {
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, PeekMessageW, MSG, PM_REMOVE,
        };

        let manager = crate::mock::new_manager().to_interface::<ITfThreadMgr>();
        let (evt_tx, evt_rx) = mpsc::sync_channel(4);
        let sink = Rc::new(DrainSink {
            owner: thread::current().id(),
            manager,
            client: 7,
            state: Arc::new(SharedSessionState::new()),
            event_receiver: evt_rx,
            pending_ticket: RefCell::new(None),
            last_conversion: RefCell::new(None),
            selection: RefCell::new(None),
        });

        let mut window = DrainWindow::create(Rc::as_ptr(&sink)).expect("create drain window");
        let hwnd_val = window.hwnd.0 as isize;

        // Push a completed event into the channel
        evt_tx
            .send(WorkerEvent::Completed {
                request_id: 100,
                text: "test".into(),
                alternatives: None,
            })
            .expect("send event");

        // Notify drain via PostMessage from another thread
        let join_handle = thread::spawn(move || {
            notify_drain(Some(hwnd_val), 100);
        });
        join_handle.join().expect("join worker thread");

        // Pump messages on this STA thread
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut msg = MSG::default();
        loop {
            unsafe {
                if PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                    if msg.message == WM_SUMIBI_DRAIN {
                        break;
                    }
                }
            }
            if Instant::now() >= deadline {
                panic!("timed out waiting for WM_SUMIBI_DRAIN message");
            }
            thread::sleep(Duration::from_millis(5));
        }

        // WindowProc should have called sink.drain(), which consumed the event from event_receiver
        assert!(sink.event_receiver.try_recv().is_err());

        window.destroy();
    }

    #[test]
    fn shared_session_state_protection_profile_roundtrip() {
        let state = SharedSessionState::new();
        // Default must be Standard
        assert_eq!(state.protection_profile(), ProtectionProfile::Standard);

        // Dynamically switch to Strict
        state.set_protection_profile(ProtectionProfile::Strict);
        assert_eq!(state.protection_profile(), ProtectionProfile::Strict);

        // Switch back to Standard
        state.set_protection_profile(ProtectionProfile::Standard);
        assert_eq!(state.protection_profile(), ProtectionProfile::Standard);
    }

    #[test]
    fn drain_sink_cancels_ticket_on_worker_failure_and_leaves_state_clean() {
        let manager = crate::mock::new_manager().to_interface::<ITfThreadMgr>();
        let (evt_tx, evt_rx) = mpsc::sync_channel(4);
        let sink = Rc::new(DrainSink {
            owner: thread::current().id(),
            manager,
            client: 7,
            state: Arc::new(SharedSessionState::new()),
            event_receiver: evt_rx,
            pending_ticket: RefCell::new(None),
            last_conversion: RefCell::new(None),
            selection: RefCell::new(None),
        });

        // 1. Send Failed event
        evt_tx
            .send(WorkerEvent::Failed { request_id: 42 })
            .expect("send failed event");

        // 2. Drain should safely handle the Failed event even without a pending ticket
        assert!(!sink.drain().unwrap());
        assert!(sink.event_receiver.try_recv().is_err());
    }

    #[test]
    fn session_denies_conversion_hotkey_when_unconsented_or_missing_key() {
        if !supported() {
            return;
        }
        let manager = crate::mock::new_manager().to_interface::<ITfThreadMgr>();
        let session = TipHostSession::start(&manager, 7).expect("start session");

        // In default start state, connected is false and allowed is false
        assert!(!session.state().is_ready());
        assert!(!session.state().is_connected());

        // Pressing preserved key immediately returns FALSE (fail-closed)
        let res = session
            .on_preserved_key(None, &COMMAND_CONVERT)
            .expect("on_preserved_key unready");
        assert!(!res.as_bool());

        // Even if connected is true, without allowed (consent + credential), key is denied
        session.state().connected.store(true, Ordering::Release);
        assert!(!session.state().is_ready());
        let res2 = session
            .on_preserved_key(None, &COMMAND_CONVERT)
            .expect("on_preserved_key connected but unconsented");
        assert!(!res2.as_bool());

        session.close().expect("close session");
    }
}
